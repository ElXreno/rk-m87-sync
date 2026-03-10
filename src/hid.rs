use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::{AsFd, AsRawFd};
use std::path::{Path, PathBuf};

use log::{debug, warn};
use nix::poll::{poll, PollFd, PollFlags, PollTimeout};

use crate::error::{Error, Result};
use crate::protocol::{
    self, build_feature_report, build_output_report, Protocol, CMD_ECHO, CMD_SYSPARAM, VID,
};

/// Trait abstracting keyboard HID sends (for testing with mocks).
#[cfg_attr(not(test), allow(dead_code))]
pub trait KeyboardSink {
    fn send_sysparam(&self, payload: &[u8; 14]) -> Result<bool>;
}

const READ_TIMEOUT_MS: u16 = 500;

/// ioctl number for HIDIOCSFEATURE (set feature report).
/// _IOC(dir=WR, type='H', nr=0x06, size)
///
/// We use raw ioctl instead of the `hidapi` crate because hidapi-rs's
/// linux-native backend incorrectly uses _IOW (dir=1) instead of _IOWR
/// (dir=3). Kernels >= 6.16 tightened hidraw ioctl parsing and reject
/// the wrong direction bits with EINVAL.
fn hidiocsfeature(size: usize) -> libc::c_ulong {
    const IOC_WRITE: libc::c_ulong = 1;
    const IOC_READ: libc::c_ulong = 2;
    ((IOC_WRITE | IOC_READ) << 30)
        | ((size as libc::c_ulong) << 16)
        | ((b'H' as libc::c_ulong) << 8)
        | 0x06
}

pub struct DetectedDevice {
    pub path: PathBuf,
    pub protocol: Protocol,
}

/// Scan sysfs for hidraw devices matching VID 258A with a known PID on input1.
pub fn find_devices() -> Vec<DetectedDevice> {
    let sysfs = Path::new("/sys/class/hidraw");
    let Ok(entries) = std::fs::read_dir(sysfs) else {
        return Vec::new();
    };

    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();

    let vid_str = format!("{:08X}", VID);
    let mut devices = Vec::new();

    for name in &names {
        let uevent_path = sysfs.join(name).join("device/uevent");
        let Ok(uevent) = std::fs::read_to_string(&uevent_path) else {
            continue;
        };
        let uevent_upper = uevent.to_uppercase();

        if !uevent_upper.contains(&vid_str) {
            continue;
        }

        let mut matched = None;
        for &(pid, proto) in protocol::KNOWN_DEVICES {
            let needle = format!("{vid_str}:{pid:08X}");
            if uevent_upper.contains(&needle) {
                matched = Some(proto);
                break;
            }
        }
        let Some(proto) = matched else { continue };

        // Must be input1 (interface 1 = vendor config channel)
        let is_input1 = uevent.lines().any(|line| {
            line.starts_with("HID_PHYS=") && line.ends_with("/input1")
        });
        if !is_input1 {
            continue;
        }

        devices.push(DetectedDevice {
            path: PathBuf::from(format!("/dev/{name}")),
            protocol: proto,
        });
    }

    devices
}

/// Look up the protocol for a manually-specified hidraw device.
pub fn get_protocol_for_device(path: &Path) -> Option<Protocol> {
    let name = path.file_name()?.to_str()?;
    let uevent_path = Path::new("/sys/class/hidraw").join(name).join("device/uevent");
    let uevent = std::fs::read_to_string(uevent_path).ok()?;
    let uevent_upper = uevent.to_uppercase();
    let vid_str = format!("{:08X}", VID);

    for &(pid, proto) in protocol::KNOWN_DEVICES {
        let needle = format!("{vid_str}:{pid:08X}");
        if uevent_upper.contains(&needle) {
            return Some(proto);
        }
    }
    None
}

/// Opened HID device handle using raw fd operations.
pub struct HidDevice {
    file: File,
    pub protocol: Protocol,
    pub path: PathBuf,
}

impl HidDevice {
    pub fn open(path: &Path, protocol: Protocol) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
            .map_err(|e| match e.kind() {
                io::ErrorKind::PermissionDenied => Error::PermissionDenied {
                    path: path.to_owned(),
                },
                _ => Error::Io(e),
            })?;

        Ok(Self {
            file,
            protocol,
            path: path.to_owned(),
        })
    }

    /// Send a 20-byte output report and read response with timeout.
    fn send_and_recv_output(&self, pkt: &[u8; 20]) -> Result<Option<[u8; 20]>> {
        // Drain any stale data from the read buffer before writing.
        // The fd is O_NONBLOCK, so read returns WouldBlock when the
        // buffer is empty. Without this, a late-arriving response
        // from a previous send (or an unsolicited report) would be
        // consumed by our poll/read below instead of the actual
        // response to this write.
        let mut drain = [0u8; 20];
        loop {
            match (&self.file).read(&mut drain) {
                Ok(0) => break,
                Ok(_) => continue,
                Err(_) => break, // WouldBlock = buffer empty
            }
        }

        // Write
        if (&self.file).write(pkt).is_err() {
            return Err(Error::DeviceDisconnected {
                path: self.path.clone(),
            });
        }

        // Poll for response
        let mut pollfds = [PollFd::new(self.file.as_fd(), PollFlags::POLLIN)];
        let ready = poll(&mut pollfds, PollTimeout::from(READ_TIMEOUT_MS))
            .map_err(|e| Error::Io(e.into()))?;
        if ready == 0 {
            return Ok(None);
        }

        // Read response
        let mut resp = [0u8; 20];
        match (&self.file).read(&mut resp) {
            Ok(n) if n >= 20 => Ok(Some(resp)),
            _ => Ok(None),
        }
    }

    /// Send a feature report via ioctl HIDIOCSFEATURE.
    fn send_feature_report(&self, buf: &mut [u8; 520]) -> Result<()> {
        let fd = self.file.as_raw_fd();
        // SAFETY: HIDIOCSFEATURE is the only hidraw ioctl with no safe wrapper.
        // The buffer is a valid mutable [u8; 520] and fd is an open hidraw device.
        let ret = unsafe { libc::ioctl(fd, hidiocsfeature(520), buf.as_mut_ptr()) };
        if ret < 0 {
            return Err(Error::DeviceDisconnected {
                path: self.path.clone(),
            });
        }
        Ok(())
    }

    /// Dongle pre-flight: send Echo (cmdId 0x09) and verify response.
    pub fn echo_ping(&self) -> Result<bool> {
        let payload = [0x0E, 0xDE, 0xAD];
        let pkt = build_output_report(0x13, CMD_ECHO, &payload);

        let resp = self.send_and_recv_output(&pkt)?;
        let Some(resp) = resp else {
            return Ok(false);
        };

        if resp[1] & 0x7F != CMD_ECHO {
            return Ok(false);
        }
        if resp[1] & 0x80 != 0 {
            return Ok(false);
        }
        Ok(true)
    }

    /// USB cable pre-flight: send a feature report probe.
    pub fn probe_feature(&self) -> Result<()> {
        let payload = [0xDE, 0xAD];
        let mut buf = build_feature_report(self.protocol.report_id(), CMD_ECHO, &payload);
        self.send_feature_report(&mut buf)
    }

    /// Send SysParam to the keyboard. Returns true if the keyboard acknowledged.
    /// For USB cable (fire-and-forget), always returns true on success.
    /// For dongle, returns false if no response was received (keyboard may be off/switched).
    fn send_sysparam_impl(&self, payload: &[u8; 14]) -> Result<bool> {
        match self.protocol {
            Protocol::Dongle => {
                let pkt = build_output_report(self.protocol.report_id(), CMD_SYSPARAM, payload);

                let resp = self.send_and_recv_output(&pkt)?;
                match resp {
                    None => Ok(false),
                    Some(resp) if resp[1] & 0x80 != 0 => Err(Error::Crc),
                    Some(_) => Ok(true),
                }
            }
            Protocol::UsbCable => {
                let mut buf =
                    build_feature_report(self.protocol.report_id(), CMD_SYSPARAM, payload);
                self.send_feature_report(&mut buf)?;
                Ok(true)
            }
        }
    }

    /// Public send_sysparam that delegates to the internal implementation.
    pub fn send_sysparam(&self, payload: &[u8; 14]) -> Result<bool> {
        self.send_sysparam_impl(payload)
    }

    /// Format the packet for display (verbose one-shot mode).
    pub fn format_packet(&self, payload: &[u8; 14]) -> String {
        match self.protocol {
            Protocol::Dongle => {
                let pkt = build_output_report(self.protocol.report_id(), CMD_SYSPARAM, payload);
                format!("Packet: {}", hex(&pkt))
            }
            Protocol::UsbCable => {
                let buf = build_feature_report(self.protocol.report_id(), CMD_SYSPARAM, payload);
                format!("Packet: {}... ({} bytes)", hex(&buf[..22]), buf.len())
            }
        }
    }
}

impl KeyboardSink for HidDevice {
    fn send_sysparam(&self, payload: &[u8; 14]) -> Result<bool> {
        self.send_sysparam_impl(payload)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Discover, open, and probe a keyboard device.
pub fn connect(device: Option<&Path>, no_ping: bool) -> Result<HidDevice> {
    let candidates: Vec<DetectedDevice> = if let Some(path) = device {
        let proto = get_protocol_for_device(path).unwrap_or_else(|| {
            warn!("Unknown PID for {}, assuming dongle protocol", path.display());
            Protocol::Dongle
        });
        vec![DetectedDevice {
            path: path.to_owned(),
            protocol: proto,
        }]
    } else {
        let devs = find_devices();
        if devs.is_empty() {
            return Err(Error::NoDevice);
        }
        devs
    };

    let mut last_error: Option<String> = None;

    for det in &candidates {
        let dev = match HidDevice::open(&det.path, det.protocol) {
            Ok(d) => d,
            Err(Error::PermissionDenied { path }) => {
                return Err(Error::PermissionDenied { path });
            }
            Err(e) => {
                last_error = Some(format!("{}: {e}", det.path.display()));
                continue;
            }
        };

        match det.protocol {
            Protocol::Dongle if !no_ping => match dev.echo_ping() {
                Ok(true) => return Ok(dev),
                Ok(false) => {
                    last_error = Some(format!(
                        "{} ({}): no response",
                        det.path.display(),
                        det.protocol.label()
                    ));
                    continue;
                }
                Err(e) => {
                    last_error = Some(format!("{}: {e}", det.path.display()));
                    continue;
                }
            },
            Protocol::UsbCable => match dev.probe_feature() {
                Ok(()) => return Ok(dev),
                Err(e) => {
                    last_error = Some(format!(
                        "{} ({}): feature report failed: {e}",
                        det.path.display(),
                        det.protocol.label()
                    ));
                    continue;
                }
            },
            Protocol::Dongle => {
                // no_ping=true, skip ping
                return Ok(dev);
            }
        }
    }

    if let Some(msg) = &last_error {
        debug!("{msg}");
    }
    Err(Error::NoDevice)
}

#!/usr/bin/env python3
"""Sync system time and volume to RK M87 keyboard LCD.

Reverse engineered from RK_Keyboard_Software DeviceDriver.exe.

Supports two connection modes:
  - Dongle (PID 0x0150): CDev3632 output report protocol (Report 0x13, 20 bytes)
  - USB cable (PID 0x01A2): CDevG5KB feature report protocol (Report 0x09, 520 bytes)

SysParam payload (shared between both protocols):
  [0]  vol   - Speaker volume 0-100
  [1]  cpu   - CPU usage % (unused, mask bit 1)
  [2]  mem   - Memory usage % (unused, mask bit 1)
  [3]  yr_lo - Year low byte (raw 16-bit LE)
  [4]  yr_hi - Year high byte
  [5]  month - Month (1-12)
  [6]  day   - Day (1-31)
  [7]  hour  - Hour (0-23)
  [8]  minute- Minute (0-59)
  [9]  second- Second (0-59)
  [10] dow   - Day of week (0=Sun, 1=Mon, ..., 6=Sat)
"""
import argparse
import array
import datetime
import fcntl
import os
import select
import signal
import subprocess
import sys
import time

VID = "0000258A"
# PID → (report_id, protocol)
KNOWN_PIDS = {
    "00000150": (0x13, "output"),   # 2.4 GHz dongle → CDev3632
    "000001A2": (0x09, "feature"),  # USB cable → CDevG5KB
}
CMD_ECHO = 0x09
CMD_SYSPARAM = 0x0B
READ_TIMEOUT = 0.5  # seconds

# ioctl numbers for HID feature reports
# _IOC(dir, type, nr, size) = (dir << 30) | (size << 16) | (type << 8) | nr
_IOC_WRITE = 1
_IOC_READ = 2


def _HIDIOCSFEATURE(size: int) -> int:
    return ((_IOC_WRITE | _IOC_READ) << 30) | (size << 16) | (ord("H") << 8) | 0x06


def _HIDIOCGFEATURE(size: int) -> int:
    return ((_IOC_WRITE | _IOC_READ) << 30) | (size << 16) | (ord("H") << 8) | 0x07


def find_devices() -> list[tuple[str, str]]:
    """Auto-detect all RK M87 hidraw devices (vendor config interface).

    Returns list of (device_path, pid) tuples, sorted by hidraw number.
    Scans sysfs for hidraw devices matching VID 258a with a known PID on input1.
    """
    sysfs = "/sys/class/hidraw"
    if not os.path.isdir(sysfs):
        return []

    devices = []
    for entry in sorted(os.listdir(sysfs)):
        uevent_path = os.path.join(sysfs, entry, "device", "uevent")
        try:
            with open(uevent_path) as f:
                uevent = f.read()
        except OSError:
            continue

        # Check VID match and find which PID
        uevent_upper = uevent.upper()
        if VID not in uevent_upper:
            continue

        matched_pid = None
        for pid in KNOWN_PIDS:
            if f"{VID}:{pid}" in uevent_upper:
                matched_pid = pid
                break
        if matched_pid is None:
            continue

        # Must be input1 (interface 1 = vendor config channel)
        for line in uevent.splitlines():
            if line.startswith("HID_PHYS=") and line.endswith("/input1"):
                devices.append((f"/dev/{entry}", matched_pid))

    return devices


def get_pid_for_device(device: str) -> str | None:
    """Look up the PID for a manually-specified hidraw device."""
    sysfs_uevent = f"/sys/class/hidraw/{os.path.basename(device)}/device/uevent"
    try:
        with open(sysfs_uevent) as f:
            uevent = f.read().upper()
    except OSError:
        return None

    for pid in KNOWN_PIDS:
        if f"{VID}:{pid}" in uevent:
            return pid
    return None


def crc(pkt: bytearray) -> int:
    return sum(pkt[:19]) & 0xFF


def send_and_recv_output(fd: int, pkt: bytes, timeout: float = READ_TIMEOUT) -> bytes | None:
    """Send a 20-byte output report and read the response with timeout."""
    os.write(fd, pkt)

    ready, _, _ = select.select([fd], [], [], timeout)
    if not ready:
        return None

    return os.read(fd, 20)


def send_feature_report(fd: int, buf: bytearray) -> None:
    """Send a feature report via ioctl HIDIOCSFEATURE."""
    # fcntl.ioctl with a mutable buffer modifies it in-place
    fcntl.ioctl(fd, _HIDIOCSFEATURE(len(buf)), buf)


def echo_ping(fd: int) -> bool:
    """Send Echo (cmdId 0x09) via output report and verify the device responds.

    Only used for dongle mode (CDev3632 output reports).
    """
    pkt = bytearray(20)
    pkt[0] = 0x13  # Report ID for output report
    pkt[1] = CMD_ECHO
    pkt[2] = 0x01
    pkt[3] = 0x00
    pkt[4] = 0x0E
    pkt[5] = 0xDE
    pkt[6] = 0xAD
    pkt[19] = crc(pkt)

    resp = send_and_recv_output(fd, bytes(pkt))
    if resp is None or len(resp) < 20:
        return False
    if resp[1] & 0x7F != CMD_ECHO:
        return False
    if resp[1] & 0x80:
        return False

    return True


def get_volume() -> int:
    """Get current default sink volume (0-100) via wpctl or pactl."""
    # Try WirePlumber (PipeWire) first
    try:
        out = subprocess.run(
            ["wpctl", "get-volume", "@DEFAULT_AUDIO_SINK@"],
            capture_output=True, text=True, timeout=2,
        )
        if out.returncode == 0:
            parts = out.stdout.strip().split()
            vol_float = float(parts[1])
            muted = "[MUTED]" in out.stdout
            return 0 if muted else min(100, round(vol_float * 100))
    except (FileNotFoundError, IndexError, ValueError, subprocess.TimeoutExpired):
        pass

    # Fallback to PulseAudio
    try:
        out = subprocess.run(
            ["pactl", "get-sink-volume", "@DEFAULT_SINK@"],
            capture_output=True, text=True, timeout=2,
        )
        if out.returncode == 0:
            for part in out.stdout.split("/"):
                part = part.strip()
                if part.endswith("%"):
                    return min(100, int(part[:-1].strip()))
    except (FileNotFoundError, ValueError, subprocess.TimeoutExpired):
        pass

    return 0


def get_volume_pulsectl(pulse) -> int:
    """Get current default sink volume (0-100) via pulsectl."""
    info = pulse.server_info()
    default_name = info.default_sink_name
    for sink in pulse.sink_list():
        if sink.name == default_name:
            if sink.mute:
                return 0
            vol = pulse.volume_get_all_chans(sink)
            return min(100, round(vol * 100))
    return 0


def build_sysparam_payload(volume: int, now: datetime.datetime) -> bytes:
    """Build the 14-byte SysParam payload (shared between both protocols)."""
    year = now.year
    dow = (now.weekday() + 1) % 7  # Python Mon=0 → Windows Sun=0

    payload = bytearray(14)
    payload[0] = volume & 0xFF
    payload[1] = 0x00  # cpu
    payload[2] = 0x00  # mem
    payload[3] = year & 0xFF
    payload[4] = (year >> 8) & 0xFF
    payload[5] = now.month
    payload[6] = now.day
    payload[7] = now.hour
    payload[8] = now.minute
    payload[9] = now.second
    payload[10] = dow
    return bytes(payload)


def build_output_report(report_id: int, cmd_id: int, payload: bytes) -> bytes:
    """Build a 20-byte CDev3632 output report packet."""
    pkt = bytearray(20)
    pkt[0] = report_id
    pkt[1] = cmd_id
    pkt[2] = 0x01  # numPackages
    pkt[3] = 0x00  # packageIndex
    pkt[4] = len(payload) & 0x0F  # meta: (board=0 << 4) | dataLen
    pkt[5 : 5 + len(payload)] = payload
    pkt[19] = sum(pkt[:19]) & 0xFF  # CRC
    return bytes(pkt)


def build_feature_report(report_id: int, cmd_id: int, payload: bytes) -> bytearray:
    """Build a 520-byte CDevG5KB feature report buffer."""
    buf = bytearray(520)
    buf[0] = report_id
    buf[1] = cmd_id
    buf[2] = 0x00  # board
    buf[3] = 0x00  # reserved
    buf[4] = 0x01  # numPackages
    buf[5] = 0x00  # packageIndex
    buf[6] = len(payload) & 0xFF  # dataLenLow
    buf[7] = (len(payload) >> 8) & 0xFF  # dataLenHigh
    buf[8 : 8 + len(payload)] = payload
    return buf


def send_sysparam(fd: int, protocol: str, report_id: int, payload: bytes,
                  verbose: bool = True) -> bool:
    """Send SysParam to the keyboard. Returns True on success."""
    if protocol == "output":
        pkt = build_output_report(report_id, CMD_SYSPARAM, payload)
        if verbose:
            print(f"Packet: {pkt.hex()}")

        resp = send_and_recv_output(fd, pkt)
        if resp is None:
            if verbose:
                print("Warning: No response from keyboard (sent anyway)", file=sys.stderr)
            return False

        if resp[1] & 0x80:
            if verbose:
                print("Error: Keyboard reported CRC error", file=sys.stderr)
            return False

        if verbose and resp[1] & 0x7F != CMD_SYSPARAM:
            print(
                f"Warning: Unexpected response cmdId=0x{resp[1] & 0x7F:02x}",
                file=sys.stderr,
            )
        return True
    else:
        buf = build_feature_report(report_id, CMD_SYSPARAM, payload)
        if verbose:
            print(f"Packet: {buf[:22].hex()}... ({len(buf)} bytes)")
        send_feature_report(fd, buf)
        return True


def connect(device: str | None, no_ping: bool) -> tuple[int, str, int, str, str] | None:
    """Discover and open a working keyboard device.

    Returns (fd, protocol, report_id, device_path, mode_label) or None.
    Prints errors/permission issues to stderr.
    """
    if device:
        pid = get_pid_for_device(device)
        if pid is None:
            print(f"Warning: Unknown PID for {device}, assuming dongle protocol",
                  file=sys.stderr)
            pid = "00000150"
        candidates = [(device, pid)]
    else:
        candidates = find_devices()
        if not candidates:
            return None

    last_error = ""
    for dev, pid in candidates:
        report_id, protocol = KNOWN_PIDS[pid]
        mode = "USB cable" if protocol == "feature" else "2.4 GHz dongle"

        try:
            fd = os.open(dev, os.O_RDWR | os.O_NONBLOCK)
        except FileNotFoundError:
            last_error = f"{dev} does not exist"
            continue
        except PermissionError:
            print(f"Error: Permission denied on {dev}", file=sys.stderr)
            print("Fix: add udev rule or run as root", file=sys.stderr)
            return None
        except OSError as e:
            last_error = f"Cannot open {dev}: {e}"
            continue

        ok = False
        try:
            if protocol == "output" and not no_ping:
                if not echo_ping(fd):
                    last_error = f"{dev} ({mode}): no response"
                    continue

            if protocol == "feature":
                try:
                    probe = build_feature_report(report_id, CMD_ECHO, b"\xde\xad")
                    send_feature_report(fd, probe)
                except OSError:
                    last_error = f"{dev} ({mode}): feature report failed"
                    continue

            ok = True
            return (fd, protocol, report_id, dev, mode)
        finally:
            if not ok:
                os.close(fd)

    if last_error:
        print(f"Last: {last_error}", file=sys.stderr)
    return None


def watch_loop(device: str | None, no_ping: bool) -> int:
    """Daemon mode: continuously sync time and volume to the keyboard."""
    import pulsectl

    shutdown = False

    def _on_signal(signum, frame):
        nonlocal shutdown
        shutdown = True

    signal.signal(signal.SIGINT, _on_signal)
    signal.signal(signal.SIGTERM, _on_signal)

    while not shutdown:
        # --- connect to keyboard ---
        conn = connect(device, no_ping)
        if conn is None:
            print("Waiting for keyboard...", file=sys.stderr)
            time.sleep(2)
            continue

        fd, protocol, report_id, dev, mode = conn
        print(f"Connected: {dev} ({mode})")

        # --- connect to PulseAudio ---
        try:
            pulse_events = pulsectl.Pulse("rk-m87-events")
            pulse_query = pulsectl.Pulse("rk-m87-query")
        except pulsectl.PulseError as e:
            print(f"PulseAudio connect failed: {e}", file=sys.stderr)
            os.close(fd)
            time.sleep(2)
            continue

        pulse_events.event_mask_set("sink")
        pulse_events.event_callback_set(lambda ev: raise_pulse_stop())

        TIME_SYNC_INTERVAL = 30 * 60  # seconds
        # Each send_sysparam blocks the keyboard's USB/firmware for
        # ~300ms total.  Debounce volume changes so we only send once
        # after scrolling stops — keeps EP0 free for encoder HID
        # reports and preserves native scroll speed.
        VOL_DEBOUNCE = 0.5  # seconds of silence before sending
        last_vol = -1
        vol_dirty = False
        vol_last_change = 0.0
        next_time_sync = 0.0

        try:
            while not shutdown:
                now_mono = time.monotonic()

                if vol_dirty:
                    remaining = VOL_DEBOUNCE - (now_mono - vol_last_change)
                    timeout = max(0.01, remaining)
                else:
                    timeout = min(2.0, max(1.0, next_time_sync - now_mono))

                try:
                    pulse_events.event_listen(timeout=timeout)
                except pulsectl.PulseDisconnected:
                    print("PulseAudio disconnected, reconnecting...", file=sys.stderr)
                    break

                try:
                    vol = get_volume_pulsectl(pulse_query)
                except pulsectl.PulseDisconnected:
                    print("PulseAudio disconnected, reconnecting...", file=sys.stderr)
                    break

                now_mono = time.monotonic()

                if vol != last_vol:
                    last_vol = vol
                    vol_dirty = True
                    vol_last_change = now_mono
                    continue

                time_due = now_mono >= next_time_sync
                debounce_expired = vol_dirty and (now_mono - vol_last_change) >= VOL_DEBOUNCE

                if debounce_expired:
                    vol_dirty = False
                    print(f"Volume: {last_vol}%")
                elif time_due:
                    pass
                else:
                    continue

                now = datetime.datetime.now()
                payload = build_sysparam_payload(last_vol if last_vol >= 0 else 0, now)
                try:
                    send_sysparam(fd, protocol, report_id, payload, verbose=False)
                except (OSError, BrokenPipeError):
                    print(f"Device disconnected, reconnecting...", file=sys.stderr)
                    break

                if time_due:
                    print(f"Time synced: {now.strftime('%H:%M:%S')}")
                    next_time_sync = now_mono + TIME_SYNC_INTERVAL
        finally:
            os.close(fd)
            pulse_events.close()
            pulse_query.close()

    print("\nShutdown.")
    return 0


def raise_pulse_stop():
    """Raise PulseLoopStop to break out of event_listen."""
    import pulsectl
    raise pulsectl.PulseLoopStop()


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Sync system time and volume to RK M87 keyboard LCD"
    )
    parser.add_argument(
        "--device", "-d",
        help="hidraw device path (auto-detected if omitted)",
    )
    parser.add_argument(
        "--no-ping", action="store_true",
        help="skip echo ping check (dongle mode only)",
    )
    parser.add_argument(
        "--watch", "-w", action="store_true",
        help="daemon mode: continuously sync time and volume",
    )
    args = parser.parse_args()

    if args.watch:
        try:
            import pulsectl  # noqa: F401
        except ImportError:
            print("Error: --watch requires pulsectl", file=sys.stderr)
            print("Install: nix-shell -p python3Packages.pulsectl", file=sys.stderr)
            return 1
        return watch_loop(args.device, args.no_ping)

    # --- One-shot mode (original behavior) ---
    conn = connect(args.device, args.no_ping)
    if conn is None:
        print(
            "Error: No responding keyboard found",
            file=sys.stderr,
        )
        return 1

    fd, protocol, report_id, dev, mode = conn
    try:
        now = datetime.datetime.now()
        vol = get_volume()
        payload = build_sysparam_payload(vol, now)

        print(f"Device: {dev} ({mode})")
        print(f"Time:   {now.strftime('%Y-%m-%d %H:%M:%S')}")
        print(f"Volume: {vol}%")

        ok = send_sysparam(fd, protocol, report_id, payload)
        if ok:
            print("Synced!")
            return 0
        return 3
    except OSError as e:
        print(f"Error: {e}", file=sys.stderr)
        return 3
    finally:
        os.close(fd)


if __name__ == "__main__":
    sys.exit(main())

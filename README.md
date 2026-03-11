# rk-m87-sync

Sync system time and volume to the Royal Kludge M87 keyboard LCD on Linux.

> **WARNING:** This project is fully vibe-coded with [Claude Opus 4.6](https://docs.anthropic.com/en/docs/about-claude/models).
> It writes directly to HID devices. Use at your own risk.

## Tested Hardware

| Keyboard | Connection |
|----------|------------|
| Royal Kludge M87 | USB cable (PID `01A2`) |
| Royal Kludge M87 | 2.4GHz dongle (PID `0150`) |

> Other RK keyboards with LCD screens may work if they share the same protocol.
> Open an issue with your model if it works or fails.

## Requirements

- Linux with `hidraw` support
- PulseAudio or PipeWire (uses libpulse directly)
- Read/write access to the hidraw device (via udev rule or root)

## Installation

### NixOS (module)

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rk-m87-sync = {
      url = "github:ElXreno/rk-m87-sync";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { nixpkgs, rk-m87-sync, ... }:
    {
      nixosConfigurations."hostname" = nixpkgs.lib.nixosSystem {
        modules = [
          rk-m87-sync.nixosModules.default
          {
            services.rk-m87-sync.enable = true;
          }
        ];
      };
    };
}
```

This sets up the systemd user service, udev rules, and the package automatically.

### Pre-built binaries

Download `.deb`, `.rpm`, or tarball from the
[Releases](https://github.com/ElXreno/rk-m87-sync/releases) page.

### Manual build

Requires `pkg-config` and `libpulse` development headers (e.g. `libpulse-dev` on Debian/Ubuntu, `libpulse` on Arch).

```console
cargo build --release
```

## Usage

```
rk-m87-sync — Sync system time and volume to RK M87 keyboard LCD

Usage: rk-m87-sync [-d <device>] [--no-ping] [--daemon]

Options:
  -d, --device      hidraw device path (auto-detected if omitted)
  --no-ping         skip echo ping check (dongle mode only)
  --daemon          daemon mode: continuously sync time and volume
  --help, help      display usage information
```

### One-shot sync

```console
rk-m87-sync
```

### Daemon mode

Continuously syncs time every 30 minutes and volume on change:

```console
rk-m87-sync --daemon
```

## Udev rule

To allow access without root, create `/etc/udev/rules.d/99-rk-m87.rules`:

```
# RK M87 keyboard (USB cable)
SUBSYSTEM=="hidraw", ATTRS{idVendor}=="258a", ATTRS{idProduct}=="01a2", MODE="0660", TAG+="uaccess"

# RK M87 dongle
SUBSYSTEM=="hidraw", ATTRS{idVendor}=="258a", ATTRS{idProduct}=="0150", MODE="0660", TAG+="uaccess"
```

Then reload: `sudo udevadm control --reload-rules && sudo udevadm trigger`

## Known Limitations

- **Volume bar mismatch during scrolling** — the keyboard's screen controller increments volume by 2% per encoder tick (hardcoded in firmware), which may differ from the OS volume step. The LCD bar drifts during scrolling and snaps to the correct value ~500ms after you stop.
- **Encoder blocked during SysParam sends** — each HID update takes ~25-50ms to send and process, blocking the rotary encoder. Volume updates are debounced (500ms quiet period) to preserve native scroll speed.

## How it works

<details>
<summary>Protocol details</summary>

The keyboard has no hardware RTC — it relies on the host PC to send the current
time via HID reports. The Windows software does this silently in the background.

This tool replicates the protocol:
- **USB cable:** 520-byte feature reports (Report ID `0x09`) via `ioctl(HIDIOCSFEATURE)`
- **Dongle:** 20-byte output reports (Report ID `0x13`) with CRC byte

Both modes use command ID `0x0B` (SetScreenParam) carrying a 14-byte payload
with volume, time, and date fields.

See [docs/PROTOCOL.md](docs/PROTOCOL.md) for the full protocol specification.

</details>

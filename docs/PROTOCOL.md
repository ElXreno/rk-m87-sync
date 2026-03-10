# Royal Kludge M87 USB HID Protocol

Reverse engineered from `RK_Keyboard_Software_Setup_V4.6 20250617.exe` (`DeviceDriver.exe`)
and live hardware probing on Linux.

---

## 1. Device Identification

| Field          | Value                                  |
|----------------|----------------------------------------|
| USB VID        | `0x258A` (SINOWEALTH)                  |
| Chip           | SINOWEALTH SH68F90A (8051-based MCU)   |
| Keyboard Model | Royal Kludge M87 (87-key TKL, LCD)     |
| LCD Resolution | 320×172 pixels (RGB, configurable GIF) |
| RK Software PID| `0x01AF` (internal config folder)       |

The keyboard has no hardware RTC — time is maintained in software and must be
synced from the host on every connect.

### Connection Modes

The keyboard supports two connection modes, each with a different USB PID
and HID protocol:

| Mode           | USB PID    | Product String             | Protocol    |
|----------------|------------|----------------------------|-------------|
| 2.4 GHz dongle | `0x0150`   | "SINOWEALTH Gaming KB"     | CDev3632 (output reports)  |
| USB cable      | `0x01A2`   | "SINO WEALTH Gaming Keyboard" | CDevG5KB (feature reports) |

Both modes use the **same SysParam payload** (cmdId 0x0B) for time/volume sync —
only the transport framing differs.

---

## 2. USB Interfaces & HID Reports

Both connection modes expose **two HID interfaces**:

| Interface | Purpose                    | Usage Page        |
|-----------|----------------------------|--------------------|
| 0         | Boot keyboard (6KRO)       | Generic Desktop    |
| 1         | Vendor config channel      | Vendor-defined     |

**Interface 1** (`/inputN` where N=1 in `HID_PHYS`) is used for all
configuration, time sync, and profile management.

### Dongle Mode (PID 0x0150)

Interface 1 supports:
- **Output Report 0x13**: 20 bytes (Report ID + 19 data)

Uses the **CDev3632** protocol — output reports with per-packet CRC
and read-back response.

### USB Cable Mode (PID 0x01A2)

Interface 1 supports:
- **Feature Report 0x09**: 520 bytes (Report ID + 519 data)
- Plus additional reports (0x05, 0x06) for other config

Uses the **CDevG5KB** protocol — feature reports via
`ioctl(HIDIOCSFEATURE)` / `HidD_SetFeature`. The report ID is **0x09**
(not 0x0A as on some other RK models — the report ID is read from the
device object at runtime, not hardcoded).

---

## 3. Packet Format (CDev3632 Protocol)

All communication uses **Report ID 0x13** output reports, 20 bytes total:

```
Offset  Size  Field         Description
──────  ────  ────────────  ──────────────────────────────────────
  0      1    reportId      Always 0x13
  1      1    cmdId         Command identifier (see §4)
  2      1    numPackages   Total packets in this transfer (1 for single-packet)
  3      1    packageIndex  0-based index of current packet
  4      1    meta          (board << 4) | dataLen
  5-18   14   payload       Command-specific data (zero-padded)
  19     1    crc           Checksum: sum(bytes[0..18]) & 0xFF
```

### Meta Byte (offset 4)

```
Bits 7-4: board    — board identifier (usually 0)
Bits 3-0: dataLen  — number of valid payload bytes in this packet (max 14 = 0x0E)
```

### CRC Calculation

Simple 8-bit sum of bytes 0 through 18 (inclusive), masked to one byte:

```python
crc = sum(packet[0:19]) & 0xFF
```

### Multi-Packet Transfers

For commands transferring more than 14 bytes (e.g., GetProfile = 128 bytes):
- `numPackages` = ceil(totalBytes / 14)
- `packageIndex` increments 0, 1, 2, ...
- Each packet carries up to 14 bytes of payload
- Example: 128-byte profile → 10 packets (9 × 14 + 1 × 2)

### Response Format

Responses use the same 20-byte format. After sending a command, the host reads
a response with the same `cmdId`. The keyboard echoes the cmdId and includes
response data in the payload. The host validates:
1. `cmdId` matches the request
2. CRC is correct
3. `packageIndex` matches expected sequence

---

## 4. Known Command IDs

| cmdId | Name              | Dir  | Description                                |
|-------|-------------------|------|--------------------------------------------|
| 0x01  | Echo              | R/W  | Echoes back the sent data                  |
| 0x02  | Echo              | R/W  | Echoes back the sent data                  |
| 0x03  | SetMatrix         | W    | Set key matrix / layer data                |
| 0x04  | SetProfile        | W    | Write profile data (128 bytes, multi-pkt)  |
| 0x05  | GetPassword       | R    | Get device password/auth (6 bytes)         |
| 0x06  | SetPassword       | W    | Set password (no response expected)        |
| 0x07  | GetDongleStatus   | R    | Dongle/connection status                   |
| 0x09  | Echo              | R/W  | Echoes back the sent data                  |
| 0x0A  | SetLedRgbTab      | W    | Set RGB LED color table                    |
| 0x0B  | **SetScreenParam**| W    | **SysParam: time, volume, CPU, memory**    |
| 0x0C–0x0F | (Time-related)| W   | Partial time set (legacy/undocumented)     |
| 0x11  | ResetDevice       | W    | Reset keyboard to defaults                 |
| 0x12  | SendWeb           | W    | Web/cloud related data                     |
| 0x44  | GetProfile        | R    | Read profile data (128 bytes, multi-pkt)   |

### cmdId 0x0B — SetScreenParam (SysParam)

This is the primary command for syncing time, volume, CPU, and memory data
to the keyboard LCD. The Windows software sends this:
- Once on connect (if `SendTimeOnOpenKB=1` in KB.ini)
- Periodically via `SysParam_Thread` (interval = `SendSysParam` ms in KB.ini)

---

## 5. SysParam Payload (cmdId 0x0B)

The 14-byte payload encodes system state. Which fields the keyboard reads
is controlled by `SysParamMask` in KB.ini:

```
Offset  Field         Mask Bit  Description
──────  ────────────  ────────  ──────────────────────────────────
  0     volume        bit 2     Speaker volume, 0-100 (integer %)
  1     cpuUsage      bit 1     CPU usage, 0-100 (integer %)
  2     memoryLoad    bit 1     RAM usage, 0-100 (integer %)
  3     yearLow       bit 0     Year, low byte (e.g. 0xEA for 2026)
  4     yearHigh      bit 0     Year, high byte (e.g. 0x07 for 2026)
  5     month         bit 0     Month, 1-12
  6     day           bit 0     Day, 1-31
  7     hour          bit 0     Hour, 0-23
  8     minute        bit 0     Minute, 0-59
  9     second        bit 0     Second, 0-59
  10    dayOfWeek     bit 0     0=Sunday, 1=Monday, ..., 6=Saturday
  11-13 (padding)     —         Always 0x00
```

### Important Notes

- **All values are raw integers** — NOT BCD encoded
- **Year is 16-bit little-endian** (e.g., 2026 = `0x07EA` → bytes `EA 07`)
- DayOfWeek follows Windows SYSTEMTIME convention (0 = Sunday)
- Volume comes from Windows IAudioEndpointVolume::GetMasterVolumeLevelScalar
  (float 0.0–1.0 × 100, rounded to integer)
- CPU usage comes from PDH `\Processor Information(_Total)\% Processor Utility`
- Memory load comes from GlobalMemoryStatusEx().dwMemoryLoad

### SysParamMask for RK M87 (PID 0x01AF)

`SysParamMask=5` → binary `101` → bits 0 and 2:
- **Bit 0 (time)**: ✓ keyboard reads time fields
- **Bit 1 (CPU/mem)**: ✗ keyboard ignores CPU/memory
- **Bit 2 (volume)**: ✓ keyboard reads volume

### Example Packet

Setting time to 2026-03-10 14:30:45 (Tuesday), volume 65%:

```
Offset: 00 01 02 03 04 05 06 07 08 09 0A 0B 0C 0D 0E 0F 10 11 12 13
Data:   13 0B 01 00 0E 41 00 00 EA 07 03 0A 0E 1E 2D 02 00 00 00 CRC

Where:
  13    = Report ID
  0B    = cmdId (SetScreenParam)
  01    = 1 packet total
  00    = packet index 0
  0E    = meta (board=0, len=14)
  41    = volume 65%
  00 00 = CPU/memory (unused)
  EA 07 = year 2026 (0x07EA LE)
  03    = March
  0A    = 10th
  0E    = 14 hours
  1E    = 30 minutes
  2D    = 45 seconds
  02    = Tuesday (0=Sun)
  00 00 00 = padding
  CRC   = sum(bytes[0..18]) & 0xFF
```

---

## 6. KB.ini Configuration (Device 0x01AF)

Located at `app/Dev/01AF/KB.ini` inside the installer. Key fields:

```ini
[OPT]
Fw=1                      # Firmware type (1 = newer)
Psd=6,0,0,0,0,23          # Password config (type=6)
ShowScreen=1               # Has LCD screen
RGBScreen=1                # Screen supports RGB
MaxScreenFrame=60          # Max GIF animation frames
siScreen=320,172           # Screen resolution
DataUnitSize=512           # Screen data transfer unit
SendTimeOnOpenKB=1         # Send time on device connect
SendSysParam=1000          # SysParam send interval (ms)
SendOnlyDataChanged=1      # Skip if data unchanged
SysParamMask=5             # Bits: 0=time, 1=cpu/mem, 2=volume
PackageTime=12             # Inter-packet delay (ms)
KbLayout=1                 # Standard 104-key layout reference
KbImgUse=0x0174            # Shares key image with PID 0x0174
LayoutKeyNum=104           # Number of keys
LayerNum=4                 # Number of programmable layers
```

### SysParamMask Values

| Mask | Binary | Fields Sent           |
|------|--------|-----------------------|
| 1    | 001    | Time only             |
| 2    | 010    | CPU + Memory only     |
| 3    | 011    | Time + CPU + Memory   |
| 4    | 100    | Volume only           |
| 5    | 101    | Time + Volume         |
| 7    | 111    | All fields            |

---

## 7. Device Class Hierarchy (DeviceDriver.exe)

The Windows software has three device classes with virtual dispatch:

```
CDev3632          — Base class, uses Report 0x13 output reports (WriteFile)
├── CDevG5KB      — Newer keyboards, uses Feature Reports (HidD_SetFeature)
│                   Can delegate to CDev3632 for dongle-connected devices
└── CDevRKKB      — RK-specific keyboards, stubs out SysParam (no-op)
```

### Vtable Layout (offset → function)

| Offset | CDev3632       | CDevG5KB         | CDevRKKB        |
|--------|----------------|------------------|-----------------|
| 0x00   | Init           | Init             | Init            |
| 0x04   | FindHIDDevice  | FindHIDDevice    | FindHIDDevice   |
| 0x0C   | GetPSD         | GetPSD           | GetPSD          |
| 0x28   | IsWireless     | IsWireless       | (stub)          |
| 0x34   | GetPassword    | SendScreen       | SendScreen      |
| 0x38   | ResetDevice    | ResetDevice      | (stub)          |
| 0x3C   | ApplySetting   | ApplySetting     | ApplySetting    |
| **0x48** | **SetScreenParam** | **SetScreenParam** | **(no-op)** |
| 0x50   | (stub)         | SetGIFParam      | (stub)          |

The M87 via dongle uses `CDev3632::SetScreenParam` (vtable 0x48) which calls
`CDev3632::AccessData` with cmdId=0x0B.

### CDev3632::AccessData (FUN_0040d7e0)

Core packet builder. Parameters:
```
param_1 (this/ECX): data buffer pointer
param_2:            device object (has m_hDev at +4)
param_3:            cmdId
param_4:            board (for meta byte, << 4)
param_5:            data size in bytes
param_6:            retry flag (0xf = retry enabled)
param_7:            inter-packet delay (ms)
```

Splits data into 14-byte chunks, builds packets, writes via `FUN_0040e0f0`
(which calls `WriteFile`), reads response via `FUN_0040d370`.

### CDev3632::SendCMD (FUN_0040da80)

Simpler variant for single-packet commands. Builds packet directly:
```c
packet[0] = 0x13;           // Report ID
packet[1] = cmdId;          // from parameter
packet[2] = 1;              // numPackages = 1
packet[3] = 0;              // packageIndex = 0
// data at [4..17]
packet[19] = CRC;           // computed over [0..18]
```

---

## 8. Communication Flow

### On Device Connect

1. Software detects USB HID device via SetupAPI enumeration
2. Reads `KB.ini` for the detected PID
3. Opens HID device handle
4. Calls `GetPassword` (cmdId 0x05) — 6-byte auth handshake
5. Reads firmware version
6. If `SendTimeOnOpenKB=1`:
   - Calls `GetLocalTime()` (Windows API)
   - Builds SysParam buffer with time + volume
   - Sends via cmdId 0x0B
7. If `SendSysParam > 0`:
   - Spawns `SysParam_Thread`
   - Thread sends SysParam every `SendSysParam` ms
   - Checks `SendOnlyDataChanged` flag to skip unchanged data

### SysParam_Thread Loop

```
loop:
  wait(SendSysParam ms)
  cpu  = GetProcessorUtility()     // via PDH
  vol  = GetMasterVolumeScalar()   // via IAudioEndpointVolume × 100
  mem  = GlobalMemoryStatusEx()    // .dwMemoryLoad
  time = GetLocalTime()

  buffer[0]  = vol   (if mask & 4)
  buffer[1]  = cpu   (if mask & 2)
  buffer[2]  = mem   (if mask & 2)
  buffer[3]  = year_lo  (if mask & 1)
  buffer[4]  = year_hi
  buffer[5]  = month
  buffer[6]  = day
  buffer[7]  = hour
  buffer[8]  = minute
  buffer[9]  = second
  buffer[10] = dayOfWeek

  if (!SendOnlyDataChanged || data_changed):
    device->SetScreenParam(buffer, 14)    // vtable[0x48]
```

### Wireless Check

Before sending SysParam, the thread checks `NoSyncTimeForWireless` (KB.ini).
If set and the device reports wireless (vtable[0x28] returns non-zero),
the SysParam send is skipped. For the M87, `NoSyncTimeForWireless` is not
set in KB.ini, so time sync works over both USB and 2.4 GHz dongle.

---

## 9. Feature Report Protocol (CDevG5KB)

Used by the M87 over USB cable (PID 0x01A2) and some other RK keyboards.
Uses `HidD_SetFeature` / `HidD_GetFeature` (Windows) or
`ioctl(HIDIOCSFEATURE)` / `ioctl(HIDIOCGFEATURE)` (Linux) with 520-byte
(0x208) buffers:

```
Offset  Size  Field         Description
──────  ────  ────────────  ──────────────────────────────────────
  0      1    reportId      Device-specific (0x09 for M87 USB)
  1      1    cmdId         Command identifier (same as CDev3632)
  2      1    board         Board identifier (usually 0)
  3      1    (reserved)    Always 0
  4      1    numPackages   Total packets in transfer
  5      1    packageIndex  0-based index of current packet
  6      1    dataLenLow    Payload size, low byte
  7      1    dataLenHigh   Payload size, high byte
  8-519  512  payload       Command-specific data (zero-padded)
```

### Key Differences from CDev3632

| Aspect         | CDev3632 (dongle)         | CDevG5KB (USB cable)       |
|----------------|---------------------------|----------------------------|
| Transport      | Output reports (write/read) | Feature reports (ioctl)   |
| Report ID      | 0x13                      | 0x09 (device-dependent)    |
| Buffer size    | 20 bytes                  | 520 bytes                  |
| Max payload    | 14 bytes/packet           | 512 bytes/packet           |
| CRC            | Byte 19 checksum          | None (handled by USB/HID)  |
| Response       | Read after write          | SetScreenParam is fire-and-forget |

### SetScreenParam via Feature Report

For SysParam (cmdId 0x0B), the 14-byte payload is placed at offset 8
in the 520-byte buffer. Example for 2026-03-10 14:30:45, volume 65%:

```
Offset: 00 01 02 03 04 05 06 07 08 ...
Data:   09 0B 00 00 01 00 0E 00 41 00 00 EA 07 03 0A 0E 1E 2D 02 00 ...

Where:
  09    = Report ID (M87 USB)
  0B    = cmdId (SetScreenParam)
  00    = board
  00    = reserved
  01    = 1 packet total
  00    = packet index 0
  0E 00 = payload length 14 (little-endian)
  41... = same 14-byte SysParam payload as CDev3632
```

### Linux ioctl

```python
import fcntl

def _IOC(dir, type, nr, size):
    return (dir << 30) | (size << 16) | (type << 8) | nr

HIDIOCSFEATURE = lambda size: _IOC(3, ord('H'), 0x06, size)  # set
HIDIOCGFEATURE = lambda size: _IOC(3, ord('H'), 0x07, size)  # get

buf = bytearray(520)
buf[0] = 0x09   # report ID
buf[1] = 0x0B   # cmdId
# ... fill header and payload ...
fcntl.ioctl(fd, HIDIOCSFEATURE(520), buf)
```

---

## 10. Linux Implementation

### Auto-Detection

The script scans `/sys/class/hidraw/*/device/uevent` for entries matching
VID `258A` with a known PID (`0150` or `01A2`), then selects the
`input1` interface (vendor config channel). The PID determines which
protocol to use.

### Pre-Flight Check (Dongle Mode)

In dongle mode, the keyboard may be powered off while the dongle is
plugged in. The script sends an Echo command (cmdId 0x09) and verifies
the response before syncing. In USB cable mode this is unnecessary —
if the hidraw device exists, the keyboard is connected and powered on.

### Permissions

The hidraw device needs read/write access. Options:
- udev rule: `SUBSYSTEM=="hidraw", ATTRS{idVendor}=="258a", MODE="0666"`
- Run as root
- Add user to appropriate group

### Volume on Linux

```bash
# PipeWire/WirePlumber
wpctl get-volume @DEFAULT_AUDIO_SINK@
# Output: "Volume: 0.45" or "Volume: 0.45 [MUTED]"

# PulseAudio
pactl get-sink-volume @DEFAULT_SINK@
```

---

## 11. Firmware Architecture (3 MCUs)

The M87 keyboard contains **three microcontrollers** communicating over SPI:

```
┌─────────────────┐     SPI     ┌─────────────────┐
│  Keyboard MCU   │────────────▶│ Screen Controller│──▶ LCD
│  (chunk1, 8051  │◀────────────│  (chunk2, 8051)  │   320×172
│   SH68F90A)     │             └─────────────────┘
│                 │
│  Keymatrix,     │     2.4GHz  ┌─────────────────┐
│  Encoder,       │◀───────────▶│  Dongle BLE MCU  │──▶ USB host
│  USB HID        │             │  (chunk0, ARM)   │
└─────────────────┘             └─────────────────┘
```

### Keyboard MCU → Screen Controller SPI Protocol

Two packet types over SPI:

1. **SysParam data**: `[0x50, 0x81, sub_cmd, data_len, vol, cpu, mem, ...]`
   Forwards raw volume/time from HID to screen controller.

2. **Display page commands**: `[0x50, cmd_type, page_id, element, ..., CRC]`
   Triggers specific display elements (volume bar, clock, etc.).

### Volume Display Data Flow

```
PC ──HID cmdId 0x0B──▶ Keyboard MCU ──SPI──▶ Screen Controller ──▶ LCD bar
                        (stores vol)          (stores own copy)

Encoder tick ──────────▶ Keyboard MCU ──SPI──▶ Screen Controller ──▶ LCD bar
                         (0x81/0x82 cmd)       (adjusts own ±2%)
```

- **PC → keyboard**: Volume from SysParam (raw 0-100) forwarded over SPI
- **Encoder → screen**: Direction commands 0x81 (down) / 0x82 (up), screen
  controller adjusts its internal counter by **±2% per tick** (hardcoded)
- The screen controller maintains its **own volume counter** independently
  from the SysParam value — this causes mismatch during scrolling

### Key Keyboard MCU Functions (chunk1, Ghidra)

| Address        | Name (assigned)          | Purpose                                      |
|----------------|--------------------------|----------------------------------------------|
| CODE:006e      | DisplayUpdateDispatch    | Checks flag bits 0F1A/0F1B/0F1C, dispatches  |
| CODE:028f      | DisplayElementRenderer   | Renders volume bar, clock, etc. via SPI       |
| CODE:3905      | HidCmdDispatch           | cmdId jump table (0x03–0x0D) at CODE:39B8     |
| CODE:7276-7390 | EncoderHandler           | Rotary encoder state machine (P1.4, P3.2)     |
| CODE:1c0b      | TimerISR                 | Display update timing, flag management         |
| CODE:93b8      | SysParamTimer            | Clears 0E12 processing flag after ~50ms       |
| CODE:a29b      | SpiPacketBuilder         | Builds [0x50, type, len, data..., CRC]        |
| CODE:b86b      | SysParamSpiForwarder     | Forwards SysParam to screen controller        |

### Key EXTMEM Variables (chunk1)

| Address | Name           | Description                                         |
|---------|----------------|-----------------------------------------------------|
| 0x08F5  | sysParamVol    | Volume byte from HID SysParam (0-100)               |
| 0x0BAB  | encoderState   | Encoder state machine current state                  |
| 0x0CA6  | displayElement | Active display element (0=none, 1=volbar, 16=clock) |
| 0x0CA7  | elementParam   | Sub-element parameter / direction indicator          |
| 0x0E12  | sysParamBusy   | Processing flag (1=block display, 0=allow)           |
| 0x0F1A  | flagsA         | Bit 0=SysParam pending                               |
| 0x0F1B  | flagsB         | Bit 5=element dispatch, Bit 6=volume bar trigger     |
| 0x0F65  | displayMode    | Active display mode (affects encoder behavior)       |

---

## 12. Quirks & Pitfalls Discovered

1. **Sending cmdId 0x0C–0x0F with zeroed payloads resets the keyboard clock**
   to 2000-00-00 00:00:00. These appear to be legacy time-set commands.

2. **Scanning all 256 cmdIds** (0x00–0xFF) can reset the clock and freeze it.
   Power-cycle the keyboard to recover.

3. **Invalid BCD in time bytes** causes garbled display (e.g., non-renderable
   characters in the year field).

4. **CRC is validated** — packets with wrong CRC are silently ignored.

5. **The keyboard has no hardware RTC** — time is lost on power loss and must
   be re-synced from the host.

6. **Profile data (GetProfile 0x44)** does NOT contain time — only byte 47
   stores TimeFormat (12h/24h toggle).

7. **The RK web app** (drive.rkgaming.com) has NO time sync code at all.
   Time sync is exclusive to the Windows desktop software.

---

## 13. Known Issues with Volume Sync (`--daemon` mode)

### EP0 Blocking During SysParam Sends

Each `send_sysparam` call over USB cable (`ioctl(HIDIOCSFEATURE)`) takes
~25ms for the USB control transfer on EP0. However, the keyboard firmware
then spends an additional **~275ms processing** the SysParam packet
(setting 0E12 flag, SPI transfer to screen controller, display dispatch,
timer-based flag clearing). During this ~300ms total, the keyboard's 8051
main loop is busy and **cannot poll the rotary encoder** or send HID
consumer control reports (volume up/down) to the OS.

**Impact**: Sending SysParam on every volume event throttles the encoder
to ~2-3 ticks/second instead of the native ~40 ticks/second. The user
experiences extremely sluggish scrolling.

**Mitigation**: The `--daemon` mode debounces volume changes — it waits
500ms after the last scroll event before sending a single SysParam. This
keeps EP0 free during scrolling and preserves native scroll speed. The
trade-off is that the LCD bar shows the screen controller's internal
±2% tracking during scrolling, then jumps to the correct OS value ~500ms
after scrolling stops.

### Screen Controller's Independent Volume Counter

The screen controller (chunk2) maintains its own volume counter that
increments/decrements by **±2% per encoder tick**. This is independent
of the SysParam volume value sent from the PC. When the OS changes
volume by ±5% per tick, the screen controller's bar and the actual OS
volume diverge during scrolling.

The 2% step is **hardcoded in the screen controller firmware** (chunk2)
and is not configurable via KB.ini or any HID command. The only way to
fix this would be to modify the screen controller firmware.

### SysParam Processing Blocks Display Refresh

When a SysParam packet arrives (cmdId 0x0B), the keyboard firmware sets
`EXTMEM 0x0E12 = 1`, which blocks the timer ISR (CODE:1c0b) from
triggering volume bar display updates. The `FUN_CODE_93b8` timer clears
0E12 after ~50ms. During this window, encoder-triggered bar updates are
suppressed, so even if the correct volume is sent, the LCD may not
refresh until the bar auto-hides and the user scrolls again.

### Theoretical Fixes (Not Implemented)

- **Firmware patch**: Modify chunk2 to use ±5% step (or read step from
  SysParam). Requires reflashing the screen controller MCU.
- **cmdId 0x0D trick**: After sending 0x0B, send cmdId 0x0D which sets
  `0CA6=1` (volume bar element), potentially forcing a display refresh.
  Untested — may cause other side effects.
- **Async sends**: Use a background thread for `ioctl(HIDIOCSFEATURE)`
  so the event loop isn't blocked. However, the firmware processing
  delay (~275ms) still blocks the encoder regardless of threading.

---

## 14. UPF Firmware Format

The MCU firmware is stored as `.upf` files in the extracted installer
(`rk_extracted/app/Dev/<PID>/`). The firmware updater is `Update.exe`.

### Header (128 bytes, offset 0x00–0x7F)

```
Offset  Size  Endian  Field          Description
──────  ────  ──────  ────────────── ──────────────────────────────────────
  0      2    —       magic          Always 0x5A 0xA5
  2      7    —       vendor         "BLESINO" (Sinowealth identifier)
  9      1    —       fwType         Firmware type (0–3, see below)
  10     7    ASCII   model          e.g. "KB0000\0"
  17     2    BE      vid            USB Vendor ID (e.g. 0x258A)
  19     2    BE      pid            USB Product ID (e.g. 0x01AF)
  21     4    —       (unknown)      TBD
  25     1    —       (unknown)      TBD
  26     6    ASCII   version1       Version string component
  32     6    ASCII   version2       Version string component
  38     4    BE      size1          Chunk 0 size in bytes
  42     4    BE      size2          Chunk 1 size in bytes
  46     8    —       (fields)       Format/checksum fields
  54     6    ASCII   version3       Version string component
  60     4    BE      size3          Chunk 2 size in bytes (type 3 only)
  64     4    —       (fields)       Format/checksum fields
  68-85  —    —       (padding)      Unused
  86     1    —       (flag)         Unknown flag at offset 0x56
  87-127 —    —       (padding)      Zero-filled
```

### Firmware Types

| fwType | Chunks | Typical Use                              |
|--------|--------|------------------------------------------|
| 0      | 2      | Two firmware images (size1 + size2)      |
| 1      | 1      | Single image (size1 only)                |
| 2      | 1      | Single image (size2 only)                |
| 3      | 3      | Three images (size1 + size2 + size3)     |

### Payload Layout (offset 0x80+)

```
Offset 0x80:                     Chunk 0 (size1 bytes)
Offset 0x80 + size1:             Chunk 1 (size2 bytes)
Offset 0x80 + size1 + size2:     Chunk 2 (size3 bytes, type 3 only)
After all chunks:                TEA key (4 bytes)
```

File size = 0x80 + sum(chunk sizes) + 4

### M87 Firmware (PID 0x01AF, fwType=3)

The M87 `.upf` file contains three chunks:

| Chunk | Size     | Architecture | Content                           |
|-------|----------|--------------|-----------------------------------|
| 0     | 70,640 B | ARM Cortex-M | BLE/wireless dongle MCU firmware  |
| 1     | 61,440 B | 8051         | Keyboard MCU main firmware (SH68F90A) |
| 2     | 61,440 B | 8051         | Screen controller MCU firmware    |

- Chunk 0 starts with an ARM vector table (initial SP, reset handler)
- Chunks 1 & 2 start with `LJMP` (opcode `0x02`) — the 8051 reset vector

### Encryption: Modified TEA

All firmware chunks are encrypted with a modified TEA (Tiny Encryption Algorithm):

| Parameter      | Value                                               |
|----------------|-----------------------------------------------------|
| Algorithm      | TEA (Feistel cipher, 64-bit blocks)                 |
| Rounds         | 32                                                  |
| Delta          | `0x9E3769B9` (non-standard; standard is `0x9E3779B9`) |
| Key size       | 4 bytes (NOT the standard 128-bit key)              |
| Key source     | Last 4 bytes of the `.upf` file, after all chunks   |
| Block endian   | Big-endian uint32 pairs                             |

**Key weakness**: Each of the 4 key bytes is zero-extended to a 32-bit TEA
key word, giving only 32 bits of effective key strength instead of 128.

Decryption pseudocode:

```python
DELTA = 0x9E3769B9
MASK = 0xFFFFFFFF

def tea_decrypt(v0, v1, k0, k1, k2, k3):
    s = (DELTA * 32) & MASK  # = 0xC6ED3720
    for _ in range(32):
        v1 = (v1 - (((v0 >> 5) + k3) ^ (s + v0) ^ ((v0 << 4) + k2))) & MASK
        v0 = (v0 - (((v1 >> 5) + k1) ^ (s + v1) ^ ((v1 << 4) + k0))) & MASK
        s = (s - DELTA) & MASK
    return v0, v1
```

Data is processed in 8-byte (two uint32) blocks, read/written as big-endian.

### Decryption Tool

```bash
python3 decrypt_upf.py [path/to/firmware.upf]
```

Outputs decrypted chunks as `*_chunk0.bin`, `*_chunk1.bin`, `*_chunk2.bin`.

### Reverse Engineering Source

The encryption algorithm was traced in `Update.exe` (Ghidra):

| Address    | Function           | Purpose                             |
|------------|--------------------|-------------------------------------|
| 0x0040a940 | UPF parser         | Reads header, extracts chunks & key |
| 0x004044b0 | Decrypt wrapper    | Byte-swaps, calls TEA per 8B block  |
| 0x004045a0 | TEA decrypt core   | 32-round modified TEA decryption    |

---

## 15. Scripts

| File                          | Purpose                           |
|-------------------------------|-----------------------------------|
| `scripts/rk-m87-sync.py`     | Legacy Python sync script         |
| `scripts/decrypt_upf.py`     | UPF firmware decryptor (modified TEA) |

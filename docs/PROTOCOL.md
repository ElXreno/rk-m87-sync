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

**Important:** the dongle (CDev3632) and wired (CDevG5KB) transports use **different
cmdId numbers for the same operation**. Both tables below were derived from
`DeviceDriver.exe` decompilation in May 2026 (Ghidra project `rk-m87-smk`).

### Dongle Mode (CDev3632, Report ID 0x13, 20-byte output reports)

| cmdId | Name             | Dir | Description                                      |
|-------|------------------|-----|--------------------------------------------------|
| 0x01  | SetMatrix        | W   | Per-key matrix / layer data (multi-packet)       |
| 0x02  | SetGame          | W   | Per-key RGB color group, `nColorPerKey * 3` bytes |
| 0x03  | SetMacro         | W   | Macro data, 512-byte chunks (multi-packet)       |
| 0x04  | SetLED           | W   | 128-byte LED config                              |
| 0x06  | ResetDevice      | W   | Empty payload (was previously documented as 0x11) |
| 0x07  | GetDongleStatus  | R   | 1 byte: 0 = wireless connected                   |
| 0x09  | SetLedRgbTab     | W   | RGB LED table                                    |
| 0x0B  | **SetScreenParam** | W | **SysParam: time, volume, CPU, memory (see §5)** |
| 0x0C–0x0F | ⚠ dangerous | W   | Reset/freeze the LCD clock — DO NOT scan        |
| 0x12  | SendWeb          | W   | Web/cloud data                                   |
| 0x44  | GetLED           | R   | Read 128-byte LED config                         |
| 0x4A  | GetVersion       | R   | Read 2-byte firmware version (lo, hi)            |
| 0x88  | SetRealData      | W   | Runtime status (CDev3632 only; gated by `m_state[0x238]==0`) |

### Wired Mode (CDevG5KB, Feature Report id=m_reportId, 520-byte feature reports)

| cmdId | Name             | Dir | Description                                      |
|-------|------------------|-----|--------------------------------------------------|
| 0x03  | SetMatrix        | W   | Note: dongle uses 0x01 for the same op           |
| 0x04  | SetLED           | W   | Same as dongle                                   |
| 0x05  | SetMacro         | W   | Note: dongle uses 0x03                           |
| 0x06  | SetGame          | W   | Note: dongle uses 0x02                           |
| 0x0A  | SetLedRgbTab     | W   | Note: dongle uses 0x09                           |
| 0x0B  | SetScreenParam   | W   | Same as dongle                                   |
| 0x0C  | **AccessData_Page** | W | **LCD image data upload, frame-indexed, per-page CRC** |
| 0x0D  | **SetGIFParam**  | W   | **LCD GIF playback config (5-byte payload — see §8)** |
| 0x12  | SendWeb          | W   | Same as dongle                                   |
| 0x82  | GetPassword      | R   | Read auth bytes                                  |
| 0x84  | GetLED           | R   | Note: dongle uses 0x44                           |

### Historical mistake

Previous versions of this document labeled cmdIds 0x01/0x02/0x09 as "Echo".
That was wrong — they are real config commands (SetMatrix/SetGame/SetLedRgbTab).
The daemon's `echo_ping()` was switched to `get_dongle_status_ping()` (cmdId
0x07) in May 2026 after a live dump confirmed the previous 3-byte payload
`[0x0E, 0xDE, 0xAD]` was being persisted into the LED color-table at flash
offset `0xA800` every session.

### GET cmdIds (high bit set for read direction)

Verified from `DeviceDriver.exe` byte-level RE (May 2026):

| cmdId | Signed | Name          | Returns                                |
|-------|--------|---------------|----------------------------------------|
| 0x82  | -0x7E  | GetPassword   | 6 bytes — pairing identifier ("psd")  |
| 0x83  | -0x7D  | GetMatrix     | per-key matrix data                    |
| 0x84  | -0x7C  | GetLED        | 128 bytes — LED config                 |
| 0x85  | -0x7B  | GetRgb        | LED RGB table data                     |
| 0x87  | -0x79  | GetPower      | 2 bytes — battery + charging status    |
| 0x88  | -0x78  | GetRealData   | live RGB streaming readback            |
| 0x8A  | -0x76  | (unknown)     | TBD                                    |

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

### Named DeviceDriver.exe entry points (May 2026 RE)

| Address | Function | Role |
|---------|----------|------|
| 0x00410230 | `CDevG5KB_SetScreenParam` | wired path for SysParam (cmd 0x0B) |
| 0x0040DE90 | `CDev3632_SetScreenParam` | dongle path for SysParam (cmd 0x0B) |
| 0x004128A0 | `CDevG5KB_SetGIFParam` | LCD GIF param (cmd 0x0D) |
| 0x0040E570 | `CDev3632_SetRealData` | live RGB streaming (cmd 0x88) |
| 0x00410230 | `CDevG5KB_GetLED` | read 128 B LED config (cmd 0x84) |
| 0x0040FD10 | `CDevG5KB_GetPower` | read battery + charging (cmd 0x87) |
| 0x004103E0 | `CDevG5KB_GetPassword` | read 6 B psd (cmd 0x82) |
| 0x00410045 | `CDevG5KB_GetRealData` | read live RGB (cmd 0x88) |
| 0x00410360 | `SendWeb_dispatch` | cmd 0x12 SendWeb |
| 0x004101B0 | `SetLedRgbTab_dispatch` | wired cmd 0x0A / dongle cmd 0x09 |
| 0x0040F9C0 | `SetMatrix_dispatch` | wired cmd 0x03 / dongle cmd 0x01 |
| 0x0040DD60 | `CDev3632_SetMacro` | dongle cmd 0x03 SetMacro |
| 0x0040D6A0 | `CDev3632_FindDevice_OpenHandle` | enumerate dongle by VID/PID |
| 0x0040EE90 | `CDevG5KB_InitialDriver` | wired init |
| 0x0040F350 | `CDevG5KB_FindHIDDevice` | wired enumerate |
| 0x0040F030 | `FindScreenDevice_ByPsd` | locate screen MCU via psd match (cmd 0x82) |
| 0x004168B0 | `GetFirmwareVersion_3632` | read FW version |
| 0x00411BA0 | `CDevG5KB_ApplySetting` | apply matrix/macro/LED/sysparam in sequence |
| 0x0040E690 | `CDev3632_ApplySetting` | dongle ApplySetting |
| 0x00411700 | `FillCfg_LowDaly` | build 128 B SetLED payload (low debounce/tap) |
| 0x00411900 | `FillCfg_HighDaly` | build 128 B SetLED payload (high range) |
| 0x00448100 | `LoadProfile_rkf` | load .rkf profile |
| 0x00448530 | `SaveProfile_rkf` | save .rkf profile |
| 0x004566E0 | `SysParam_Thread` | periodic 14 B SysParam sender |
| 0x0045B090 | `GetSystemVolume_IAudioEndpoint` | Windows volume via COM |
| 0x0045AFD0 | `GetCPUUsage_PDH` | PDH-based CPU usage |
| 0x004533D0 | `LoadCfgIni` | parse cfg.ini OPT keys |

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

## 9b. LCD Image Upload (wired only)

The LCD GIF/image upload uses two cmdIds in sequence over the wired CDevG5KB
feature-report protocol. The dongle path has **no LCD image support** —
dongle vtable[0x50] (SetGIFParam slot) is a stub. Image uploads must go
over USB cable.

### Dual-HID architecture

The CDevG5KB device object has two HID handles:
- `m_hDev1` (`+0x04`) — main keyboard interface (reportId read from HID descriptor)
- `m_hDev2` (`+0x08`) — **screen daughterboard interface** (reportId = `0x09` hardcoded)

The screen daughterboard appears to Linux as a **separate hidraw device**
on the same composite USB device. For image upload, `m_hDev2` is preferred.
The main daemon currently only opens `m_hDev1` — to upload images we'd need
to enumerate and open both.

### cmdId 0x0D — SetGIFParam

5-byte payload, sent once per upload session to declare what GIF is coming:

```
Offset  Field             Description
──────  ────────────────  ──────────────────────────────────────
  0     idxBits           (isScreen2 << 6) | (gifIndex & 0x3F)
  1-2   nFrameNum (LE)    Total frame count in this GIF (16-bit)
  3-4   nGIFDelay (LE)    Per-frame delay in ms (16-bit)
```

After SetGIFParam, the app awaits a confirmation response. If `m_hDev2`
is also present, the same SetGIFParam is **re-sent to the screen handle**
with reportId 0x09.

### cmdId 0x0C — AccessData_Page

Bulk image data, **one page (≤512 bytes) per packet**, frame-indexed,
with a per-page CRC that the daughterboard validates and echoes back:

```
Offset  Field             Description
──────  ────────────────  ──────────────────────────────────────
  0     reportId          0x09 if sending to screen handle (m_hDev2)
  1     cmdId             0x0C
  2     board             0
  3     frameIdx          Which frame of the GIF this page belongs to
  4     pageIdx           Cumulative page counter across all frames
  5     CRC               (-1 - sum(payload[0..len-1])) & 0xFF
  6-7   dataLen (LE)      Payload length, max 0x200 = 512
  8-519 payload           Frame pixel data (up to 512 bytes)
```

CRC enabled by global `DAT_00611844` in the Windows app — when `0`, the
field is left as 0 and the daughterboard skips the check.

### Response / retry loop

After each page, the daughterboard ACKs via an async HID read with the
following meaningful bytes in the response:
- byte `+3`: validity flag, **must be non-zero**, else "respond unmatch"
- byte `+0` (in resp): echoes `pageIdx`
- byte `+1` (in resp): echoes CRC

If the CRC does not match, the app logs:
```
CDevG5KB::AccessData_Page frame=%d, page=%d, CRC(out)=%x, CRC(in)=%x
```
and retries that page. After all pages are acknowledged, the GIF buffer
on the daughterboard is loaded; the previously sent `SetGIFParam`
parameters drive playback.

### End-to-end flow

```
1. User picks image in CBurnScreenDlg (DeviceDriver.exe vtable[0x4C])
2. App computes frame indices (FUN_00466330 interpolates if user wants
   more frames than the source has)
3. SetGIFParam(gifIndex, nFrameNum, nGIFDelay) → cmdId 0x0D
4. For each frame:
     For each page (frame_bytes / 512):
       AccessData_Page(frameIdx, pageIdx, payload) → cmdId 0x0C
       wait ACK + verify CRC
       retry on mismatch
5. Daughterboard renders the GIF on the LCD per (nFrameNum, nGIFDelay)
```

This is the path that ultimately drives the chunk-2 → daughterboard
`[0x5A, cmd, len, sub, CRC]` 5-byte abstract-command protocol — **chunk-1**
receives `0x0C`-cmdId pages over HID, then forwards them to chunk-2 over
SPI which in turn translates to the daughterboard's internal protocol.

---

## 10. Linux Implementation

### Auto-Detection

The script scans `/sys/class/hidraw/*/device/uevent` for entries matching
VID `258A` with a known PID (`0150` or `01A2`), then selects the
`input1` interface (vendor config channel). The PID determines which
protocol to use.

### Pre-Flight Check (Dongle Mode)

In dongle mode, the keyboard may be powered off while the dongle is
plugged in. The daemon sends cmdId `0x07` (`GetDongleStatus`) with an empty
payload and accepts any non-error response as proof of life. This is a
genuine read with no side effects.

The earlier daemon implementation sent cmdId `0x09` with a 3-byte payload
`[0x0E, 0xDE, 0xAD]`. That is actually `SetLedRgbTab`, so each session
wrote those three bytes into the keyboard's LED color-table at flash
offset `0xA800` (confirmed by live dump May 2026). The current code uses
`GetDongleStatus` and avoids any persistent state change.

In USB cable mode the probe is unnecessary — if the hidraw device exists,
the keyboard is connected and powered on.

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

## 11. Firmware Architecture (3 MCUs — all SH68F90A 8051)

The M87 ecosystem contains **three SH68F90A 8051 microcontrollers**. Verified
2026-05-16 by live dump of the dongle via `sinowealth-kb-tool`, which
confirmed the dongle is an 8051 SH68F90A (not the Cortex-M0 / "SH68F1000J"
suggested by its silkscreen).

```
┌─────────────────┐     SPI     ┌─────────────────┐
│  Keyboard MCU   │────────────▶│ Screen Controller│──▶ LCD daughterboard
│  (chunk 1,      │◀────────────│  (chunk 2,      │    320×172
│   SH68F90A 8051)│             │   SH68F90A 8051)│
│                 │             └─────────────────┘
│  Keymatrix,     │     2.4 GHz ┌─────────────────┐
│  Encoder,       │◀───────────▶│  Dongle MCU     │──▶ USB host
│  USB HID        │             │  (chunk 0,      │
└─────────────────┘             │   SH68F90A 8051)│
                                 └─────────────────┘
```

The 2.4 GHz radio is **integrated** in each SH68F90A via the CCP
(capture/compare) module + dedicated TX/RX SFRs (`TXSTAT`, `TXDAT`, `TXCON`,
`TXFLG`, `TXCNTL/H`, `RXSTAT/DAT/CON/FLG`, `WCON`); no external transceiver.
The LCD daughterboard is a **separate physical chip** (not yet dumped) that
chunk 2 drives over SPI using a `[0x5A, cmd, len, sub, CRC]` 5-byte protocol.

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

## 11b. Firmware Update Protocol (Update.exe)

Royal Kludge ships `Update.exe` (a Sinowealth 8805 USB update tool, build
`20231113 V1.22 SPEC0.6`) to flash new firmware. The protocol is HID-feature-
report based and uses two interfaces simultaneously.

### Two HID interfaces

Update.exe enumerates two HID interfaces by feature-report size:

| Interface | Feature size | Purpose |
|---|---|---|
| `m_hControl` | 6 bytes | Small control commands |
| `m_hBulk` | 2050 bytes (or 1032 for BLE) | Bulk data write/read |

### Three modes for three chunks

The UPF file (`.upf`) packs 1–3 firmware chunks. The `chunk_mode` byte in
the header selects which chunks are present:

| mode | chunks | applied to |
|---|---|---|
| 0 | BLE + HID | dongle MCU + keyboard MCU |
| 1 | BLE only | dongle MCU |
| 2 | HID only | keyboard MCU (chunk 1) |
| 3 | BLE + HID + MFU | all three (chunk 0 + 1 + 2) |

- **HID mode** → chunk 1 (keyboard SH68F90A)
- **MFU mode** → chunk 2 (screen SH68F90A)
- **BLE mode** → chunk 0 (dongle SH68F90A; named "BLE" historically but
  the dongle uses 2.4 GHz proprietary radio, not Bluetooth Low Energy)
- HID and MFU use the **identical wire protocol**; only the target chunk differs.

### Control-channel commands (reportId 0x05, 6-byte feature reports)

Byte 1 is the cmd opcode — an **ASCII mnemonic** for the action:

| Cmd | Char | Operation | Bytes 2–5 |
|---|---|---|---|
| `0x45` | 'E' | Enter ISP mode (effectively: **Erase ALL** user firmware — pages 0x00..0x3B; writes `[02, 00, 00]` at addr 0 + `[F0, 00, 01]` at addr 1 as safety. Verified via byte-level RE of `isp_set_report_handler @ dongle:0xF48C` 2026-05-16) | `0x45 0x45 0x45 0x45` (filler magic) |
| `0x52` | 'R' | Setup read window | `addr_lo addr_hi len_lo len_hi` |
| `0x57` | 'W' | Setup write window / begin transfer | `addr_lo addr_hi len_lo len_hi` |
| `0x65` | 'e' | Erase page at addr | `addr_lo addr_hi 0 0` (200 ms sleep follows) |
| `0x72` | 'r' | Read 4-byte data | (no args; issued after 'R' setup) |
| `0x77` | 'w' | Write 4-byte data inline | `b0 b1 b2 b3` |
| `0x55` | 'U' | Upgrade finalize | `0x55 0x55 0x55 0x55` (filler magic) |
| `0x5A` | 'Z' | Reset / reboot device | `0 0 0 0` |

`HidD_GetFeature` on the control HID returns 6 bytes; byte 1 is the status:
- `0x00` — success
- `0x55` ('U') — busy, retry
- `0xCB` (= `-0x35`) — handshake success
- `0xFF` — error / fatal

### Bulk-data packet (reportId 0x06, 2050 bytes for HID/MFU)

```
byte 0:    0x06           (report ID)
byte 1:    0x77 or 0x72   (write or read)
bytes 2..: 2048 bytes of payload
```

### Bootloader entry (chunk 1)

The application firmware must transition into ISP/bootloader mode before
any of the control commands above are accepted. The chunk-1 firmware
recognises a Feature Report 5 SET_REPORT carrying `[0x05, 0x75]` as the
trigger. `process_isp_state_machine @ chunk1:0x8907` requires
`EXTMEM:0x1100 == 0x05` and `EXTMEM:0x1101 == 0x75 ('u')`, then calls
`enter_isp_full @ 0xB0ED`. From Linux:
`ioctl(fd, HIDIOCSFEATURE(N), [0x05, 0x75, ...])`.

Once in the bootloader, `isp_set_report_handler @ dongle_bootloader:0xF48C`
processes the ASCII commands listed above. Sending `'E'` (`[0x05, 0x45*5]`)
**erases the entire user firmware** (pages 0x00..0x3B, 60 pages) and is
the first step of an upgrade — not a benign "enter ISP" handshake.

### BLE upgrade entry

Update.exe uses a different entry sequence for the dongle/BLE upgrade
mode (`EnterBootloader_via_HID @ Update.exe:0x00409760`):

```
1. HidD_SetFeature: [0x05, 0xC9, 0x60, ...]
2. Poll HidD_GetFeature for [0x60, status, ...] response
3. status byte:
     0x00  → success, enter upgrade thread
     0x55  → busy, retry
     0xFF  → fatal error
```

This matches the firmware path in `chunk2_handle_set_report` (state 9
+ `1101 == 0xC9` → `dispatch_p_or_backtick`). The 0xC9/0x60 handshake is
the canonical "BLE upgrade mode" entry.

### HID / MFU upgrade flow

```
1. ENTER BOOTLOADER: SET_REPORT [0x05, 0x75, ...] (or 0xC9 0x60 for BLE);
                     wait for ACK
2. ERASE ALL:        control [0x05, 0x45, 0x45, 0x45, 0x45, 0x45]; sleep 2000 ms
3. BEGIN WRITE:      control [0x05, 0x57, 0x00, 0x00, len_lo, len_hi]
4. WRITE LOOP:       for page i = 0..(chunk_size/2048):
                       bulk [0x06, 0x77, page_data[2048]]   ; first page has data[0]=0
5. END WRITE:        control [0x05, 0x57, 0x00, 0x00, len_lo, len_hi]  (re-sent)
6. BEGIN VERIFY:     control [0x05, 0x52, 0x00, 0x00, len_lo, len_hi]
7. VERIFY LOOP:      for page i = 0..(chunk_size/2048):
                       HidD_GetFeature(bulk_handle, buf, 2050)
                       compare buf[2..2049] vs page_data[i]
8. (HID only) PRESERVE 0xFF80 IDENTITY: read 4 bytes, erase page, write back
   with PID/VID overlay.
9. FINALIZE:         control [0x05, 0x55, 0x55, 0x55, 0x55, 0x55]
10. RESET:           control [0x05, 0x5A, 0x00, 0x00, 0x00, 0x00]
```

### BLE upgrade flow

Uses 1032-byte feature reports with cmd `0xC9/0x05/0x02` for version
exchange. The chunk-write command is `0xCB`:

```
data channel (Report 0x06, 1032 bytes):
  [06, CB, sub, page_num, ~page_num, data(1024B), CRC_lo, CRC_hi]
  subcmd 0x02 = full 1024-byte chunk
  subcmd 0x01 = ≤128-byte final chunk
  CRC = CRC-16-CCITT (poly 0x1021, init 0x0000) over the firmware data
```

Status polling uses Report 0x05 (6 bytes); byte 0 = `0xCB` (or `'p'`=0x70
for finalize), byte 1 = `0x00` success / `0x55` busy / `0xFF` error.

### TX* SFRs are multiplexed (Sinowealth quirk)

The same `TXSTAT/TXDAT/TXCON/TXFLG/TXCNTH/CCAP1H/CCAP2H` SFRs that drive
the 2.4 GHz radio in the user firmware are repurposed by the bootloader
as **flash erase/program controllers**. `isp_erase_page @ 0xFB9E` and
`isp_write_byte_to_flash @ 0xFB6B` route through these registers. This
is why a single chip can both transmit radio packets and self-flash
without dedicated programming pins.

### UPF file format

```
offset  size  field            BE/LE  notes
──────  ────  ──────────────   ─────  ──────────────────────
  0      1   marker1                  0x5A ('Z')
  1      1   marker2                  0xA5
  2      7   magic                    "BLESINO"
  9      1   chunk_mode               0/1/2/3 (see table)
 10      7   version1                 display
 17      2   PID              BE      keyboard USB PID
 19      2   VID              BE      keyboard USB VID
 21      2   BLE_PID          BE      dongle USB PID
 23      2   BLE_VID          BE      dongle USB VID
 25      1   flag                     unknown
 26      6   version2                 display
 32      6   blockData
 38      6   version3                 display
 44      4   BLE_chunk_size   BE      chunk 0 length
 48      4   HID_chunk_size   BE      chunk 1 length
 52      4   ver_fw1          mixed
 56      4   ver_fw2          mixed
 60      6   version4                 display
 66      4   MFU_chunk_size   BE      chunk 2 length (only if mode==3)
 ...
 0x80    N   ciphertext               concatenated TEA-encrypted chunks
 EOF-4   4   key                      4-byte TEA key
```

**Encryption:** modified TEA with delta `0x9E3769B9` (note: not the standard
`0x9E3779B9`), 32 rounds, 8-byte blocks treated as big-endian uint32 pairs.
Key is 4 bytes at the end of the file; each byte is zero-extended to a uint32
to form the 16-byte expanded key. M87 key: `04 a3 ce 69`.

A working decryptor is at `scripts/decrypt_upf.py`.

### Implications for Linux

To replicate the update flow from Linux:
1. Enumerate both HID interfaces (control = 6-byte feature, bulk = 2050-byte
   feature).
2. Run the sequence above via `ioctl(HIDIOCSFEATURE)` / `HIDIOCGFEATURE`.
3. After the `0x5A` reset packet, the keyboard reboots into the new firmware.

The same ISP server that responds to these commands is also reachable via
the `LJMP 0xFF00` boot-key recovery shim — so a software-only brick recovery
should be feasible (no external programmer needed, provided the bootloader
isn't corrupted).

The bootloader entry at `0xFF00` (`isp_bootloader_entry_5a_a5`) requires
the magic byte pair `[0x5A, 0xA5]` in the register pair — the same magic
as the UPF file header marker.

---

## 11c. Chunk-2 Direct LCD Control (undocumented)

Chunk 2 (screen MCU) has its **own USB SET_REPORT handler**
(`chunk2_handle_set_report @ 0x1249`, dispatched from
`chunk2_usb_packet_state_machine @ 0x0D1D`) that mirrors chunk 1's. The
USB SETUP packet dispatcher table at `chunk2:CODE:0x15C7` is byte-for-byte
identical to chunk 1's at `chunk1:CODE:0xA2C2`.

For `SET_REPORT(Feature, Report 5, iface 0)` with payload starting
`[0x05, 0xC9, byte2, byte3]`, the firmware calls
`dispatch_p_or_backtick(byte3, byte2)`:

- `byte2 == 'p' (0x70)` → `EXTMEM:0x42 = 4` → mode-4 LCD refresh
- `byte2 == '\`' (0x60)` → `EXTMEM:0x42 = 0x80` → encoder UP packet
  `[0x5A, 0x80, 5, 0, CRC]` sent to the daughterboard

This is a **direct LCD-control path that bypasses chunk 1** entirely
and is useful for debugging the daughterboard. The same `[0x05, 0x75]`
sequence on chunk 2 triggers `halt_baddata_1f9c` (chunk-2's
"transition to bootloader" — disables IRQ then halts to let the
bootloader take over).

---

## 11d. .rkf Profile File Format

DeviceDriver.exe saves keyboard profiles to disk as `.rkf` files via
`SaveProfile_rkf @ 0x00448530` / `LoadProfile_rkf @ 0x00448100`. The format
is fixed-size, 24,332 bytes (`0x5F0C`):

```
offset  size   field            notes
──────  ────   ──────────────   ────────────────────────────────
0x00    0x40   header           model name, magic, etc.
0x40    2      PID              keyboard USB PID (e.g. 0x01AF)
0x44    4      magic            0xFCFB999A (little-endian)
0x48    8      reserved
0x50    N      key info data    4 layers × 132 keys × 16 bytes
0x21D8  N      macro buffers    10 macro slots × 834 bytes
```

The PID at offset `0x40` enforces per-model compatibility — loading a
profile saved on a different keyboard model triggers a "wrong profile
data" warning. Each key info entry is 16 bytes encoding the key type,
HID code, and any macro/layer reference.

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

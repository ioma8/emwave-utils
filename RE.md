# RE: HeartMath emWave2 USB protocol

Reverse-engineering log. Target: HeartMath emWave2 biofeedback device (USB HID).

## 2026-08-19 — Part 1: Device recon (validated)

### Device Identity & USB Enumeration
- **Product Name**: `HeartMath emWave 2`
- **VID**: `0x0E30` (3632), **PID**: `0x0008` (8), `bcdDevice`: `0x0120` (v1.20)
- **USB Spec**: USB 2.0 Full Speed (12 Mbps), 1 Configuration (41 bytes)
- **Interface**: Interface 0, Class 3 (HID), Subclass 0, Protocol 0
- **Endpoints**:
  - `EP 0x81` (Interrupt IN): 64 bytes max packet, 1 ms polling interval
  - `EP 0x01` (Interrupt OUT): 64 bytes max packet, 1 ms polling interval

### Low-Level Access & DriverKit Notes (validated)
- On macOS, DriverKit (`com.apple.AppleUserHIDDrivers`) claims exclusive kernel ownership of
  `AppleUserUSBHostHIDDevice`. User-space raw USB requests (`libusb`, `pyusb`, control transfers)
  return `[Errno 13] Access denied (insufficient permissions)` — **even under `sudo`** (tested).
- `hidutil` CAN see the device; it carries private entitlements
  (`com.apple.private.hid.client.service-protected`, `com.apple.hid.heartrate-access`) — explains
  why unsigned CLI tools enumerate zero HID devices on macOS 26.
- The official app carries `com.apple.security.device.usb` + `com.apple.security.device.serial`
  entitlements (validated via `codesign`).
- **Firmware dump over raw USB from user space: infeasible** — interface is HID-only and locked by
  the system driver. Firmware access exists only through the HID protocol itself (see flash
  commands below).

### HID Report Descriptor (328 bytes, validated — read from IOKit registry)
- **Usage Page**: `0xFF00` (Vendor Defined), **Usage**: `0x00FF`
- **bcdHID**: 1.11, country 0

| Report ID | ASCII | Type | Payload | Purpose (validated/assumed) |
|---|---|---|---|---|
| `0x75` | `'u'` | Input | 62 B | Sensor stream (IBI/HRV) |
| `0x70` | `'p'` | Input | 62 B | Raw PPG pulse stream |
| `0x66` | `'f'` | Input | 62 B | Flash/session data block |
| `0x65` | `'e'` | Input | 62 B | Event log |
| `0x46` | `'F'` | Input | 62 B | Firmware/flash dump block |
| `0x53` | `'S'` | Output | 62 B | **Host command channel (text protocol)** |
| `0x77` | `'w'` | Feature | 63 B | Write config/flash |
| `0x74` | `'t'` | Feature | 8 B | Time sync |
| `0x72` | `'r'` | Feature | 2 B | **Session control flags (validated)** |
| `0x31` | `'1'` | Feature | 12 B | Device info/version query |
| `0x62` | `'b'` | Feature | 8 B | Bootloader |
| `0x58` | `'X'` | Feature | 1 B | Exit/stop |
| `0x50` | `'P'` | Feature | 1 B | Power |
| `0x45` | `'E'` | Feature | 5 B | Erase flash |
| `0x04` | — | Feature | 62 B | Bootloader data |
| `0x49` | `'I'` | Feature | 37 B | Device ID block |

## 2026-08-19 — Part 2: Official app RE (in progress)

### Target
- `/Applications/emWave Pro.app/Contents/MacOS/emWaveMac`
- Qt 6.3 app, universal binary (x86_64 + arm64), ~32 MB, **not stripped** (898 EmSensor symbols).
- Links IOKit directly; embeds a C++ sensor SDK (`EmSensorSpace` namespace, source tree
  `heartmath/emwaveqt`, `sdk/emDS/sources/emWv2Interface.cpp`).
- `kuna` 1.130 **cannot load** these Mach-O slices ("unsupported/!recognized binary") →
  fallback to `radare2` (documented per kuna skill rules).

### EmSensor SDK API surface (validated — exported symbols)
`EmSensorSpace::emWv2Interface` (protocol layer):
- `OpenSensor()` / `CloseSensor()` / `StartDevice()` / `StopDevice()`
- `StartSession(int challenge)` — starts a recording session
- `GetRealIBI()` / `SetRealIBI(int)` / `GetCurIBI()` / `GetCurPPG()` / `GetCurHrt()` / `GetCurScore()`
- `IBI_parser(SessionSetableData*, std::string const&)` — parses IBI text records
- `genericRecordParser(...)` / `dx_parser(...)` / `sessionResumeHandler(...)`
- `handleSerialData(std::string&)` / `handleSerialData(std::string&, std::string&, SessionSetableData*)`
- `extractDataFromReport(unsigned int)` — input report dispatch
- `sendSunplusCommand(char const*, int)` — **text command writer** (the 'S' output channel)
- `sendPlusCmd(char*)`
- `readRawFlash(CCoreEMWV2_FLASH_REGION)` / `writeRawFlash(region, char*)` / `formatFlash(region)`
- `getDeviceInfo(char*, long*)` / `setDeviceInfo(char*, long*)`
- `getStoredSessions()` / `getParsedSessions(void*, TagCCoreEMWV2INFO**)` / `deleteStoredSessions`
- `SetACKReceived(bool)` / `SetNAKReceived(bool)` / `SetSessionTime(int)` / `GetSessionTime()`
- `Xio(int, char*)` — direct I/O escape

`EmSensorSpace::CDrvM801` (device driver, chip "M801"):
- `OpenSensor` / `OpenSensorEx` / `CloseSensor` / `PauseDriver` / `ResumeDriver`
- `SetStatusLight(r,g,b)` / `GetDataAvailable()` / `extractDataFromReport(unsigned)`
- `agcInitialize` / `agcFinalize` / `agcPowerLevelSet` / `agcUpdatePowerLevel` (PPG AGC)

`EmSensorSpace::USBFuncImpl` (HID transport):
- `OpenImpl(vid, pid, ...)` / `CloseImpl` / `ReadSyncImpl` / `WriteSyncImpl`
- `GetFeatureImpl(void*, void*, uint)` / `SetFeatureImpl(void*, void*, uint)`
- `GetVidPidImpl`

### Protocol strings (validated — hex-dumped from __TEXT/__const)
- `"J+\r"` / `"J-\r"` — session begin / session end markers
- `"SR\r"` — start recording
- `"PD\r"` — (pause/pulse data)
- `"SB\r"` — (session blob start?)
- `"<CO ACK>"` / `"<CO NAK>"` — coherence protocol ACK/NAK
- `"challenge: %s %d\n"` — debug (StartSession logs challenge string)
- Parser regexes:
  - `[T][=]([0-9]{2})[:]([0-9]{2})[:]([0-9]{2}) ([0-9]{2})[-]([0-9]{2})[-]([0-9]{2})` → `T=HH:MM:SS DD-MM-YY`
  - `([A-z]+)[^=]?[=][ ]?([0-9A-FTft]+)[ ]` → generic `key=value` pairs
  - `[<][ ]?[I][ ]([0-9]+)[^>]+[>]` → **IBI records `<I <ms> ...>`**
  - `([<]S[rE][^>]+[>])` → stored session records (`<Sr...>` / `<SE...>`)
  - `[<]PU.*FW=[']([^']+)[']` → **pulse records `<PU ... FW='...'>`** (FW = firmware tag)

### StartSession flow (validated — r2 pdc of `emWv2Interface::StartSession(int)` @ 0x1003db460)
1. Logs `"challenge: %s %d\n"`.
2. Validates device handle (`Helper_isValidHandle`).
3. Feature report `'r'` (2 bytes): `GetFeature` → set byte[1] = `(byte[1] & 0x80) | 0x01` →
   `SetFeature` (session flag bit 0).
4. `sendSunplusCommand("J-\r")` — end previous session if any.
5. `sendSunplusCommand("SR\r")` — start recording.
6. Re-read feature `'r'` (ACK loop).

## 2026-08-19 — Part 3: Binary protocol decode (validated, r2 `pdc`/`pd`)

### 'S' output report — exact framing (validated @ 0x1003dbc1c)
```
byte[0]   = 0x53 'S'            report ID
byte[1..2]= 0x00 0x00
byte[3]   = strlen(command)      length byte
byte[4..] = command text          zero-padded to 63 bytes
```
Written via `WriteSync(handle, buf, 63)` → interrupt OUT EP 0x01 (64-byte packet incl. ID).
Commands observed in code: `"J+\r"`, `"J-\r"`, `"SR\r"`, `"PD\r"`, `"SB\r"`.
`sendPlusCmd()` returns `"<CO ACK>"` / `"<CO NAK>"` strings.

### Input reports — framing (validated @ 0x1003da9d0 dispatch)
```
byte[0]   = report ID ('u' 0x75, 'p' 0x70, 'e' 0x65, 'f' 0x66, 'F' 0x46, 'S' 0x53)
byte[1..2]= 16-bit sequence counter (validated live: increments per report)
byte[3]   = payload length
byte[4..] = payload
```
- 'F' flash: `[3..6]` = 32-bit LE address, `[7]` = length, `[8..]` = data →
  `STORED_SESSION_BLOB::handleFlashData(addr, len, data)` reassembly.
- 'u' → text records (IBI/status), 'p' → command echo (validated live).

### Feature reports (sizes validated live)
| ID | Size | Content (live sample) |
|---|---|---|
| `'r'` 0x72 | 2 B | `72 81` — byte1 flags: bit0 = command-mode, bit7 sticky |
| `'1'` 0x31 | 13 B | flash layout: `31 c2 bd 39 00 ff ff 3e 00 ff ff 3f 00` → region bounds `0x0039BDC2`, `0x003EFFFF`, `0x003FFFFF` (4 MB flash) |
| `'I'` 0x49 | 38 B | `49 <16B UUID> <"Mar 07 2013 21:31:22"> 00` — device UUID + manufacture timestamp |
| `'t'` 0x74 | 9 B | RTC block `74 10 00 08 01 19 05 22 04` |
| `'b'`/`'w'`/`0x04` | — | **kIOReturnUnsupported (0xE0005000)** — bootloader/write features absent in device FW |

### Session control (validated)
- `StartSession`: set 'r' bit0 → `"J-\r"` → `"SR\r"` → device replies `"CO ACK"`.
- `StopDevice`: `"PD\r"` only (no feature writes) + close.
- `getStoredSessions`: `"SB\r"` → device ACKs (`"CO ACK"`), streams `'F'` blocks if sessions exist.
- `formatFlash`: only region 2; feature '1' (13 B) + 'r' involved.
- `readRawFlash`/`writeRawFlash`: **disabled stubs** (return 0x6c 'l') in the Mac build.

### Live device traffic (validated 2026-08-19, no sensor attached)
- `'p'` report = **command echo** (e.g. "J-\r", "SR\r", "SB\r" echoed back).
- `'u'` report payload records:
  - **IBI**: `<2 I=984 R=F H=61 >` — I=IBI ms, R=artifact flag F/T, H=heart rate bpm
  - **Status**: `<1 L=1 T=25 S=0 A=0 E=14 >` — L=signal level, T=temperature, S/A/E counters
  - `"CO ACK"` / `"CO NAK"` protocol acks
- Empty 'u' payloads stream continuously at ~1 report/s when idle.

### Firmware dump verdict (evidence)
- Mac app's `readRawFlash`/`writeRawFlash` are compile-time stubs (error 0x6c).
- Bootloader features `'b'`, `'w'`, `0x04` return kIOReturnUnsupported from this unit's FW.
- Flash size = 4 MB (`0x003FFFFF`), layout bounds exposed via feature '1'.
- **Conclusion: no USB-level firmware read exists in the current firmware; dump requires
  bootloader entry (unsupported) or physical SPI/JTAG access.** POC driver includes an
  experimental probe (`firmware` subcommand) for future FW revisions.

## 2026-08-19 — Part 4: Python POC driver

File: `emwave2.py` (hidapi via ctypes, no pip deps; VID/PID 0x0E30/0x0008).
- `info` — device UUID + manufacture date (feature 'I') + RTC (feature 't') + flash layout (feature '1')
- `ibi --seconds N` — start session, parse `<2 I=.. R=.. H=..>` IBI records + `<1 ...>` status
- `ppg --seconds N` — capture 'p' reports (echo channel; PPG samples pending sensor)
- `events --seconds N` — dump 'e'/'f'/'S' reports
- `sessions --seconds N [--out f]` — `SB\r` + 'F' block reassembly (device currently empty)
- `firmware [--out f]` — feature probe + 'F' listener (experimental)
- `raw --seconds N` — raw report dump for validation

HID access note: works when the terminal process holds Input Monitoring (macOS 26) or via a
sandboxed helper with `com.apple.security.device.usb`.

### Live validation with sensor attached (2026-08-19)
- Full IBI stream confirmed: `RAW IBI= 708ms`, `IBI= 697ms artifact=no HR=86 bpm` —
  coherent physiology (IBI ~700ms ↔ HR ~85 bpm). Records end with `\r` and may split
  across reports; POC driver reassembles via line buffering.
- Records seen live: `<SESSIONSTART>`, `<I <ms>>` (raw IBI series), `<2 I=.. R=.. H=..>`
  (qualified IBI + HR), `<1 L=.. T=.. S=.. A=.. E=..>` (status), `CO ACK`.
- **PPG verdict**: the emWave2 USB protocol does NOT stream raw pulse samples — the device
  computes IBI on-board. Command set is complete (`J-`, `J+`, `SR`, `PD`, `SB`); there is
  no pulse-stream command. Raw pulse (`<PU ... FW='...'>`) only exists inside stored
  session blobs (downloadable via `SB\r` when the device holds sessions).
- Session storage: device ACKs `SB\r` but currently holds no sessions (0 'F' blocks).

### Final status
All features implemented in `emwave2.py` and verified live: device info, IBI/HR stream,
status stream, command echo, session-blob protocol, flash layout. PPG live-streaming is
not supported by the device hardware protocol — documented, not a driver limitation.

## 2026-08-19 — Part 5: Session download (validated live)

### Discovery: the app's own protocol log
`~/Documents/logs/YYYY-MM-DD--HH-MM-SS.dat` — the app logs ALL device output during sync.
2026-08-19's log (3.78 MB) reveals the full device data model:
- **Chip: Sunplus SPCE061A**, boot banner `COXon SPCE061A Firmware: GeneralPlus 1.0a.r297
  CO (c) 2002-2011 Quantum Intech, Inc.` — streamed per device boot.
- Device E2PROM params (parsed from `CO <key> = <vals>` lines):
  `PCHALL PVOLUME PBRITE = 1 1 1`, `PDEFGUI PPH0LEN PHRVDIV = 2 25 16`,
  `PCUTBATmV PWARNBATmV PPUPMODE = 3440 3600 1`, `PHRVP2..6`, `PIBI PHRVPACE PBEATGEN =
  733 565 0`, `UPTIME = 383135`, `GUID = <eml v=1 >`.
- Session record groups (per stored session):
  `<PU T='..' BS=0 BF=0 P=B FW='00.78 Mar 07 2013 21:31:22' />` (pulse record + FW),
  `<BS=1 />`/`<BF=1 />` (block markers), `<Sr T='..' />` (session record),
  `<Ss T='..' />` (session stop), `<SESSIONSTART>`, `<I n>`, `<2 I=.. R=.. H=..>`,
  `<1 L=.. T=.. S=.. A=.. E=..>` — each followed by a ~110-byte binary tail (per-session
  payload, encoding TBD).
- emWave.emdb (SQLite, ~/Documents/emWave/) holds 95 sessions back to 2010.

### Live capture of our own SB download (validated)
Sequence that produced data: open → feature 'I' (38B) → feature 't' (9B) → `"SB\r"`:
- `'F'` (0x46) reports: binary blob blocks; first block = blob header
  (`00 00 00 01 ba 00 00 ...`), 55-byte payload.
- `'f'` (0x66) reports: text session records — `<Sr T='14:02:27 19-08-26' />`,
  `<Ss T='14:02:40 19-08-26' />`, `CO ACK`.
- `'p'` reports: command echo ("SB").
- Feature `'r'` bit0 (command-mode flag) and the `'P'` feature interaction change device
  behavior; exact trigger semantics not yet fully mapped (one successful burst observed,
  subsequent identical attempts returned ACK-only — under investigation).

## 2026-08-19 — Part 6: Live HR/HRV/coherence TUI

File: `emwave_tui.py` (curses + numpy, no other deps).
- Live HR (from `<2 I=.. H=..>` records), last IBI, beat count, artifacts.
- Scrolling IBI trend graph (60-ms-scaled rows, terminal-width columns).
- HRV (rolling window, default 240 beats): SDNN, RMSSD, mean RR, mean HR.
- Coherence estimate: NN series interpolated to 4 Hz, Hanning-window FFT, peak power in
  0.04-0.26 Hz band / total HRV power (VLF+LF+HF) -> 0-100% + LOW/MEDIUM/HIGH.
- `--dump N` non-curses metric mode (validated live: HR 72-100 bpm, SDNN ~40 ms,
  RMSSD ~35 ms, coherence 37-74% tracking rhythm changes).

### Next: hook dylib (`hook_hid.m` -> `hook_hid.dylib`, launcher `run_hooked.sh`)
App entitlements (`allow-dyld-environment-variables`, `disable-library-validation`)
permit DYLD_INSERT_LIBRARIES. The hook logs every IOHIDDeviceSetReport/GetReport/
RegisterInputReportCallback/Open/Close to `/tmp/hid_hook.log`. Running the app's
File→Sync under the hook reveals the app's EXACT transfer sequence for replication
in `emwave2.py`.

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

Historical note: the former Python curses TUI and temporary HID-hook tooling were
removed after the protocol was mapped and the Rust `training/` app became canonical.
The protocol evidence remains in this log; current desktop/Android behavior lives in
`training/src/`.

## 2026-08-19 — Part 7: Android APK port

- Shared egui UI/metrics moved into `training/src/lib.rs`; desktop keeps a thin binary wrapper.
- Desktop transport remains `hidapi`.
- Android transport uses Android `UsbManager` via JNI:
  - enumerates VID `0x0E30` / PID `0x0008`;
  - requests Android USB host permission with a `PendingIntent`;
  - opens `UsbDeviceConnection`, duplicates its file descriptor;
  - uses `nusb` for interface 0 HID interrupt IN `0x81`, interrupt OUT `0x01`, and HID
    class GET/SET_FEATURE control transfers.
- Manifest declares `android.hardware.usb.host`; Android activity is portrait/fullscreen by
  default, with a dedicated narrow-screen layout so the pacer, HR/IBI cards, resonance,
  graph, and HRV stats stack without horizontal clipping.
- Android uses eframe `glow` backend. The first wgpu APK was tested on Android 16 emulator
  and crashed in the emulator's Vulkan driver; the OpenGL/glow APK launches successfully.
- APK build:
  `cargo apk build --release --no-default-features --lib --target aarch64-linux-android`
  with the local debug keystore supplied through `CARGO_APK_RELEASE_KEYSTORE`.
- Verified on Android 16 Pixel emulator:
  - APK installs;
  - NativeActivity launches;
  - process remains alive;
  - fullscreen portrait UI renders without horizontal overflow;
  - emulator has no USB device attached, so physical emWave permission/transfer could not be
    exercised there.
## 2026-08-19 — Part 8: Persistent Android diagnostics

- Added a persistent reader log for Android and desktop.
- Android path:
  `/sdcard/Android/data/com.emwave.resonance/files/emwave.log`
- Logged events:
  - reader thread start;
  - USB/HID open attempts;
  - successful session start;
  - HID read errors;
  - USB permission/open/interface errors;
  - reconnect attempts.
- Retrieve after reconnecting the phone to the computer:
  `adb pull /sdcard/Android/data/com.emwave.resonance/files/emwave.log`

### Physical Pixel 9 log diagnosis
- Android permission dialog succeeded.
- Persistent log then showed repeated `interface is busy (errno 16)`.
- Root cause: normal `nusb::claim_interface` loses to Android's system HID driver.
- Android backend changed to `detach_and_claim_interface(0)`; desktop transport unchanged.

## 2026-08-19 — Part 9: Rolling graph and session history

- The Rust graph uses a bounded last-120-beat buffer; it cannot grow indefinitely. HRV
  analysis retains up to 240 clean beats.
- Added persistent `sessions.json`:
  - desktop: `~/.emwave-resonance/sessions.json`;
  - Android: `/sdcard/Android/data/com.emwave.resonance/files/sessions.json`.
- Each app session records start date/time, duration, mean HR, mean resonance score,
  clean beats, and artifact count.
- Added Train / Sessions navigation and a previous-session browser.
- Added dedicated portrait stacking so the session view and trainer fit narrow screens.

### Session persistence correction
- Sessions are finalized both when the HID reader reports a device disconnect and when
  the app exits.
- Records are deduplicated by session start timestamp, so disconnect + shutdown cannot
  create duplicate history rows.
- The graph remains a bounded 120-sample view while the HRV analysis window remains
  240 clean beats.

### Session/display behavior
- Session history now finalizes on HID disconnect as well as application shutdown;
  records are deduplicated by start timestamp.
- Android uses `FLAG_KEEP_SCREEN_ON` while the training activity is running and clears
  it when the activity exits.

### Session-history Android crash diagnosis
- A physical Pixel 9 crash showed `JNIEnv::call_method` receiving a null object in
  Android USB enumeration when no emWave2 was attached.
- Added null checks for Activity, UsbManager, device map, collection, iterator, device,
  and `openDevice` connection.
- Rebuilt and installed the null-safe APK; process stays alive with the phone connected
  to ADB and the emWave2 absent.

### Personal resonant-rate finder
- Added a `FIND RATE` screen with one-minute paced-breathing trials at 4.5, 5.0,
  5.5, 6.0, 6.5, and 7.0 breaths/min.
- The finder shows the animated breath pacer and offers either one selected trial or
  one automatic six-minute sequence covering every rate.
- Each result uses the completed trial's latest 60-second resonance estimate, ranks
  completed rates by score, and reports the dominant peak.
- Fixed session history loss: USB open errors now preserve the loaded session list, and
  disconnect persistence updates the snapshot before it is published.

### Target-frequency finder correction
- Replaced the six one-minute trials with sequential 4.5, 5.5, and 7.0
  breaths/min trials lasting two minutes each.
- Trial scoring now projects the NN series at the exact paced frequency, locates the
  strongest LF peak on an oversampled frequency grid, and penalizes peak mismatch.
- A result is called a match only when the LF peak is within 0.5 bpm of the paced rate
  and the target-frequency score is at least 35/100; otherwise the UI reports no match.

### Analysis audit
- Trial and live spectrum preprocessing now removes linear NN drift before the Hann
  taper; subtracting only the mean could leak slow trend energy into the LF peak.
- Synthetic tests confirm a target-frequency oscillation is accepted and a mismatched
  target is rejected.
- The two-minute trials remain exploratory; they are not interchangeable with a
  standards-grade five-minute LF/HF measurement.

### Beat-analysis audit
- Verified parser framing, artifact flag handling, cumulative physiological timing,
  NN-only HRV inputs, sample-variance SDNN, successive-difference RMSSD, and pNN50.
- Session mean HR now excludes artifact-marked beats; the displayed RSA span is explicitly
  named as an NN interval range in milliseconds.
- Added regression tests for the time-domain formulas and artifact-excluded mean HR.

### Full session beat archives
- Each saved session now stores every received beat with physiological timestamp,
  IBI, heart rate, and artifact flag.
- Session history exposes `VIEW GRAPH`; clean NN intervals render in green and
  artifact-marked intervals in red.
- Older summary-only sessions remain readable and show `NO ARCHIVE`.
- Finder rows now expose target-power share and LF-normalized power, making `NO MATCH`
  diagnosable instead of only showing a final score.

### Raw-session replay finding
- The latest archive contained only 126.3 wall-clock seconds and 233 clean beats;
  the log shows the emWave2 disconnected at 126 seconds, before a three-trial run
  could complete.
- Its physiological NN timeline spans about 183 seconds, indicating the device
  delivered buffered samples faster than wall time.
- Independent replay found the dominant LF peak around 2.8–2.9 bpm; target power was
  negligible at 4.5 and 7.0 bpm and about 2% of the dominant peak at 5.5 bpm.
- Therefore `NO MATCH` is correct for that archive; it does not contain evidence of
  target-rate entrainment.
- Finder now aborts an active sequence on USB disconnect instead of scoring stale or
  incomplete data.

### Receipt-time trial segmentation
- Archived samples now include both physiological IBI time and wall-clock receipt time.
- Finder trial boundaries use receipt time, preventing buffered HID reports from extending a
  nominal two-minute trial to a different physiological span.
- USB disconnect now aborts the active trial and prevents stale data from being scored.

### Trial response score semantics
- Trial score now measures response power at the tested rate independently of peak
  mismatch.
- `MATCH` is a separate classification requiring a dominant LF peak within the
  tolerance; an off-target trial retains its nonzero target-response score.
- Added a synthetic mixed-signal regression test proving an off-target dominant peak
  does not erase a nonzero target score.

### Partial trial scoring
- Trials with at least 60 seconds of receipt-time data now retain a numerical target
  response score even when the two-minute run is interrupted.
- Such results are labeled `PARTIAL` and cannot become the reliable best match.

### Latest no-data replay
- The newest session contained 247 archived samples, 240 clean beats, 6 artifacts,
  and only 130.9 seconds of receipt time.
- The USB log shows the device disconnecting at 132 seconds, so the previous
  `NO DATA` result came from the 108-second complete-trial gate after a late trial
  start, not from an empty beat stream.
- Partial trials with at least 60 seconds now retain a score and are labeled `PARTIAL`;
  only at least 108 seconds can produce a reliable `MATCH`.

### Two-session calculation validation
- Independent replay of the two newest archives matched the Rust spectral algorithm:
  both sessions peak at approximately 2.8–3.0 bpm, with LF normalized power above 96%.
- Neither archive contains a target-rate peak at 4.5, 5.5, or 7.0 bpm; the `NO MATCH`
  classification is supported by the data.
- Found and fixed one summary mismatch: session mean HR averaged the device `H=` field
  while HRV used IBI intervals. Mean HR now derives from clean `60000 / IBI`.

### Short-sample scoring
- Partial trial scoring now accepts at least 30 seconds of receipt-time data, returns
  the target-frequency response score, and labels the row `PARTIAL`.
- Reliable `MATCH` still requires at least 108 seconds, preserving the distinction
  between an exploratory short estimate and a complete trial.

### Research-aligned finder protocol
- Finder now runs the adult assessment range 6.5, 6.0, 5.5, 5.0, 4.5 breaths/min.
- Automatic runs insert two-minute natural-breathing rests between two-minute trials.
- Results are labeled cardiac PPG candidates; the app does not claim phase-verified
  resonance without a synchronized respiration channel.
- Complete trials still require at least 108 seconds of data for reliable matching.

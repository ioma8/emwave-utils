# emWave Utils

Tools for the HeartMath emWave2 USB device, including protocol utilities and a Rust desktop/Android resonance trainer.

## Components

- `emwave2.py` — macOS HID protocol utility for inspection, IBI capture, sessions, and raw reports.
- `capture.py` — passive HID traffic logger.
- `src/` — Rust trainer shared by desktop and Android.
- `RE.md` — reverse-engineering notes and protocol evidence.
- `HRV_COHERENCE_RESEARCH.md` — HRV and resonance-method references.

## Build

```sh
cargo test
cargo run --bin emwave-trainer
cargo apk build --release --no-default-features --lib --target aarch64-linux-android
```

The Android build requires the Android SDK/NDK and a release keystore. The emWave2 device uses USB HID; connect it directly and grant Android USB permission when prompted.

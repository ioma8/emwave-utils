//! HID transport for the HeartMath emWave2 (VID 0x0E30, PID 0x0008).
//!
//! Protocol (decoded in ../RE.md): the device is a vendor HID device. Host
//! commands go out on the 63-byte output report `'S'` (`[0]=0x53, [3]=len,
//! [4..]=command text`), sensor data comes back on input reports (`[0]=report
//! id`, `[1..2]=seq`, `[3]=len`, `[4..]=payload`).

use hidapi::{HidApi, HidDevice};

pub const REPORT_LEN: usize = 63;
const VID: u16 = 0x0e30;
const PID: u16 = 0x0008;

pub struct Device {
    _api: HidApi, // must outlive the device handle
    dev: HidDevice,
}

impl Device {
    /// Open the emWave2 and put it into a live recording session.
    pub fn open_and_start() -> Result<Device, String> {
        let api = HidApi::new().map_err(|e| e.to_string())?;
        let dev = api
            .open(VID, PID)
            .map_err(|_| "emWave2 not found (plug it in; check HID access)".to_string())?;
        let d = Device { _api: api, dev };
        d.session_flag(true)?;
        d.send_command(b"J-\r")?;
        d.send_command(b"SR\r")?;
        Ok(d)
    }

    fn get_feature(&self, report_id: u8, size: usize) -> Result<Vec<u8>, String> {
        let mut buf = vec![0u8; size + 1];
        buf[0] = report_id;
        let n = self
            .dev
            .get_feature_report(&mut buf)
            .map_err(|e| e.to_string())?;
        Ok(buf[..n].to_vec())
    }

    fn set_feature(&self, report_id: u8, payload: &[u8]) -> Result<(), String> {
        let mut buf = vec![report_id];
        buf.extend_from_slice(payload);
        self.dev
            .send_feature_report(&buf)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Feature report 'r': byte[1] bit0 = command-mode / session flag.
    fn session_flag(&self, on: bool) -> Result<(), String> {
        let cur = self.get_feature(0x72, 2)?;
        let b = cur.get(1).copied().unwrap_or(0);
        let flags = (b & 0x80) | if on { 1 } else { 0 };
        self.set_feature(0x72, &[flags])
    }

    fn send_command(&self, cmd: &[u8]) -> Result<(), String> {
        let mut buf = [0u8; REPORT_LEN];
        buf[0] = 0x53;
        buf[3] = cmd.len() as u8;
        buf[4..4 + cmd.len()].copy_from_slice(cmd);
        let n = self.dev.write(&buf).map_err(|e| e.to_string())?;
        if n < REPORT_LEN {
            return Err("short write".to_string());
        }
        Ok(())
    }

    /// Read one input report. `Ok(None)` = timeout (no report available).
    pub fn read_report(&self, timeout_ms: i32) -> Result<Option<Vec<u8>>, String> {
        let mut buf = [0u8; 64];
        match self.dev.read_timeout(&mut buf, timeout_ms) {
            Ok(n) if n > 0 => Ok(Some(buf[..n].to_vec())),
            Ok(_) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
}

pub const REPORT_LEN: usize = 63;

pub trait HidTransport {
    fn get_feature(&self, report_id: u8, size: usize) -> Result<Vec<u8>, String>;
    fn set_feature(&self, report_id: u8, payload: &[u8]) -> Result<(), String>;
    fn write_report(&mut self, report: &[u8; REPORT_LEN]) -> Result<(), String>;

    fn start_session(&mut self) -> Result<(), String> {
        let current = self.get_feature(0x72, 2)?;
        let preserved = current.get(1).copied().unwrap_or(0);
        self.set_feature(0x72, &[preserved | 1])?;
        self.write_report(&command_report(b"J-\r"))?;
        self.write_report(&command_report(b"SR\r"))
    }
}

pub fn command_report(command: &[u8]) -> [u8; REPORT_LEN] {
    let mut report = [0u8; REPORT_LEN];
    report[0] = b'S';
    report[3] = command.len() as u8;
    report[4..4 + command.len()].copy_from_slice(command);
    report
}

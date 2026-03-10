use chrono::{Datelike, Local, Timelike};

pub const VID: u16 = 0x258A;
pub const CMD_ECHO: u8 = 0x09;
pub const CMD_SYSPARAM: u8 = 0x0B;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Dongle,
    UsbCable,
}

impl Protocol {
    pub fn report_id(self) -> u8 {
        match self {
            Protocol::Dongle => 0x13,
            Protocol::UsbCable => 0x09,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Protocol::Dongle => "2.4 GHz dongle",
            Protocol::UsbCable => "USB cable",
        }
    }
}

/// Known USB PIDs and their protocols.
pub const KNOWN_DEVICES: &[(u16, Protocol)] = &[
    (0x0150, Protocol::Dongle),
    (0x01A2, Protocol::UsbCable),
];

/// Build the 14-byte SysParam payload (shared between both protocols).
pub fn build_sysparam_payload(volume: u8, now: &chrono::DateTime<Local>) -> [u8; 14] {
    let year = now.year() as u16;
    // chrono: Mon=0..Sun=6 → protocol: Sun=0..Sat=6
    let dow = (now.weekday().num_days_from_sunday()) as u8;

    let mut payload = [0u8; 14];
    payload[0] = volume;
    // [1] cpu, [2] mem — unused for M87 (SysParamMask bit 1 = 0)
    payload[3] = (year & 0xFF) as u8;
    payload[4] = (year >> 8) as u8;
    payload[5] = now.month() as u8;
    payload[6] = now.day() as u8;
    payload[7] = now.hour() as u8;
    payload[8] = now.minute() as u8;
    payload[9] = now.second() as u8;
    payload[10] = dow;
    payload
}

/// Build a 20-byte CDev3632 output report packet.
pub fn build_output_report(report_id: u8, cmd_id: u8, payload: &[u8]) -> [u8; 20] {
    let mut pkt = [0u8; 20];
    pkt[0] = report_id;
    pkt[1] = cmd_id;
    pkt[2] = 0x01; // numPackages
    pkt[3] = 0x00; // packageIndex
    pkt[4] = (payload.len() as u8) & 0x0F; // meta: (board=0 << 4) | dataLen
    let n = payload.len().min(14);
    pkt[5..5 + n].copy_from_slice(&payload[..n]);
    pkt[19] = crc(&pkt);
    pkt
}

/// Build a 520-byte CDevG5KB feature report buffer.
pub fn build_feature_report(report_id: u8, cmd_id: u8, payload: &[u8]) -> [u8; 520] {
    let mut buf = [0u8; 520];
    buf[0] = report_id;
    buf[1] = cmd_id;
    buf[2] = 0x00; // board
    buf[3] = 0x00; // reserved
    buf[4] = 0x01; // numPackages
    buf[5] = 0x00; // packageIndex
    let len = payload.len();
    buf[6] = (len & 0xFF) as u8;
    buf[7] = ((len >> 8) & 0xFF) as u8;
    let n = len.min(512);
    buf[8..8 + n].copy_from_slice(&payload[..n]);
    buf
}

/// CRC: sum of bytes 0..19 masked to 8 bits.
pub fn crc(packet: &[u8; 20]) -> u8 {
    packet[..19].iter().map(|&b| b as u16).sum::<u16>() as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// Verify against PROTOCOL.md example:
    /// 2026-03-10 14:30:45 (Tuesday), volume 65%
    #[test]
    fn test_sysparam_payload() {
        // Build expected payload manually
        let expected: [u8; 14] = [
            0x41, // volume 65
            0x00, // cpu
            0x00, // mem
            0xEA, // year low (2026 = 0x07EA)
            0x07, // year high
            0x03, // March
            0x0A, // 10th
            0x0E, // 14 hours
            0x1E, // 30 minutes
            0x2D, // 45 seconds
            0x02, // Tuesday (0=Sun)
            0x00, 0x00, 0x00,
        ];

        let now = chrono::Local
            .with_ymd_and_hms(2026, 3, 10, 14, 30, 45)
            .unwrap();
        let payload = build_sysparam_payload(65, &now);
        assert_eq!(payload, expected);
    }

    #[test]
    fn test_output_report_crc() {
        let payload = [0x41u8, 0, 0, 0xEA, 0x07, 0x03, 0x0A, 0x0E, 0x1E, 0x2D, 0x02, 0, 0, 0];
        let pkt = build_output_report(0x13, CMD_SYSPARAM, &payload);

        assert_eq!(pkt[0], 0x13);
        assert_eq!(pkt[1], CMD_SYSPARAM);
        assert_eq!(pkt[2], 0x01);
        assert_eq!(pkt[3], 0x00);
        assert_eq!(pkt[4], 0x0E); // len=14
        assert_eq!(&pkt[5..19], &payload);
        // CRC = sum of bytes 0..18
        let expected_crc: u8 = pkt[..19].iter().map(|&b| b as u16).sum::<u16>() as u8;
        assert_eq!(pkt[19], expected_crc);
    }

    #[test]
    fn test_feature_report_header() {
        let payload = [0x41u8, 0, 0, 0xEA, 0x07, 0x03, 0x0A, 0x0E, 0x1E, 0x2D, 0x02, 0, 0, 0];
        let buf = build_feature_report(0x09, CMD_SYSPARAM, &payload);

        assert_eq!(buf[0], 0x09);
        assert_eq!(buf[1], CMD_SYSPARAM);
        assert_eq!(buf[2], 0x00); // board
        assert_eq!(buf[3], 0x00); // reserved
        assert_eq!(buf[4], 0x01); // numPackages
        assert_eq!(buf[5], 0x00); // packageIndex
        assert_eq!(buf[6], 0x0E); // len low
        assert_eq!(buf[7], 0x00); // len high
        assert_eq!(&buf[8..22], &payload);
        // Rest should be zero
        assert!(buf[22..].iter().all(|&b| b == 0));
    }

    #[test]
    fn test_echo_ping_packet() {
        let payload = [0x0E, 0xDE, 0xAD];
        let pkt = build_output_report(0x13, CMD_ECHO, &payload);

        assert_eq!(pkt[0], 0x13);
        assert_eq!(pkt[1], CMD_ECHO);
        assert_eq!(pkt[4], 0x03); // meta: len=3
        assert_eq!(pkt[5], 0x0E);
        assert_eq!(pkt[6], 0xDE);
        assert_eq!(pkt[7], 0xAD);
    }
}

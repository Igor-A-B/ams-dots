//! The phone's BLE protocol (receiver side): service layout, the motion packet
//! decoder and the control command encoder.
//!
//! The authority is `../AmsDotsMobileController/docs/protocol.md`; the tests
//! decode `protocol/vectors.txt`, the golden file the Kotlin codec also passes.
//! All numbers on the wire are little-endian.

use std::fmt;

/// GATT service the phone advertises. The laptop scans for it (the phone has no name).
pub const SERVICE_UUID: &str = "5be6e39c-9095-468d-a372-5871e621b58b";
/// Notify characteristic that carries motion packets.
pub const MOTION_UUID: &str = "5af2dba6-9864-4524-812f-60c35dcb3ff0";
/// Write characteristic that takes control commands.
pub const CONTROL_UUID: &str = "95978bed-ce1d-404c-932b-25cab3db5b2e";

/// Version 1 has no speed; version 2 (what the phone sends) adds the GPS speed and its age to the header.
const VERSION_1: u8 = 1;
const VERSION_2: u8 = 2;
const HEADER_SIZE_V1: usize = 13;
const HEADER_SIZE_V2: usize = 17;
/// Wire value of the speed field when the phone has no GPS speed.
const NO_SPEED: u16 = 0xFFFF;
const SAMPLE_SIZE: usize = 12;
const MAX_SAMPLES: usize = 4;

/// Lowest and highest sample rate (Hz) the phone accepts.
pub const MIN_RATE_HZ: u8 = 10;
pub const MAX_RATE_HZ: u8 = 100;

/// One sensor reading in the phone's own axes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawSample {
    /// Linear acceleration (gravity removed), mm/s².
    pub accel_mm_s2: [i16; 3],
    /// Angular velocity, mrad/s.
    pub gyro_mrad_s: [i16; 3],
}

/// The phone's newest GPS speed fix, as sent in the header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpeedFix {
    /// Speed over ground, cm/s.
    pub cm_s: u16,
    /// The packet's first sample time minus the fix time, ms. Negative: the fix is newer than the first sample.
    pub age_ms: i16,
}

impl SpeedFix {
    /// Speed over ground, m/s.
    pub fn speed(&self) -> f32 {
        f32::from(self.cm_s) / 100.0
    }
}

/// One notification from the phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    /// Counter that wraps at 65536; gaps mean lost packets.
    pub sequence: u16,
    /// Time of the first sample, ms since the phone's sensors started.
    pub first_sample_ms: u32,
    /// Gravity in the phone's axes, in 1/1000 of 9.81 m/s². Points up, away from the ground.
    pub gravity_mg: [i16; 3],
    /// 1 to 4 samples, oldest first.
    pub samples: Vec<RawSample>,
    /// The phone's newest GPS speed; `None` without one (no GPS, a stale fix, or a version 1 packet).
    pub speed: Option<SpeedFix>,
}

/// Why a packet was rejected; matches the `error:` names in `vectors.txt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketError {
    TooShort,
    BadVersion,
    BadLength,
    BadCount,
}

impl fmt::Display for PacketError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            PacketError::TooShort => "packet shorter than its header",
            PacketError::BadVersion => "unsupported protocol version",
            PacketError::BadLength => "sample bytes are not a multiple of 12",
            PacketError::BadCount => "a packet must hold 1 to 4 samples",
        })
    }
}

fn i16_at(bytes: &[u8], at: usize) -> i16 {
    i16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn vec3_at(bytes: &[u8], at: usize) -> [i16; 3] {
    [i16_at(bytes, at), i16_at(bytes, at + 2), i16_at(bytes, at + 4)]
}

/// Decodes one notification (version 1 or 2).
pub fn decode(bytes: &[u8]) -> Result<Packet, PacketError> {
    let header_size = match bytes.first() {
        None => return Err(PacketError::TooShort),
        Some(&VERSION_1) => HEADER_SIZE_V1,
        Some(&VERSION_2) => HEADER_SIZE_V2,
        Some(_) => return Err(PacketError::BadVersion),
    };
    if bytes.len() < header_size {
        return Err(PacketError::TooShort);
    }
    let sample_bytes = bytes.len() - header_size;
    if sample_bytes % SAMPLE_SIZE != 0 {
        return Err(PacketError::BadLength);
    }
    let count = sample_bytes / SAMPLE_SIZE;
    if !(1..=MAX_SAMPLES).contains(&count) {
        return Err(PacketError::BadCount);
    }
    let samples = (0..count)
        .map(|i| {
            let at = header_size + i * SAMPLE_SIZE;
            RawSample { accel_mm_s2: vec3_at(bytes, at), gyro_mrad_s: vec3_at(bytes, at + 6) }
        })
        .collect();
    let speed = (bytes[0] == VERSION_2)
        .then(|| (u16::from_le_bytes([bytes[13], bytes[14]]), i16_at(bytes, 15)))
        .filter(|(cm_s, _)| *cm_s != NO_SPEED)
        .map(|(cm_s, age_ms)| SpeedFix { cm_s, age_ms });
    Ok(Packet {
        sequence: u16::from_le_bytes([bytes[1], bytes[2]]),
        first_sample_ms: u32::from_le_bytes([bytes[3], bytes[4], bytes[5], bytes[6]]),
        gravity_mg: vec3_at(bytes, 7),
        samples,
        speed,
    })
}

/// The bytes that ask the phone to sample at `hz` (clamped to 10-100).
pub fn set_rate_command(hz: u8) -> [u8; 2] {
    [0x01, hz.clamp(MIN_RATE_HZ, MAX_RATE_HZ)]
}

#[cfg(test)]
mod tests {
    use super::*;

    const VECTORS: &str = include_str!("../../AmsDotsMobileController/protocol/vectors.txt");

    struct Block {
        kind: String,
        name: String,
        fields: Vec<(String, String)>,
    }

    impl Block {
        fn one(&self, key: &str) -> &str {
            &self.fields.iter().find(|(k, _)| k == key).unwrap_or_else(|| panic!("{}: no {key}", self.name)).1
        }
        fn all(&self, key: &str) -> Vec<&str> {
            self.fields.iter().filter(|(k, _)| k == key).map(|(_, v)| v.as_str()).collect()
        }
    }

    fn blocks() -> Vec<Block> {
        let normalized = VECTORS.replace("\r\n", "\n");
        normalized
            .split("\n\n")
            .filter_map(|text| {
                let pairs: Vec<(String, String)> = text
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty() && !l.starts_with('#'))
                    .map(|l| {
                        let (k, v) = l.split_once(':').expect("key: value");
                        (k.trim().to_string(), v.trim().to_string())
                    })
                    .collect();
                let (kind, name) = pairs.first()?.clone();
                Some(Block { kind, name, fields: pairs[1..].to_vec() })
            })
            .collect()
    }

    fn bytes(hex: &str) -> Vec<u8> {
        hex.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    fn numbers(text: &str) -> Vec<i64> {
        text.split_whitespace().map(|n| n.parse().unwrap()).collect()
    }

    #[test]
    fn valid_vectors_decode_to_the_documented_values() {
        let valid: Vec<Block> = blocks().into_iter().filter(|b| b.kind == "vector").collect();
        assert!(valid.len() >= 7);
        for v in valid {
            let packet = decode(&bytes(v.one("hex"))).unwrap_or_else(|e| panic!("{}: {e}", v.name));
            assert_eq!(i64::from(packet.sequence), v.one("seq").parse::<i64>().unwrap(), "{}", v.name);
            assert_eq!(i64::from(packet.first_sample_ms), v.one("first_sample_ms").parse::<i64>().unwrap(), "{}", v.name);
            let g = numbers(v.one("gravity_mg"));
            assert_eq!(packet.gravity_mg.map(i64::from).to_vec(), g, "{}", v.name);
            let expected: Vec<Vec<i64>> = v.all("sample").into_iter().map(numbers).collect();
            let actual: Vec<Vec<i64>> = packet
                .samples
                .iter()
                .map(|s| s.accel_mm_s2.iter().chain(&s.gyro_mrad_s).map(|&n| i64::from(n)).collect())
                .collect();
            assert_eq!(actual, expected, "{}", v.name);
            let speed = v.fields.iter().find(|(k, _)| k == "speed_cm_s").map(|(_, s)| s.as_str()).unwrap_or("none");
            let expected_speed = (speed != "none").then(|| SpeedFix {
                cm_s: speed.parse().unwrap(),
                age_ms: v.one("fix_age_ms").parse().unwrap(),
            });
            assert_eq!(packet.speed, expected_speed, "{}", v.name);
        }
    }

    #[test]
    fn invalid_vectors_are_rejected_with_the_documented_error() {
        let invalid: Vec<Block> = blocks().into_iter().filter(|b| b.kind == "invalid").collect();
        assert!(invalid.len() >= 9);
        for v in invalid {
            let expected = match v.one("error") {
                "too_short" => PacketError::TooShort,
                "bad_version" => PacketError::BadVersion,
                "bad_length" => PacketError::BadLength,
                "bad_count" => PacketError::BadCount,
                other => panic!("{}: unknown error kind {other}", v.name),
            };
            assert_eq!(decode(&bytes(v.one("hex"))), Err(expected), "{}", v.name);
        }
    }

    #[test]
    fn a_speed_fix_converts_to_m_s() {
        assert_eq!(SpeedFix { cm_s: 1389, age_ms: 0 }.speed(), 13.89);
    }

    #[test]
    fn the_rate_command_is_clamped_to_what_the_phone_accepts() {
        assert_eq!(set_rate_command(50), [0x01, 50]);
        assert_eq!(set_rate_command(1), [0x01, 10]);
        assert_eq!(set_rate_command(250), [0x01, 100]);
    }
}

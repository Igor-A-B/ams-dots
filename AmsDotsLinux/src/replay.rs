//! Mock motion data: reads a CSV recording and plays it back over time.
//!
//! The CSV has the header `t,forward,lateral,yaw`:
//! - `t`        seconds since the start of the recording
//! - `forward`  m/s², positive = the vehicle is speeding up
//! - `lateral`  m/s², positive = passengers are pushed to the right (e.g. in a left turn)
//! - `yaw`      rad/s, turning speed; positive = turning left
//!
//! These are the same quantities the phone data will provide once it has been
//! converted to the vehicle's frame, so the rest of the app will not care
//! whether motion comes from a file or from Bluetooth.

/// One row of the recording.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub t: f32,
    pub forward: f32,
    pub lateral: f32,
    pub yaw: f32,
}

/// Reads and parses a recording from disk.
pub fn load(path: &str) -> Result<Vec<Sample>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    parse(&text)
}

/// Parses CSV text. The first line is the header and is skipped; rows must be
/// in increasing time order.
pub fn parse(text: &str) -> Result<Vec<Sample>, String> {
    let mut samples: Vec<Sample> = Vec::new();
    for (index, line) in text.lines().enumerate().skip(1) {
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(',').collect();
        if fields.len() != 4 {
            return Err(format!("line {}: expected 4 columns, got {}", index + 1, fields.len()));
        }
        let mut values = [0.0f32; 4];
        for (slot, field) in values.iter_mut().zip(&fields) {
            *slot = field
                .trim()
                .parse::<f32>()
                .map_err(|e| format!("line {}: bad number {:?}: {e}", index + 1, field))?;
        }
        let sample = Sample { t: values[0], forward: values[1], lateral: values[2], yaw: values[3] };
        if samples.last().is_some_and(|prev| sample.t <= prev.t) {
            return Err(format!("line {}: time must increase", index + 1));
        }
        samples.push(sample);
    }
    if samples.len() < 2 {
        return Err("recording needs at least two rows".to_string());
    }
    Ok(samples)
}

/// Motion at time `t` seconds, blending between the two nearest rows. After the
/// last row the recording starts over, so the dots keep moving.
pub fn at(samples: &[Sample], t: f32) -> Sample {
    let first = samples[0];
    let last = samples[samples.len() - 1];
    let length = last.t - first.t;
    let t = first.t + (t - first.t).rem_euclid(length);

    // Index of the first row at or after `t`.
    let next = samples.partition_point(|s| s.t < t).clamp(1, samples.len() - 1);
    let (a, b) = (samples[next - 1], samples[next]);
    let k = (t - a.t) / (b.t - a.t);
    Sample {
        t,
        forward: a.forward + (b.forward - a.forward) * k,
        lateral: a.lateral + (b.lateral - a.lateral) * k,
        yaw: a.yaw + (b.yaw - a.yaw) * k,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CSV: &str = "t,forward,lateral,yaw\n0,0,0,0\n1,2,-4,0.5\n2,0,0,0\n";

    #[test]
    fn parses_rows_and_skips_header() {
        let samples = parse(CSV).unwrap();
        assert_eq!(samples.len(), 3);
        assert_eq!(samples[1], Sample { t: 1.0, forward: 2.0, lateral: -4.0, yaw: 0.5 });
    }

    #[test]
    fn rejects_malformed_input() {
        assert!(parse("t,f,l,y\n0,0,0\n1,1,1,1\n").is_err());
        assert!(parse("t,f,l,y\n0,0,x,0\n1,1,1,1\n").is_err());
        assert!(parse("t,f,l,y\n1,0,0,0\n0,1,1,1\n").is_err());
        assert!(parse("t,f,l,y\n0,0,0,0\n").is_err());
    }

    #[test]
    fn interpolates_between_rows() {
        let samples = parse(CSV).unwrap();
        let s = at(&samples, 0.5);
        assert_eq!((s.forward, s.lateral, s.yaw), (1.0, -2.0, 0.25));
    }

    #[test]
    fn loops_after_the_last_row() {
        let samples = parse(CSV).unwrap();
        let s = at(&samples, 2.5); // same as 0.5
        assert_eq!((s.forward, s.lateral, s.yaw), (1.0, -2.0, 0.25));
    }

    #[test]
    fn the_bundled_recording_is_valid() {
        let samples = load("tests/data/drive.csv").unwrap();
        assert!(samples.len() > 1000);
    }
}

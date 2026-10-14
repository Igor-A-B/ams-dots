//! The phone's GPS speed as a reference for the forward acceleration.
//!
//! GPS speed (Doppler) is far more accurate than an accelerometer, which is
//! off by a bias that depends on how the phone sits and how well Android
//! removed gravity. But it arrives once a second, about a second late: dots
//! driven by it alone would react to braking after the car has stopped
//! braking. So the accelerometer keeps the timing and the speed keeps it honest.
//!
//! An observer integrates the corrected forward acceleration into a speed
//! estimate. Every new fix is compared with what the estimate was *at the time
//! of the fix* (the history covers the fix's delay), and the difference
//! corrects both the estimate and an acceleration bias:
//!
//! ```text
//! estimate' = forward - bias            (integrated per sample)
//! error     = fix.speed - estimate(fix time)
//! estimate += GAIN_SPEED * error        bias -= GAIN_BIAS * error
//! output    = forward - bias
//! ```
//!
//! A sensor that reads too high makes the estimate fall behind the GPS, which
//! lowers the bias until they agree, so over a few seconds the output follows
//! the real change of speed while keeping the accelerometer's 50 Hz detail.
//!
//! Without a fix for [`TIMEOUT_MS`] (no GPS, a tunnel, a phone that does not
//! send speed) everything is dropped and the output is the plain
//! accelerometer value again.

use std::collections::VecDeque;

/// Share of the speed error that is taken straight into the estimate at each fix.
const GAIN_SPEED: f32 = 0.5;
/// Bias change (m/s²) per m/s of speed error at each fix. With one fix a second and
/// `GAIN_SPEED` this settles with a time constant of about three seconds.
const GAIN_BIAS: f32 = 0.15;
/// The bias never exceeds this (m/s²): a phone that is more wrong than this has the wrong axis, not a bias.
const MAX_BIAS: f32 = 1.0;
/// An error bigger than this (m/s) means the estimate is lost (first fix after a gap, or the forward
/// direction just changed): it is replaced by the speed instead of corrected.
const MAX_ERROR: f32 = 4.0;
/// Without a new fix for this long the speed no longer counts.
pub const TIMEOUT_MS: i32 = 3_000;
/// How far back the estimate is remembered, to compare a fix with the estimate at its own time.
const HISTORY_MS: i32 = 6_000;
/// Two fix times closer than this are the same fix, repeated in the next packet.
const SAME_FIX_MS: i32 = 30;

/// One GPS speed measurement.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fix {
    /// m/s.
    pub speed: f32,
    /// When it was taken, on the phone's sample clock (ms, wraps at 2³²).
    pub time_ms: u32,
}

/// What `status` reports about the speed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeedStatus {
    /// Latest GPS speed, m/s.
    pub speed: f32,
    /// How much acceleration is being taken off the accelerometer, m/s².
    pub bias: f32,
}

#[derive(Default)]
pub struct SpeedAid {
    /// Speed estimate now, m/s; `None` while there is no fix to anchor it.
    estimate: Option<f32>,
    bias: f32,
    /// The estimate at recent sample times, oldest first.
    history: VecDeque<(u32, f32)>,
    /// A fix that is newer than the samples processed so far.
    pending: Option<Fix>,
    /// The last fix offered, to tell a repeat from a new one.
    last_offered: Option<Fix>,
    /// Time and speed of the last fix that was applied.
    last_applied: Option<Fix>,
}

/// Signed milliseconds from `earlier` to `later` on the wrapping phone clock.
fn since(later: u32, earlier: u32) -> i32 {
    later.wrapping_sub(earlier) as i32
}

impl SpeedAid {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets everything (new connection, new forward direction).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// The speed information of one packet. Call it before [`SpeedAid::process`] of that packet's samples.
    /// The phone repeats a fix in every packet until the next one: only a different fix time counts as new.
    pub fn offer(&mut self, fix: Option<Fix>) {
        let Some(fix) = fix else { return };
        if self.last_offered.is_some_and(|last| since(fix.time_ms, last.time_ms).abs() <= SAME_FIX_MS) {
            return;
        }
        self.last_offered = Some(fix);
        self.pending = Some(fix);
    }

    /// Corrects one forward acceleration sample (m/s²) taken at `time_ms` and standing for `dt` seconds.
    pub fn process(&mut self, time_ms: u32, forward: f32, dt: f32) -> f32 {
        if self.last_applied.is_some_and(|fix| since(time_ms, fix.time_ms) > TIMEOUT_MS) {
            let last_offered = self.last_offered;
            let pending = self.pending;
            self.reset();
            // The same old fix must not count as new when it keeps being repeated.
            self.last_offered = last_offered;
            self.pending = pending;
        }

        if let Some(estimate) = self.estimate.as_mut() {
            *estimate += (forward - self.bias) * dt;
            let estimate = *estimate;
            self.history.push_back((time_ms, estimate));
            while self.history.front().is_some_and(|(t, _)| since(time_ms, *t) > HISTORY_MS) {
                self.history.pop_front();
            }
        }

        if let Some(fix) = self.pending.filter(|fix| since(time_ms, fix.time_ms) >= 0) {
            self.pending = None;
            self.apply(fix, time_ms);
        }
        forward - self.bias
    }

    /// The GPS speed and the correction in use, while the speed counts.
    pub fn status(&self) -> Option<SpeedStatus> {
        let fix = self.last_applied?;
        self.estimate.map(|_| SpeedStatus { speed: fix.speed, bias: self.bias })
    }

    fn apply(&mut self, fix: Fix, now_ms: u32) {
        let Some(current) = self.estimate else {
            // The first fix: nothing to compare with. The estimate starts at the GPS speed.
            self.estimate = Some(fix.speed);
            self.history.clear();
            self.history.push_back((now_ms, fix.speed));
            self.last_applied = Some(fix);
            return;
        };
        let Some(then) = self.estimate_at(fix.time_ms) else {
            return; // older than the history: cannot be compared
        };
        let error = fix.speed - then;
        let (shift, bias) = if error.abs() > MAX_ERROR {
            (error, 0.0)
        } else {
            (GAIN_SPEED * error, (self.bias - GAIN_BIAS * error).clamp(-MAX_BIAS, MAX_BIAS))
        };
        self.estimate = Some(current + shift);
        for (_, v) in self.history.iter_mut() {
            *v += shift;
        }
        self.bias = bias;
        self.last_applied = Some(fix);
    }

    /// The estimate at `time_ms`, interpolated between the two samples around it.
    fn estimate_at(&self, time_ms: u32) -> Option<f32> {
        let (first_t, first_v) = *self.history.front()?;
        if since(time_ms, first_t) < 0 {
            return None;
        }
        let mut before = (first_t, first_v);
        for &(t, v) in &self.history {
            if since(t, time_ms) >= 0 {
                let span = since(t, before.0);
                if span <= 0 {
                    return Some(v);
                }
                let share = since(time_ms, before.0) as f32 / span as f32;
                return Some(before.1 + (v - before.1) * share);
            }
            before = (t, v);
        }
        Some(before.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 0.02;
    const STEP_MS: u32 = 20;
    /// Time the fix takes to reach the laptop, ms.
    const DELAY_MS: u32 = 150;

    /// A simulated drive: true acceleration `truth(t)`, an accelerometer reading `truth + sensor_bias`, a GPS
    /// fix every second (speed of that moment, delivered `DELAY_MS` later), repeated in every packet (4 samples).
    struct Drive {
        aid: SpeedAid,
        t_ms: u32,
        speed: f32,
        sensor_bias: f32,
        /// The fixes made so far and not yet delivered.
        in_flight: Vec<(Fix, u32)>,
        latest_delivered: Option<Fix>,
        gps_on: bool,
        next_fix_ms: u32,
        outputs: Vec<(u32, f32, f32)>,
    }

    impl Drive {
        fn new(sensor_bias: f32, start_speed: f32) -> Self {
            Self {
                aid: SpeedAid::new(),
                t_ms: 1_000_000,
                speed: start_speed,
                sensor_bias,
                in_flight: Vec::new(),
                latest_delivered: None,
                gps_on: true,
                next_fix_ms: 1_000_000,
                outputs: Vec::new(),
            }
        }

        fn run(&mut self, seconds: f32, truth: impl Fn(f32) -> f32) {
            let start = self.t_ms;
            let samples = (seconds / DT).round() as u32;
            for i in 0..samples {
                if i % 4 == 0 {
                    self.aid.offer(self.latest_delivered);
                }
                let elapsed = self.t_ms.wrapping_sub(start) as f32 / 1000.0;
                let true_accel = truth(elapsed);
                self.speed += true_accel * DT;
                if self.gps_on && since(self.t_ms, self.next_fix_ms) >= 0 {
                    self.in_flight.push((Fix { speed: self.speed, time_ms: self.t_ms }, self.t_ms.wrapping_add(DELAY_MS)));
                    self.next_fix_ms = self.next_fix_ms.wrapping_add(1000);
                }
                if let Some(at) = self.in_flight.iter().position(|(_, due)| since(self.t_ms, *due) >= 0) {
                    self.latest_delivered = Some(self.in_flight.remove(at).0);
                }
                let out = self.aid.process(self.t_ms, true_accel + self.sensor_bias, DT);
                self.outputs.push((self.t_ms, out, true_accel));
                self.t_ms = self.t_ms.wrapping_add(STEP_MS);
            }
        }

        /// Mean of (output - truth) over the last `seconds` of the run.
        fn mean_error(&self, seconds: f32) -> f32 {
            let n = (seconds / DT) as usize;
            let tail = &self.outputs[self.outputs.len() - n..];
            tail.iter().map(|(_, out, truth)| out - truth).sum::<f32>() / n as f32
        }
    }

    #[test]
    fn without_a_fix_the_accelerometer_passes_through() {
        let mut aid = SpeedAid::new();
        for i in 0..500u32 {
            let forward = (i as f32 * 0.1).sin();
            assert_eq!(aid.process(i * STEP_MS, forward, DT), forward);
        }
        assert_eq!(aid.status(), None);
    }

    #[test]
    fn a_constant_sensor_bias_is_removed_while_driving_steadily() {
        let mut drive = Drive::new(0.4, 12.0);
        drive.run(30.0, |_| 0.0);
        assert!(drive.mean_error(5.0).abs() < 0.03, "{}", drive.mean_error(5.0));
        let status = drive.aid.status().expect("synced");
        assert!((status.bias - 0.4).abs() < 0.05, "{status:?}");
        assert!((status.speed - 12.0).abs() < 0.2);
    }

    #[test]
    fn real_acceleration_still_gets_through_at_full_strength() {
        // 2 m/s² for 3 s after 20 s of cruising, with a 0.3 m/s² sensor bias.
        let mut drive = Drive::new(0.3, 8.0);
        drive.run(20.0, |_| 0.0);
        let before = drive.outputs.len();
        drive.run(3.0, |_| 2.0);
        let peak = drive.outputs[before..].iter().map(|(_, out, _)| *out).fold(f32::MIN, f32::max);
        assert!((peak - 2.0).abs() < 0.25, "{peak}");
        drive.run(15.0, |_| 0.0);
        assert!(drive.mean_error(5.0).abs() < 0.05, "settles after the manoeuvre: {}", drive.mean_error(5.0));
    }

    #[test]
    fn a_repeated_fix_counts_once() {
        let mut aid = SpeedAid::new();
        let first = Fix { speed: 10.0, time_ms: 1000 };
        aid.offer(Some(first));
        aid.process(1000, 0.0, DT);
        // The estimate sits at 10, a repeat of a second fix 1 m/s higher must pull the bias only once.
        let second = Fix { speed: 11.0, time_ms: 2000 };
        for t in (1020..=2000).step_by(20) {
            aid.process(t, 0.0, DT);
        }
        for t in (2020..=2400).step_by(20) {
            aid.offer(Some(second));
            aid.process(t, 0.0, DT);
        }
        let bias = aid.status().unwrap().bias;
        assert!((bias + GAIN_BIAS * 1.0).abs() < 0.01, "{bias}");
    }

    #[test]
    fn a_fix_newer_than_the_packet_waits_for_its_sample() {
        let mut aid = SpeedAid::new();
        aid.offer(Some(Fix { speed: 5.0, time_ms: 1000 }));
        aid.process(1000, 0.0, DT);
        for t in (1020..=1980).step_by(20) {
            aid.process(t, 0.0, DT);
        }
        // Offered with the packet that starts at 1980, stamped 2040.
        aid.offer(Some(Fix { speed: 6.0, time_ms: 2040 }));
        aid.process(1980, 0.0, DT);
        assert_eq!(aid.status().unwrap().speed, 5.0, "not applied yet");
        aid.process(2000, 0.0, DT);
        aid.process(2020, 0.0, DT);
        aid.process(2040, 0.0, DT);
        assert_eq!(aid.status().unwrap().speed, 6.0);
    }

    #[test]
    fn the_accelerometer_takes_over_again_when_the_speed_stops_coming() {
        let mut drive = Drive::new(0.4, 12.0);
        drive.run(20.0, |_| 0.0);
        assert!(drive.mean_error(2.0).abs() < 0.05);
        drive.gps_on = false;
        drive.run(5.0, |_| 0.0);
        // The last fix keeps being repeated, but it is old.
        assert!(drive.aid.status().is_none());
        assert!((drive.mean_error(1.0) - 0.4).abs() < 1e-3, "plain accelerometer again: {}", drive.mean_error(1.0));
    }

    #[test]
    fn the_speed_comes_back_after_a_gap() {
        let mut drive = Drive::new(0.4, 12.0);
        drive.run(10.0, |_| 0.0);
        drive.gps_on = false;
        drive.run(6.0, |_| 0.0);
        drive.gps_on = true;
        drive.next_fix_ms = drive.t_ms;
        drive.run(30.0, |_| 0.0);
        assert!(drive.mean_error(5.0).abs() < 0.05, "{}", drive.mean_error(5.0));
    }

    #[test]
    fn a_lost_estimate_resyncs_instead_of_correcting() {
        let mut aid = SpeedAid::new();
        aid.offer(Some(Fix { speed: 10.0, time_ms: 1000 }));
        aid.process(1000, 0.0, DT);
        for t in (1020..=2000).step_by(20) {
            aid.process(t, 0.0, DT);
        }
        // The speed is 9 m/s away from what the accelerometer implied (e.g. the forward direction just flipped).
        aid.offer(Some(Fix { speed: 19.0, time_ms: 2000 }));
        aid.process(2020, 0.0, DT);
        let status = aid.status().unwrap();
        assert_eq!(status.bias, 0.0);
        assert!(aid.estimate.unwrap() > 18.9);
    }

    #[test]
    fn the_bias_is_limited() {
        let mut drive = Drive::new(3.0, 5.0);
        drive.run(30.0, |_| 0.0);
        assert!(drive.aid.status().map_or(true, |s| s.bias.abs() <= MAX_BIAS + 1e-6));
    }

    #[test]
    fn the_clock_wrapping_is_harmless() {
        let mut drive = Drive::new(0.4, 12.0);
        drive.t_ms = u32::MAX - 8_000;
        drive.next_fix_ms = drive.t_ms;
        drive.run(30.0, |_| 0.0);
        assert!(drive.mean_error(5.0).abs() < 0.05, "{}", drive.mean_error(5.0));
    }
}

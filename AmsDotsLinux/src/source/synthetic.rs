//! Simulated phone: generates fake "vehicle" motion so we can develop without a phone.

use super::MotionSource;
use crate::types::{SensorSample, Vec3};
use std::time::Duration;

/// Sine-wave braking/accelerating on the forward axis. Scenarios (turns, bumps,
/// replayed recordings) will be added here later.
pub struct SyntheticSource {
    rate_hz: f32,
    next_t: Duration,
}

impl SyntheticSource {
    pub fn new(rate_hz: f32) -> Self {
        Self { rate_hz, next_t: Duration::ZERO }
    }
}

impl MotionSource for SyntheticSource {
    fn poll(&mut self, now: Duration) -> Option<SensorSample> {
        if self.next_t > now {
            return None;
        }
        let t = self.next_t;
        self.next_t += Duration::from_secs_f32(1.0 / self.rate_hz);
        let accel = Vec3 { x: 0.0, y: 3.0 * (t.as_secs_f32() * 0.8).sin(), z: 0.0 };
        Some(SensorSample { t, accel, gyro: Vec3::default() })
    }
}

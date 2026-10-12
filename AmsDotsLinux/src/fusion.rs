//! Stage 2 — Fusion. Phone frame -> vehicle frame, filtering, calibration.

use crate::types::{SensorSample, VehicleMotion};

pub trait Fusion {
    fn push(&mut self, sample: SensorSample);
    /// Latest filtered motion estimate.
    fn current(&self) -> VehicleMotion;
    /// Learn the phone->vehicle axis mapping from recent motion.
    fn calibrate(&mut self);
}

/// Placeholder: assumes phone Y = forward, X = right, Z = up; no filtering.
#[derive(Default)]
pub struct PassthroughFusion {
    latest: VehicleMotion,
}

impl Fusion for PassthroughFusion {
    fn push(&mut self, s: SensorSample) {
        self.latest = VehicleMotion { forward: s.accel.y, lateral: s.accel.x, yaw_rate: s.gyro.z };
    }

    fn current(&self) -> VehicleMotion {
        self.latest
    }

    fn calibrate(&mut self) {}
}

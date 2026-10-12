//! Plain data types shared by every pipeline stage.

use std::time::Duration;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

/// One raw reading from the phone, in the phone's own axes.
#[derive(Debug, Clone, Copy, Default)]
pub struct SensorSample {
    /// Time since the source started (phone-side clock, not wall time).
    pub t: Duration,
    /// Linear acceleration in m/s², gravity already removed.
    pub accel: Vec3,
    /// Angular velocity in rad/s.
    pub gyro: Vec3,
}

/// Motion expressed in the vehicle's frame (output of fusion).
#[derive(Debug, Clone, Copy, Default)]
pub struct VehicleMotion {
    /// m/s², positive = speeding up.
    pub forward: f32,
    /// m/s², positive = pushed to the right.
    pub lateral: f32,
    /// rad/s, positive = turning left.
    pub yaw_rate: f32,
}

/// A dot's position in normalized screen coordinates (0..1 on both axes).
#[derive(Debug, Clone, Copy, Default)]
pub struct Dot {
    pub pos: Vec2,
    pub radius: f32,
    pub opacity: f32,
}

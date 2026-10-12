//! Stage 1 — Ingest. Anything that can produce `SensorSample`s.

mod bluetooth;
mod synthetic;

pub use bluetooth::BluetoothSource;
pub use synthetic::SyntheticSource;

use crate::types::SensorSample;
use std::time::Duration;

/// Pull-based and non-blocking: the main loop drains it every frame.
/// Simulated, replayed and real (Bluetooth) inputs are interchangeable.
pub trait MotionSource {
    /// Return the next sample available at `now` (time since app start), if any.
    fn poll(&mut self, now: Duration) -> Option<SensorSample>;

    /// False when the link is down (dots should ease back to rest).
    fn connected(&self) -> bool {
        true
    }
}

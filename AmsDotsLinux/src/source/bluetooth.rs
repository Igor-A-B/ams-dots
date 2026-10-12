//! Real phone over Bluetooth tethering (IP over BT PAN). Not implemented yet.
//!
//! Plan: bind a UDP socket on the PAN interface, parse the phone app's packets
//! into `SensorSample`s, track connection state with a timeout.

use super::MotionSource;
use crate::types::SensorSample;
use std::time::Duration;

pub struct BluetoothSource;

impl MotionSource for BluetoothSource {
    fn poll(&mut self, _now: Duration) -> Option<SensorSample> {
        None
    }

    fn connected(&self) -> bool {
        false
    }
}

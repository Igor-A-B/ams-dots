//! Where the dots get their motion from: nothing (vehicle at rest), a CSV
//! recording, or the phone over Bluetooth. The overlay asks one question per
//! frame, [`MotionSource::at`], and does not care which it is.

use crate::fusion::{CalibrationEvent, Fusion, Motion};
use crate::protocol::Packet;
use crate::replay::{self, Sample};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// Without a packet for this long the phone counts as gone and the dots settle.
/// At 50 Hz a packet arrives about every 80 ms, so this tolerates a few lost ones.
pub const STALE_AFTER: Duration = Duration::from_millis(500);

pub enum MotionSource {
    /// Vehicle at rest.
    Rest,
    /// A recording, looped.
    Replay(Vec<Sample>),
    /// The phone, via the Bluetooth receiver thread.
    Live(Arc<Live>),
}

impl MotionSource {
    /// Motion `elapsed` seconds after the overlay started.
    pub fn at(&self, elapsed: f32) -> Sample {
        match self {
            MotionSource::Rest => Sample { t: elapsed, forward: 0.0, lateral: 0.0, yaw: 0.0 },
            MotionSource::Replay(samples) => replay::at(samples, elapsed),
            MotionSource::Live(live) => {
                let m = live.current(Instant::now());
                Sample { t: elapsed, forward: m.forward, lateral: m.lateral, yaw: m.yaw }
            }
        }
    }
}

/// What a received packet changed, for the receiver to log.
#[derive(Debug, Default, PartialEq)]
pub struct PushOutcome {
    /// Packets missed since the previous one (from the sequence numbers).
    pub lost: u16,
    pub calibration: Option<CalibrationEvent>,
}

struct Inner {
    fusion: Fusion,
    latest: Motion,
    last_packet: Option<Instant>,
    connected: bool,
    last_sequence: Option<u16>,
}

/// The newest motion from the phone, shared between the Bluetooth thread
/// (writes) and the overlay (reads once per frame).
pub struct Live {
    inner: Mutex<Inner>,
}

impl Live {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                fusion: Fusion::new(),
                latest: Motion::default(),
                last_packet: None,
                connected: false,
                last_sequence: None,
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Feeds one packet. Every sample goes through the fusion (calibration
    /// needs them all); the newest one is what the dots show.
    pub fn push(&self, packet: &Packet, now: Instant) -> PushOutcome {
        let mut inner = self.lock();
        let lost = inner.last_sequence.map_or(0, |prev| packet.sequence.wrapping_sub(prev).wrapping_sub(1));
        // A huge "gap" is a reordered or restarted stream, not thousands of lost packets.
        let lost = if lost > u16::MAX / 2 { 0 } else { lost };
        inner.last_sequence = Some(packet.sequence);
        for sample in &packet.samples {
            inner.latest = inner.fusion.process(packet.gravity_mg, sample);
        }
        inner.last_packet = Some(now);
        PushOutcome { lost, calibration: inner.fusion.take_event() }
    }

    /// The Bluetooth thread reports the link going up or down. Down means no motion.
    pub fn set_connected(&self, connected: bool) {
        let mut inner = self.lock();
        inner.connected = connected;
        inner.last_sequence = None;
        if !connected {
            inner.latest = Motion::default();
            inner.last_packet = None;
        }
    }

    pub fn is_connected(&self) -> bool {
        self.lock().connected
    }

    /// Starts a calibration (see [`Fusion::calibrate`]).
    pub fn calibrate(&self) {
        self.lock().fusion.calibrate();
    }

    /// The motion to show at `now`: the newest sample, or zero while the phone is disconnected or silent.
    pub fn current(&self, now: Instant) -> Motion {
        let inner = self.lock();
        match inner.last_packet {
            Some(at) if inner.connected && now.saturating_duration_since(at) <= STALE_AFTER => inner.latest,
            _ => Motion::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::RawSample;

    fn packet(sequence: u16, accel_y: i16) -> Packet {
        Packet {
            sequence,
            first_sample_ms: u32::from(sequence) * 80,
            gravity_mg: [0, 0, 1000],
            samples: vec![RawSample { accel_mm_s2: [0, accel_y, 0], gyro_mrad_s: [0; 3] }; 2],
        }
    }

    #[test]
    fn shows_the_newest_motion_while_fresh() {
        let live = Live::new();
        let t0 = Instant::now();
        live.set_connected(true);
        live.push(&packet(0, 2000), t0);
        let m = live.current(t0 + Duration::from_millis(100));
        assert!((m.forward - 2.0).abs() < 0.01);
    }

    #[test]
    fn settles_to_zero_when_packets_stop() {
        let live = Live::new();
        let t0 = Instant::now();
        live.set_connected(true);
        live.push(&packet(0, 2000), t0);
        assert_eq!(live.current(t0 + STALE_AFTER + Duration::from_millis(1)), Motion::default());
    }

    #[test]
    fn settles_to_zero_on_disconnect_and_ignores_stale_data_after_reconnect() {
        let live = Live::new();
        let t0 = Instant::now();
        live.set_connected(true);
        live.push(&packet(0, 2000), t0);
        live.set_connected(false);
        assert_eq!(live.current(t0), Motion::default());
        live.set_connected(true);
        assert_eq!(live.current(t0), Motion::default(), "no packet since reconnecting");
    }

    #[test]
    fn counts_lost_packets_from_the_sequence_numbers() {
        let live = Live::new();
        let t0 = Instant::now();
        live.set_connected(true);
        assert_eq!(live.push(&packet(10, 0), t0).lost, 0);
        assert_eq!(live.push(&packet(11, 0), t0).lost, 0);
        assert_eq!(live.push(&packet(14, 0), t0).lost, 2);
        assert_eq!(live.push(&packet(15, 0), t0).lost, 0);
    }

    #[test]
    fn the_sequence_wraps_without_a_false_loss() {
        let live = Live::new();
        let t0 = Instant::now();
        live.set_connected(true);
        live.push(&packet(u16::MAX, 0), t0);
        assert_eq!(live.push(&packet(0, 0), t0).lost, 0);
    }

    #[test]
    fn a_repeated_or_restarted_sequence_is_not_counted_as_loss() {
        let live = Live::new();
        let t0 = Instant::now();
        live.set_connected(true);
        live.push(&packet(500, 0), t0);
        assert_eq!(live.push(&packet(3, 0), t0).lost, 0);
    }

    #[test]
    fn rest_and_replay_sources_answer_per_frame() {
        assert_eq!(MotionSource::Rest.at(1.0).forward, 0.0);
        let samples = replay::parse("t,f,l,y\n0,0,0,0\n1,2,0,0\n").unwrap();
        assert_eq!(MotionSource::Replay(samples).at(0.5).forward, 1.0);
    }

    #[test]
    fn calibration_events_are_reported_to_the_receiver() {
        let live = Live::new();
        let t0 = Instant::now();
        live.set_connected(true);
        live.calibrate();
        let mut event = None;
        for seq in 0..60 {
            event = event.or(live.push(&packet(seq, 2000), t0).calibration);
        }
        assert!(matches!(event, Some(CalibrationEvent::Done { .. })));
    }
}

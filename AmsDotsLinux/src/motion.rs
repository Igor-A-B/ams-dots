//! Where the dots get their motion from: nothing (vehicle at rest), a CSV
//! recording, or the phone over Bluetooth. The overlay asks one question per
//! frame, [`MotionSource::at`], and does not care which it is.
//!
//! [`Live`] is also the meeting point of the Bluetooth thread, the fusion and
//! the `status` / `connect` / `disconnect` commands: it holds the newest motion,
//! the state of the link and a few counters.

use crate::fusion::{ForwardState, Fusion, Motion};
use crate::protocol::Packet;
use crate::replay::{self, Sample};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use tokio::sync::Notify;

/// Without a packet for this long the phone counts as gone and the dots settle.
/// At 50 Hz a packet arrives about every 80 ms, so this tolerates a few lost ones.
pub const STALE_AFTER: Duration = Duration::from_millis(500);

/// Spacing of samples when the packets do not say (50 Hz, the phone's default).
const DEFAULT_SAMPLE_DT: f32 = 0.02;

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

/// Where the connection to the phone stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    /// Switched off with `disconnect`; nothing is scanned.
    Paused,
    /// Looking for the phone's service.
    Scanning,
    /// Found; connecting and setting up.
    Connecting,
    /// Receiving motion.
    Connected,
}

impl LinkState {
    pub fn text(self) -> &'static str {
        match self {
            LinkState::Paused => "paused (run `ams-dots-linux connect`)",
            LinkState::Scanning => "scanning for the phone",
            LinkState::Connecting => "connecting",
            LinkState::Connected => "connected",
        }
    }
}

/// What a received packet changed, for the receiver to log.
#[derive(Debug, Default, PartialEq)]
pub struct PushOutcome {
    /// Packets missed since the previous one (from the sequence numbers).
    pub lost: u16,
    /// The forward direction was learned (or its confirmation changed) with this packet.
    pub forward_learned: Option<ForwardState>,
}

/// Everything `status` reports.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub link: LinkState,
    pub phone: Option<String>,
    pub motion: Motion,
    /// Milliseconds since the last packet.
    pub age_ms: Option<u64>,
    /// Packets per second, smoothed.
    pub packets_per_s: f32,
    pub packets: u64,
    pub lost: u64,
    pub forward: ForwardState,
}

impl Status {
    /// One line, for `status --watch`.
    pub fn line(&self) -> String {
        let age = self.age_ms.map_or("    -".to_string(), |ms| format!("{ms:>4}ms"));
        format!(
            "{:<10} {:>4.1} pkt/s lost {:<3} age {}  fwd {:>6.2} lat {:>6.2} yaw {:>6.2}  heading: {}",
            match self.link {
                LinkState::Paused => "paused",
                LinkState::Scanning => "scanning",
                LinkState::Connecting => "connecting",
                LinkState::Connected => "connected",
            },
            self.packets_per_s,
            self.lost,
            age,
            self.motion.forward,
            self.motion.lateral,
            self.motion.yaw,
            self.forward.text(),
        )
    }

    /// Several lines, for `status`.
    pub fn report(&self) -> String {
        format!(
            "link:     {}\nphone:    {}\npackets:  {} received, {} lost, {:.1} per second\nmotion:   forward {:.2} m/s2, lateral {:.2} m/s2, yaw {:.2} rad/s\nforward:  {}",
            self.link.text(),
            self.phone.as_deref().unwrap_or("-"),
            self.packets,
            self.lost,
            self.packets_per_s,
            self.motion.forward,
            self.motion.lateral,
            self.motion.yaw,
            self.forward.text(),
        )
    }
}

impl ForwardState {
    pub fn text(&self) -> String {
        match *self {
            ForwardState::Guessing { moving_s } => format!("guessing from the phone's pose, learning ({moving_s:.0} of 5 s of straight driving)"),
            ForwardState::Learned { confidence, from_turns: true } => format!("learned from driving ({:.0}% sure)", confidence * 100.0),
            ForwardState::Learned { confidence, from_turns: false } => format!("learned the axis ({:.0}% sure), front end assumed until the first turn", confidence * 100.0),
        }
    }
}

struct Inner {
    fusion: Fusion,
    latest: Motion,
    last_packet: Option<Instant>,
    link: LinkState,
    enabled: bool,
    phone: Option<String>,
    last_sequence: Option<u16>,
    /// Time of the previous packet's first sample and its sample count, to measure the sample spacing.
    previous: Option<(u32, usize)>,
    packets: u64,
    lost: u64,
    /// Seconds between packets, smoothed.
    interval: Option<f32>,
}

/// The newest motion from the phone, shared between the Bluetooth thread
/// (writes), the overlay (reads once per frame) and the control commands.
pub struct Live {
    inner: Mutex<Inner>,
    /// Woken when `connect` / `disconnect` flips [`Live::is_enabled`].
    toggled: Notify,
}

impl Live {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                fusion: Fusion::new(),
                latest: Motion::default(),
                last_packet: None,
                link: LinkState::Scanning,
                enabled: true,
                phone: None,
                last_sequence: None,
                previous: None,
                packets: 0,
                lost: 0,
                interval: None,
            }),
            toggled: Notify::new(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Feeds one packet. Every sample goes through the fusion (it learns from all
    /// of them); the newest one is what the dots show.
    pub fn push(&self, packet: &Packet, now: Instant) -> PushOutcome {
        let mut inner = self.lock();
        let lost = inner.last_sequence.map_or(0, |prev| packet.sequence.wrapping_sub(prev).wrapping_sub(1));
        // A huge "gap" is a reordered or restarted stream, not thousands of lost packets.
        let lost = if lost > u16::MAX / 2 { 0 } else { lost };
        inner.last_sequence = Some(packet.sequence);

        // Sample spacing: from the time between the first samples of consecutive packets.
        let dt = match inner.previous {
            Some((first_ms, count)) if lost == 0 && count > 0 => {
                let ms = packet.first_sample_ms.wrapping_sub(first_ms) as f32 / count as f32;
                if (2.0..200.0).contains(&ms) { ms / 1000.0 } else { DEFAULT_SAMPLE_DT }
            }
            _ => DEFAULT_SAMPLE_DT,
        };
        inner.previous = Some((packet.first_sample_ms, packet.samples.len()));

        let before = inner.fusion.forward_state();
        for sample in &packet.samples {
            inner.latest = inner.fusion.process(packet.gravity_mg, sample, dt);
        }
        let after = inner.fusion.forward_state();

        if let Some(previous) = inner.last_packet {
            let seconds = now.saturating_duration_since(previous).as_secs_f32();
            inner.interval = Some(inner.interval.map_or(seconds, |avg| avg * 0.9 + seconds * 0.1));
        }
        inner.last_packet = Some(now);
        inner.packets += 1;
        inner.lost += u64::from(lost);

        let forward_learned = match (before, after) {
            (ForwardState::Guessing { .. }, ForwardState::Learned { .. }) => Some(after),
            (ForwardState::Learned { from_turns: false, .. }, ForwardState::Learned { from_turns: true, .. }) => Some(after),
            _ => None,
        };
        PushOutcome { lost, forward_learned }
    }

    /// The Bluetooth thread reports where the link stands. Anything but `Connected` means no motion.
    pub fn set_link(&self, link: LinkState) {
        let mut inner = self.lock();
        inner.link = link;
        inner.last_sequence = None;
        inner.previous = None;
        inner.interval = None;
        if link != LinkState::Connected {
            inner.latest = Motion::default();
            inner.last_packet = None;
        }
        if matches!(link, LinkState::Scanning | LinkState::Paused) {
            inner.phone = None;
        }
    }

    pub fn set_phone(&self, address: String) {
        self.lock().phone = Some(address);
    }

    /// `connect` / `disconnect`: whether the receiver should look for the phone at all.
    pub fn set_enabled(&self, enabled: bool) {
        self.lock().enabled = enabled;
        self.toggled.notify_waiters();
        self.toggled.notify_one();
    }

    pub fn is_enabled(&self) -> bool {
        self.lock().enabled
    }

    /// Resolves the next time [`Live::set_enabled`] is called.
    pub async fn toggled(&self) {
        self.toggled.notified().await;
    }

    /// Forgets the learned forward direction (`relearn`).
    pub fn relearn(&self) {
        self.lock().fusion.relearn();
    }

    /// The motion to show at `now`: the newest sample, or zero while the phone is disconnected or silent.
    pub fn current(&self, now: Instant) -> Motion {
        let inner = self.lock();
        match inner.last_packet {
            Some(at) if inner.link == LinkState::Connected && now.saturating_duration_since(at) <= STALE_AFTER => inner.latest,
            _ => Motion::default(),
        }
    }

    pub fn status(&self, now: Instant) -> Status {
        let motion = self.current(now);
        let inner = self.lock();
        Status {
            link: inner.link,
            phone: inner.phone.clone(),
            motion,
            age_ms: inner.last_packet.map(|at| now.saturating_duration_since(at).as_millis() as u64),
            packets_per_s: inner.interval.filter(|s| *s > 0.0).map_or(0.0, |s| 1.0 / s),
            packets: inner.packets,
            lost: inner.lost,
            forward: inner.fusion.forward_state(),
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
            samples: vec![RawSample { accel_mm_s2: [0, accel_y, 0], gyro_mrad_s: [0; 3] }; 4],
        }
    }

    fn connected() -> (Arc<Live>, Instant) {
        let live = Live::new();
        live.set_link(LinkState::Connected);
        (live, Instant::now())
    }

    #[test]
    fn shows_the_newest_motion_while_fresh() {
        let (live, t0) = connected();
        live.push(&packet(0, 2000), t0);
        let m = live.current(t0 + Duration::from_millis(100));
        assert!((m.forward - 2.0).abs() < 0.01);
    }

    #[test]
    fn settles_to_zero_when_packets_stop() {
        let (live, t0) = connected();
        live.push(&packet(0, 2000), t0);
        assert_eq!(live.current(t0 + STALE_AFTER + Duration::from_millis(1)), Motion::default());
    }

    #[test]
    fn settles_to_zero_on_disconnect_and_ignores_stale_data_after_reconnect() {
        let (live, t0) = connected();
        live.push(&packet(0, 2000), t0);
        live.set_link(LinkState::Scanning);
        assert_eq!(live.current(t0), Motion::default());
        live.set_link(LinkState::Connected);
        assert_eq!(live.current(t0), Motion::default(), "no packet since reconnecting");
    }

    #[test]
    fn counts_lost_packets_from_the_sequence_numbers() {
        let (live, t0) = connected();
        assert_eq!(live.push(&packet(10, 0), t0).lost, 0);
        assert_eq!(live.push(&packet(11, 0), t0).lost, 0);
        assert_eq!(live.push(&packet(14, 0), t0).lost, 2);
        assert_eq!(live.push(&packet(15, 0), t0).lost, 0);
        assert_eq!(live.status(t0).lost, 2);
        assert_eq!(live.status(t0).packets, 4);
    }

    #[test]
    fn the_sequence_wraps_without_a_false_loss() {
        let (live, t0) = connected();
        live.push(&packet(u16::MAX, 0), t0);
        assert_eq!(live.push(&packet(0, 0), t0).lost, 0);
    }

    #[test]
    fn a_repeated_or_restarted_sequence_is_not_counted_as_loss() {
        let (live, t0) = connected();
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
    fn the_packet_rate_is_measured() {
        let (live, t0) = connected();
        for i in 0..20u16 {
            live.push(&packet(i, 0), t0 + Duration::from_millis(80 * u64::from(i)));
        }
        let rate = live.status(t0 + Duration::from_millis(80 * 20)).packets_per_s;
        assert!((rate - 12.5).abs() < 0.5, "{rate}");
    }

    #[test]
    fn learning_is_reported_once_it_happens() {
        // 6 s of acceleration back and forth along the phone's Y axis.
        let (live, t0) = connected();
        let mut learned = None;
        for seq in 0..120u16 {
            let accel = if (seq / 20) % 2 == 0 { 2000 } else { -2000 };
            learned = learned.or(live.push(&packet(seq, accel), t0).forward_learned);
        }
        assert!(matches!(learned, Some(ForwardState::Learned { .. })), "{learned:?}");
    }

    #[test]
    fn the_sample_spacing_comes_from_the_packet_times() {
        // Packets 160 ms apart with 4 samples = 40 ms per sample (25 Hz): 5 s of movement takes 125 samples, not 250.
        let (live, t0) = connected();
        let make = |seq: u16, accel: i16| Packet { first_sample_ms: u32::from(seq) * 160, ..packet(seq, accel) };
        let mut learned_at = None;
        for seq in 0..60u16 {
            let accel = if (seq / 8) % 2 == 0 { 2000 } else { -2000 };
            if live.push(&make(seq, accel), t0).forward_learned.is_some() && learned_at.is_none() {
                learned_at = Some(seq);
            }
        }
        let seq = learned_at.expect("learned");
        // 5 s at 160 ms per packet is 31.25 packets.
        assert!((31..=35).contains(&seq), "learned after packet {seq}");
    }

    #[test]
    fn connect_and_disconnect_toggle_the_receiver() {
        let live = Live::new();
        assert!(live.is_enabled());
        live.set_enabled(false);
        assert!(!live.is_enabled());
    }
}

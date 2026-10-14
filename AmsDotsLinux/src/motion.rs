//! Where the dots get their motion from: a CSV recording or the phone over
//! Bluetooth (no phone connected means a vehicle at rest). The overlay asks one
//! question per frame, [`MotionSource::at`], and does not care which it is.
//!
//! [`Live`] is also the meeting point of the Bluetooth thread, the fusion and
//! the `status` / `connect` / `disconnect` / `phones` / `select` commands: it
//! holds the newest motion, the state of the link, a few counters, which phone
//! to follow and which phones were heard recently.

use crate::fusion::{ForwardState, Fusion, Motion};
use crate::picker::{Candidate, Target};
use crate::protocol::Packet;
use crate::replay::{self, Sample};
use crate::speed::{Fix, SpeedAid, SpeedStatus};
use std::io::Write;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use tokio::sync::Notify;

/// Without a packet for this long the phone counts as gone and the dots settle.
/// At 50 Hz a packet arrives about every 80 ms, so this tolerates a few lost ones.
pub const STALE_AFTER: Duration = Duration::from_millis(500);

/// A phone heard this long ago still shows up in `phones` (a connected phone does not advertise, so
/// the list would otherwise lose everything but the connected one).
pub const SIGHTING_MAX_AGE: Duration = Duration::from_secs(30);

/// Spacing of samples when the packets do not say (50 Hz, the phone's default).
const DEFAULT_SAMPLE_DT: f32 = 0.02;

pub enum MotionSource {
    /// A recording, looped.
    Replay(Vec<Sample>),
    /// The phone, via the Bluetooth receiver thread. Zero motion while no phone is connected.
    Live(Arc<Live>),
}

impl MotionSource {
    /// Motion `elapsed` seconds after the overlay started.
    pub fn at(&self, elapsed: f32) -> Sample {
        match self {
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

/// Columns of a `--record` file: the packet's sequence, the sample's time (ms, phone clock), the raw phone
/// values exactly as received (gravity milli-g, acceleration mm/s², rotation mrad/s, phone axes) and the
/// fusion's output (m/s², rad/s). `forward` already has the GPS correction; `gps_speed` (m/s, the newest fix
/// in use) and `bias` (m/s² taken off the accelerometer) are empty while the phone sends no speed.
pub const RECORDING_HEADER: &str = "seq,t_ms,grav_x,grav_y,grav_z,acc_x,acc_y,acc_z,gyro_x,gyro_y,gyro_z,forward,lateral,yaw,gps_speed,bias\n";

fn recording_row(packet: &Packet, index: usize, dt: f32, m: Motion, gps: Option<SpeedStatus>) -> String {
    let s = &packet.samples[index];
    let t_ms = f64::from(packet.first_sample_ms) + index as f64 * f64::from(dt) * 1000.0;
    let [gx, gy, gz] = packet.gravity_mg;
    let [ax, ay, az] = s.accel_mm_s2;
    let [wx, wy, wz] = s.gyro_mrad_s;
    let gps = gps.map_or(",".to_string(), |g| format!("{:.2},{:.3}", g.speed, g.bias));
    format!(
        "{},{t_ms:.0},{gx},{gy},{gz},{ax},{ay},{az},{wx},{wy},{wz},{:.3},{:.3},{:.3},{gps}\n",
        packet.sequence, m.forward, m.lateral, m.yaw
    )
}

/// Everything `status` reports.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub link: LinkState,
    pub phone: Option<String>,
    /// Which phone the receiver is looking for or following.
    pub target: Target,
    pub motion: Motion,
    /// Milliseconds since the last packet.
    pub age_ms: Option<u64>,
    /// Packets per second, smoothed.
    pub packets_per_s: f32,
    pub packets: u64,
    pub lost: u64,
    pub forward: ForwardState,
    /// The phone's GPS speed and the correction it applies; `None` while the forward motion is the accelerometer alone.
    pub gps: Option<SpeedStatus>,
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

    /// What `status` says about where the forward acceleration comes from.
    fn source_text(&self) -> String {
        match self.gps {
            Some(g) => format!("GPS speed {:.1} m/s ({:.0} km/h) guides it, accelerometer bias {:+.2} m/s2 removed", g.speed, g.speed * 3.6, -g.bias),
            None => "accelerometer only (the phone sends no GPS speed)".to_string(),
        }
    }

    /// Several lines, for `status`.
    pub fn report(&self) -> String {
        let link = match self.link {
            LinkState::Scanning => format!("scanning for {}", self.target.describe()),
            other => other.text().to_string(),
        };
        format!(
            "link:     {}\nphone:    {}\npackets:  {} received, {} lost, {:.1} per second\nmotion:   forward {:.2} m/s2, lateral {:.2} m/s2, yaw {:.2} rad/s\nforward:  {}\nspeed:    {}",
            link,
            self.phone.as_deref().unwrap_or("-"),
            self.packets,
            self.lost,
            self.packets_per_s,
            self.motion.forward,
            self.motion.lateral,
            self.motion.yaw,
            self.forward.text(),
            self.source_text(),
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
    /// Corrects the forward acceleration with the phone's GPS speed, when there is one.
    speed: SpeedAid,
    latest: Motion,
    last_packet: Option<Instant>,
    link: LinkState,
    enabled: bool,
    phone: Option<String>,
    /// The phone being connected to or connected.
    phone_candidate: Option<Candidate>,
    target: Target,
    /// Incremented by every [`Live::set_target`], so the receiver can tell its target changed.
    target_generation: u64,
    /// Phones heard while scanning, and when.
    sightings: Vec<(Candidate, Instant)>,
    last_sequence: Option<u16>,
    /// Time of the previous packet's first sample and its sample count, to measure the sample spacing.
    previous: Option<(u32, usize)>,
    packets: u64,
    lost: u64,
    /// Seconds between packets, smoothed.
    interval: Option<f32>,
    /// `--record`: every received sample and what the fusion made of it, as CSV.
    recording: Option<Box<dyn Write + Send>>,
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
                speed: SpeedAid::new(),
                latest: Motion::default(),
                last_packet: None,
                link: LinkState::Scanning,
                enabled: true,
                phone: None,
                phone_candidate: None,
                target: Target::any(),
                target_generation: 0,
                sightings: Vec::new(),
                last_sequence: None,
                previous: None,
                packets: 0,
                lost: 0,
                interval: None,
                recording: None,
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
        inner.speed.offer(packet.speed.map(|s| Fix { speed: s.speed(), time_ms: packet.first_sample_ms.wrapping_sub_signed(i32::from(s.age_ms)) }));
        for (i, sample) in packet.samples.iter().enumerate() {
            let mut motion = inner.fusion.process(packet.gravity_mg, sample, dt);
            let sample_ms = packet.first_sample_ms.wrapping_add((i as f32 * dt * 1000.0).round() as u32);
            motion.forward = inner.speed.process(sample_ms, motion.forward, dt);
            inner.latest = motion;
            let row = recording_row(packet, i, dt, motion, inner.speed.status());
            if let Some(out) = inner.recording.as_mut() {
                if out.write_all(row.as_bytes()).is_err() {
                    inner.recording = None;
                }
            }
        }
        if let Some(out) = inner.recording.as_mut() {
            let _ = out.flush();
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

    /// `--record`: from now on, writes every sample to `out` as CSV ([`RECORDING_HEADER`]).
    pub fn record_to(&self, mut out: Box<dyn Write + Send>) -> std::io::Result<()> {
        out.write_all(RECORDING_HEADER.as_bytes())?;
        out.flush()?;
        self.lock().recording = Some(out);
        Ok(())
    }

    /// The Bluetooth thread reports where the link stands. Anything but `Connected` means no motion.
    pub fn set_link(&self, link: LinkState) {
        let mut inner = self.lock();
        inner.link = link;
        inner.last_sequence = None;
        inner.previous = None;
        inner.interval = None;
        inner.speed.reset();
        if link != LinkState::Connected {
            inner.latest = Motion::default();
            inner.last_packet = None;
        }
        if matches!(link, LinkState::Scanning | LinkState::Paused) {
            inner.phone = None;
            inner.phone_candidate = None;
        }
    }

    pub fn is_connected(&self) -> bool {
        self.lock().link == LinkState::Connected
    }

    /// The phone being connected to (shown by `status`, marked by `phones`).
    pub fn set_phone(&self, phone: &Candidate) {
        let mut inner = self.lock();
        inner.phone = Some(format!("{} ({})", phone.label(), phone.address));
        inner.phone_candidate = Some(phone.clone());
    }

    /// `select`: follow another phone from now on. The receiver drops the current one and looks for this one.
    pub fn set_target(&self, target: Target) {
        let mut inner = self.lock();
        inner.target = target;
        inner.target_generation += 1;
        drop(inner);
        self.toggled.notify_waiters();
        self.toggled.notify_one();
    }

    /// The phone to follow, and a number that changes whenever `select` changes it.
    pub fn target(&self) -> (Target, u64) {
        let inner = self.lock();
        (inner.target.clone(), inner.target_generation)
    }

    /// The receiver reports the phones it can hear right now.
    pub fn report_sightings(&self, heard: &[Candidate], now: Instant) {
        let mut inner = self.lock();
        inner.sightings.retain(|(_, at)| now.saturating_duration_since(*at) <= SIGHTING_MAX_AGE);
        for candidate in heard {
            match inner.sightings.iter_mut().find(|(known, _)| known.address == candidate.address) {
                Some(entry) => *entry = (candidate.clone(), now),
                None => inner.sightings.push((candidate.clone(), now)),
            }
        }
    }

    /// The phones to offer in `phones` / `select`: heard in the last [`SIGHTING_MAX_AGE`] plus the connected one,
    /// one entry per Bluetooth name (a phone changes its address when its link restarts), strongest signal first.
    pub fn phones(&self, now: Instant) -> Vec<Candidate> {
        let inner = self.lock();
        let mut recent: Vec<&(Candidate, Instant)> =
            inner.sightings.iter().filter(|(_, at)| now.saturating_duration_since(*at) <= SIGHTING_MAX_AGE).collect();
        recent.sort_by_key(|(_, at)| std::cmp::Reverse(*at));
        let mut list: Vec<Candidate> = Vec::new();
        for (candidate, _) in recent {
            if candidate.name.is_none() || !list.iter().any(|c| c.name == candidate.name) {
                list.push(candidate.clone());
            }
        }
        if let Some(current) = &inner.phone_candidate {
            if !list.iter().any(|c| c.address == current.address || (c.name.is_some() && c.name == current.name)) {
                list.push(Candidate { rssi: None, ..current.clone() });
            }
        }
        list.sort_by_key(|c| std::cmp::Reverse(c.rssi.unwrap_or(i16::MIN)));
        list
    }

    /// Bluetooth name of the phone that is connected (or being connected to) right now.
    pub fn connected_name(&self) -> Option<String> {
        self.lock().phone_candidate.as_ref().and_then(|c| c.name.clone())
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
        let mut inner = self.lock();
        inner.fusion.relearn();
        inner.speed.reset(); // its speed estimate was integrated along the old forward direction
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
            target: inner.target.clone(),
            motion,
            age_ms: inner.last_packet.map(|at| now.saturating_duration_since(at).as_millis() as u64),
            packets_per_s: inner.interval.filter(|s| *s > 0.0).map_or(0.0, |s| 1.0 / s),
            packets: inner.packets,
            lost: inner.lost,
            forward: inner.fusion.forward_state(),
            gps: inner.speed.status(),
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
            speed: None,
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
    fn phone_and_replay_sources_answer_per_frame() {
        assert_eq!(MotionSource::Live(Live::new()).at(1.0).forward, 0.0, "no phone, no motion");
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

    /// A packet whose speed is the same fix as long as it lasts: one fix a second, `speed_cm_s` each time.
    fn packet_with_speed(sequence: u16, accel_y: i16, speed_cm_s: u16) -> Packet {
        let plain = packet(sequence, accel_y);
        let fix_ms = plain.first_sample_ms / 1000 * 1000;
        let age_ms = (plain.first_sample_ms - fix_ms) as i16;
        Packet { speed: Some(crate::protocol::SpeedFix { cm_s: speed_cm_s, age_ms }), ..plain }
    }

    #[test]
    fn the_gps_speed_corrects_the_forward_acceleration_and_is_reported() {
        // A steady 10 m/s while the accelerometer insists on 0.4 m/s²: after a while the dots see none of it.
        let (live, t0) = connected();
        for seq in 0..400u16 {
            live.push(&packet_with_speed(seq, 400, 1000), t0);
        }
        let m = live.current(t0);
        assert!(m.forward.abs() < 0.05, "{}", m.forward);
        let gps = live.status(t0).gps.expect("the speed is in use");
        assert!((gps.speed - 10.0).abs() < 1e-3 && (gps.bias - 0.4).abs() < 0.05, "{gps:?}");
        let report = live.status(t0).report();
        assert!(report.contains("GPS speed 10.0 m/s (36 km/h)"), "{report}");
    }

    #[test]
    fn without_a_speed_in_the_packets_the_accelerometer_alone_drives_the_dots() {
        let (live, t0) = connected();
        for seq in 0..400u16 {
            live.push(&packet(seq, 400), t0);
        }
        assert!((live.current(t0).forward - 0.4).abs() < 1e-4);
        assert_eq!(live.status(t0).gps, None);
        assert!(live.status(t0).report().contains("accelerometer only"));
    }

    #[test]
    fn a_reconnect_or_relearn_forgets_the_speed() {
        let (live, t0) = connected();
        for seq in 0..400u16 {
            live.push(&packet_with_speed(seq, 400, 1000), t0);
        }
        assert!(live.status(t0).gps.is_some());
        live.relearn();
        assert_eq!(live.status(t0).gps, None);
        live.push(&packet_with_speed(400, 400, 1000), t0);
        assert!(live.status(t0).gps.is_some());
        live.set_link(LinkState::Scanning);
        assert_eq!(live.status(t0).gps, None);
    }

    #[test]
    fn connect_and_disconnect_toggle_the_receiver() {
        let live = Live::new();
        assert!(live.is_enabled());
        live.set_enabled(false);
        assert!(!live.is_enabled());
    }

    fn phone(name: &str, address: &str, rssi: i16) -> Candidate {
        Candidate { name: Some(name.to_string()), address: address.to_string(), rssi: Some(rssi) }
    }

    #[test]
    fn selecting_a_phone_changes_the_target_and_wakes_the_receiver() {
        let live = Live::new();
        assert_eq!(live.target(), (Target::any(), 0));
        live.set_target(Target::named("Redmi"));
        assert_eq!(live.target(), (Target::named("Redmi"), 1));
        let waiter = live.clone();
        let woke = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
                tokio::time::timeout(Duration::from_secs(2), waiter.toggled()).await.is_ok()
            })
        });
        std::thread::sleep(Duration::from_millis(100));
        live.set_target(Target::named("Pixel"));
        assert!(woke.join().unwrap(), "set_target must wake a receiver that is waiting");
    }

    #[test]
    fn sightings_expire_and_one_phone_with_two_addresses_is_listed_once() {
        let live = Live::new();
        let t0 = Instant::now();
        live.report_sightings(&[phone("Redmi", "A1", -70), phone("Pixel", "B1", -50)], t0);
        live.report_sightings(&[phone("Redmi", "A2", -60)], t0 + Duration::from_secs(5));
        let names = |now| live.phones(now).iter().map(|c| format!("{}@{}", c.label(), c.address)).collect::<Vec<_>>();
        assert_eq!(names(t0 + Duration::from_secs(6)), ["Pixel@B1", "Redmi@A2"], "the newest address of Redmi wins");
        assert_eq!(names(t0 + SIGHTING_MAX_AGE + Duration::from_secs(1)), ["Redmi@A2"], "Pixel was last heard too long ago");
        assert!(names(t0 + Duration::from_secs(60)).is_empty());
    }
}

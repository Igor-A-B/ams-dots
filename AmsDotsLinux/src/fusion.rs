//! Turns the phone's raw readings into the vehicle's motion, and works out by
//! itself which way is forward.
//!
//! The phone sends acceleration and rotation in *its own* axes plus the
//! gravity direction. Here they become what the dots need, in the vehicle's
//! frame (the same conventions as `replay.rs`):
//!
//! - **up** is the gravity vector (Android reports it pointing up, away from the ground)
//! - **forward** is a fixed direction in the phone's frame (the *forward hint*),
//!   flattened onto the horizontal plane with the current `up`, so tilting the
//!   phone does not turn the vehicle axes
//! - **right** = forward x up
//!
//! Then `forward` = acceleration along forward, `lateral` = minus the
//! acceleration along right (a left turn pushes passengers right, so positive
//! lateral = pushed right), `yaw` = rotation about up (positive = turning left).
//!
//! # Learning "forward" (no calibration step)
//!
//! A phone has no velocity sensor, but two facts about cars give the direction
//! of travel from acceleration alone:
//!
//! 1. **The axis.** While driving straight (small yaw rate) the horizontal
//!    acceleration is mostly along the car: speeding up and braking. Summing the
//!    outer products of those accelerations gives a matrix whose strongest
//!    direction is the car's forward/backward axis. It is trusted once more than
//!    [`MIN_STRAIGHT_S`] seconds of movement have been seen and the direction is
//!    clear (`MIN_CONFIDENCE`).
//! 2. **The sign.** In a turn the centripetal acceleration points to the inside
//!    of the turn, so `acceleration x yaw rate` points to the car's *left*
//!    (`left = up x forward`) for a car going forward. Summed over turns this
//!    fixes which end of the axis is the front, and it can also give the whole
//!    direction when the car has only turned so far.
//!
//! Until there is evidence, the forward hint is a guess from the phone's pose
//! (top edge when it lies flat, back when it stands upright in a holder).
//! The sums fade with a time constant of [`MEMORY_S`], so the estimate follows
//! the phone if it is moved in its mount. A reversing car would teach the
//! opposite sign for a while; reversing is short and the sums outweigh it.

use crate::protocol::RawSample;

type V3 = [f32; 3];
type M3 = [[f32; 3]; 3];

/// Phone readings are mm/s², mrad/s and milli-g.
const MILLI: f32 = 0.001;

/// Gravity shorter than this (in g) is not a real reading; motion is reported as zero.
const MIN_GRAVITY: f32 = 0.3;
/// The forward hint must keep at least this much horizontal length (a unit vector)
/// or the phone lies in an orientation where it cannot define "forward".
const MIN_HORIZONTAL_HINT: f32 = 0.1;

/// Seconds of movement in a straight line needed before the axis is adopted.
pub const MIN_STRAIGHT_S: f32 = 5.0;
/// Seconds of turning needed before turns decide the direction.
const MIN_TURN_S: f32 = 3.0;
/// Yaw rate (rad/s) below which the car counts as going straight, above which as turning.
const STRAIGHT_YAW: f32 = 0.08;
const TURN_YAW: f32 = 0.12;
/// Horizontal acceleration (m/s²) that counts as movement; weaker is sensor noise and idle vibration.
const MOVING_MIN: f32 = 0.25;
/// Above this the reading is a pothole or a knock, not driving.
const MOVING_MAX: f32 = 8.0;
/// Share of the straight-line acceleration energy that must lie on one axis.
const MIN_CONFIDENCE: f32 = 0.8;
/// Share of the turn energy that must line up with the yaw rate for turns to be trusted.
const MIN_TURN_COHERENCE: f32 = 0.5;
/// A new estimate replaces the hint only if it differs by more than this (cosine of 5 degrees), to avoid jitter.
const HYSTERESIS_COS: f32 = 0.996;
/// Axis and turn evidence that disagree by more than this (cosine of 30 degrees) mean the axis is not trusted.
const AGREE_COS: f32 = 0.866;
/// How often (in samples) the estimate is recomputed.
const ESTIMATE_EVERY: u32 = 10;
/// The evidence fades with this time constant (seconds).
pub const MEMORY_S: f32 = 90.0;
/// Longest gap between samples that is integrated as such; a pause must not count as movement.
const MAX_DT: f32 = 0.2;

/// Vehicle motion at one instant.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Motion {
    /// m/s², positive = speeding up.
    pub forward: f32,
    /// m/s², positive = passengers pushed right.
    pub lateral: f32,
    /// rad/s, positive = turning left.
    pub yaw: f32,
}

/// How sure the app is about which way is forward, for `status`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ForwardState {
    /// No evidence yet: a guess from the phone's pose. `moving_s` is the straight-line movement seen so far.
    Guessing { moving_s: f32 },
    /// Learned from driving. `from_turns` tells whether turns confirmed the front from the back.
    Learned { confidence: f32, from_turns: bool },
}

pub struct Fusion {
    /// Forward direction in the phone's frame, or `None` until the first gravity reading guesses it.
    forward_hint: Option<V3>,
    learned: Option<(f32, bool)>,
    /// Sum of horizontal acceleration outer products over straight, moving samples.
    straight: M3,
    straight_s: f32,
    /// Sum of yaw rate times horizontal acceleration over turning samples: points left.
    turn: V3,
    /// Sum of |yaw rate| x |acceleration| over the same samples, to judge how coherent `turn` is.
    turn_energy: f32,
    turn_s: f32,
    since_estimate: u32,
}

impl Fusion {
    pub fn new() -> Self {
        Self {
            forward_hint: None,
            learned: None,
            straight: [[0.0; 3]; 3],
            straight_s: 0.0,
            turn: [0.0; 3],
            turn_energy: 0.0,
            turn_s: 0.0,
            since_estimate: 0,
        }
    }

    /// Forgets everything learned and guesses again from the phone's pose.
    /// For when the phone was moved and the old direction would mislead.
    pub fn relearn(&mut self) {
        *self = Self::new();
    }

    pub fn forward_state(&self) -> ForwardState {
        match self.learned {
            Some((confidence, from_turns)) => ForwardState::Learned { confidence, from_turns },
            None => ForwardState::Guessing { moving_s: self.straight_s },
        }
    }

    /// Converts one sample. `gravity_mg` is the packet's gravity in milli-g,
    /// `dt` the seconds this sample stands for (it weights what is learned).
    pub fn process(&mut self, gravity_mg: [i16; 3], sample: &RawSample, dt: f32) -> Motion {
        let gravity = gravity_mg.map(|v| f32::from(v) * MILLI);
        if norm(gravity) < MIN_GRAVITY {
            return Motion::default();
        }
        let up = normalize(gravity);
        let accel = sample.accel_mm_s2.map(|v| f32::from(v) * MILLI);
        let gyro = sample.gyro_mrad_s.map(|v| f32::from(v) * MILLI);
        let yaw = dot(gyro, up);

        self.learn(up, accel, yaw, dt.clamp(0.0, MAX_DT));

        let hint = *self.forward_hint.get_or_insert_with(|| guess_forward(up));
        let flat = sub(hint, scale(up, dot(hint, up)));
        if norm(flat) < MIN_HORIZONTAL_HINT {
            return Motion { yaw, ..Motion::default() };
        }
        let forward = normalize(flat);
        let right = cross(forward, up);
        Motion { forward: dot(accel, forward), lateral: -dot(accel, right), yaw }
    }

    fn learn(&mut self, up: V3, accel: V3, yaw: f32, dt: f32) {
        if dt <= 0.0 {
            return;
        }
        let fade = (-dt / MEMORY_S).exp();
        self.straight = mat_scale(self.straight, fade);
        self.straight_s *= fade;
        self.turn = scale(self.turn, fade);
        self.turn_energy *= fade;
        self.turn_s *= fade;

        let horizontal = sub(accel, scale(up, dot(accel, up)));
        let strength = norm(horizontal);
        if strength < MOVING_MAX {
            if yaw.abs() < STRAIGHT_YAW && strength > MOVING_MIN {
                self.straight = mat_add(self.straight, mat_scale(outer(horizontal), dt));
                self.straight_s += dt;
            } else if yaw.abs() > TURN_YAW {
                self.turn = add(self.turn, scale(horizontal, yaw * dt));
                self.turn_energy += yaw.abs() * strength * dt;
                self.turn_s += dt;
            }
        }

        self.since_estimate += 1;
        if self.since_estimate >= ESTIMATE_EVERY {
            self.since_estimate = 0;
            self.update_hint(up);
        }
    }

    /// Recomputes the forward direction from the evidence and adopts it if it is clear and different enough.
    fn update_hint(&mut self, up: V3) {
        let horizontal = |v: V3| sub(v, scale(up, dot(v, up)));

        // Turns: `turn` points left, so forward = left x up.
        let from_turn = (self.turn_s >= MIN_TURN_S && norm(self.turn) >= MIN_TURN_COHERENCE * self.turn_energy)
            .then(|| horizontal(self.turn))
            .filter(|left| norm(*left) > 1e-6)
            .map(|left| cross(normalize(left), up))
            .map(normalize);

        // Straight driving: the strongest horizontal direction, if it is clear.
        let axis = (self.straight_s > MIN_STRAIGHT_S)
            .then(|| strongest_direction(&self.straight, self.forward_hint.unwrap_or([0.0, 1.0, 0.0])))
            .filter(|(_, confidence)| *confidence >= MIN_CONFIDENCE)
            .map(|(direction, confidence)| (normalize(horizontal(direction)), confidence));

        let (target, confidence, from_turns) = match (axis, from_turn) {
            (Some((u, confidence)), Some(t)) if dot(u, t).abs() >= AGREE_COS => (if dot(u, t) >= 0.0 { u } else { scale(u, -1.0) }, confidence, true),
            // The two disagree: turns tie the direction to the yaw rate directly, so they win.
            (Some(_), Some(t)) => (t, 0.5, true),
            (Some((u, confidence)), None) => {
                // Axis known, front unknown: keep the end that agrees with what is in use now.
                let current = self.forward_hint.unwrap_or(u);
                (if dot(u, current) >= 0.0 { u } else { scale(u, -1.0) }, confidence, false)
            }
            (None, Some(t)) => (t, 0.5, true),
            (None, None) => return,
        };

        let changed = self.forward_hint.is_none_or(|h| dot(normalize(horizontal(h)), target) < HYSTERESIS_COS);
        if changed || self.learned.is_none() {
            self.forward_hint = Some(target);
        }
        self.learned = Some((confidence, from_turns));
    }

    /// The forward direction in the phone's frame.
    #[cfg(test)]
    pub fn forward_hint(&self) -> Option<V3> {
        self.forward_hint
    }
}

/// Picks the likelier "forward" for a phone with no evidence yet: whichever of
/// the top edge (+Y) and the back (-Z) lies more in the horizontal plane.
fn guess_forward(up: V3) -> V3 {
    let top = [0.0, 1.0, 0.0];
    let back = [0.0, 0.0, -1.0];
    if norm(sub(top, scale(up, dot(top, up)))) >= norm(sub(back, scale(up, dot(back, up)))) { top } else { back }
}

/// Strongest direction of a symmetric positive matrix and the share of the
/// total energy (trace) on it, by power iteration from `start`.
fn strongest_direction(m: &M3, start: V3) -> (V3, f32) {
    let trace = m[0][0] + m[1][1] + m[2][2];
    if trace <= 1e-9 {
        return (start, 0.0);
    }
    let mut v = if norm(start) > 1e-6 { normalize(start) } else { [0.0, 1.0, 0.0] };
    for _ in 0..30 {
        let next = mat_vec(m, v);
        if norm(next) < 1e-12 {
            break;
        }
        v = normalize(next);
    }
    (v, dot(v, mat_vec(m, v)) / trace)
}

fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: V3, k: f32) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

fn norm(a: V3) -> f32 {
    dot(a, a).sqrt()
}

fn normalize(a: V3) -> V3 {
    scale(a, 1.0 / norm(a))
}

fn outer(a: V3) -> M3 {
    [scale(a, a[0]), scale(a, a[1]), scale(a, a[2])]
}

fn mat_add(a: M3, b: M3) -> M3 {
    [add(a[0], b[0]), add(a[1], b[1]), add(a[2], b[2])]
}

fn mat_scale(a: M3, k: f32) -> M3 {
    [scale(a[0], k), scale(a[1], k), scale(a[2], k)]
}

fn mat_vec(m: &M3, v: V3) -> V3 {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay;

    const FLAT: [i16; 3] = [0, 0, 1000];
    const UPRIGHT: [i16; 3] = [0, 1000, 0];
    const DT: f32 = 0.02;

    fn sample(accel: [i16; 3], gyro: [i16; 3]) -> RawSample {
        RawSample { accel_mm_s2: accel, gyro_mrad_s: gyro }
    }

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    fn process(f: &mut Fusion, gravity: [i16; 3], accel: [i16; 3], gyro: [i16; 3]) -> Motion {
        f.process(gravity, &sample(accel, gyro), DT)
    }

    // ---- conversion to vehicle axes (the phone lies flat or stands upright; nothing learned yet) ----

    #[test]
    fn a_flat_phone_with_its_top_forward_reads_forward_acceleration() {
        let m = process(&mut Fusion::new(), FLAT, [0, 2000, 0], [0; 3]);
        assert!(approx(m.forward, 2.0) && approx(m.lateral, 0.0), "{m:?}");
    }

    #[test]
    fn acceleration_to_the_right_pushes_passengers_left() {
        // Flat, top forward: +X is the vehicle's right. Speeding up to the right = lateral negative.
        let m = process(&mut Fusion::new(), FLAT, [3000, 0, 0], [0; 3]);
        assert!(approx(m.lateral, -3.0) && approx(m.forward, 0.0), "{m:?}");
        let left = process(&mut Fusion::new(), FLAT, [-3000, 0, 0], [0; 3]);
        assert!(approx(left.lateral, 3.0), "{left:?}");
    }

    #[test]
    fn an_upright_phone_uses_its_back_as_forward() {
        let m = process(&mut Fusion::new(), UPRIGHT, [0, 0, -2000], [0; 3]);
        assert!(approx(m.forward, 2.0), "{m:?}");
        // In a holder facing the driver the phone's +X is the vehicle's right: speeding up that way is lateral negative.
        let m = process(&mut Fusion::new(), UPRIGHT, [2000, 0, 0], [0; 3]);
        assert!(approx(m.lateral, -2.0), "{m:?}");
    }

    #[test]
    fn turning_left_is_positive_yaw_whatever_the_pose() {
        let flat = process(&mut Fusion::new(), FLAT, [0; 3], [0, 0, 500]);
        assert!(approx(flat.yaw, 0.5), "{flat:?}");
        // Upright: up is +Y, so the same turn shows as rotation about +Y.
        let upright = process(&mut Fusion::new(), UPRIGHT, [0; 3], [0, 500, 0]);
        assert!(approx(upright.yaw, 0.5), "{upright:?}");
    }

    #[test]
    fn gravity_is_not_part_of_the_motion() {
        let m = process(&mut Fusion::new(), FLAT, [0, 0, 5000], [0; 3]);
        assert!(approx(m.forward, 0.0) && approx(m.lateral, 0.0), "{m:?}");
    }

    #[test]
    fn missing_gravity_gives_no_motion() {
        let m = process(&mut Fusion::new(), [0, 0, 0], [5000, 5000, 5000], [500; 3]);
        assert_eq!(m, Motion::default());
    }

    #[test]
    fn the_guess_does_not_flip_when_the_phone_is_tilted_later() {
        let mut f = Fusion::new();
        process(&mut f, FLAT, [0; 3], [0; 3]); // locks +Y as forward
        // Tilted 60 degrees towards upright: -Z would now be the likelier guess, but +Y stays.
        let m = process(&mut f, [0, 866, 500], [0, 1000, 0], [0; 3]);
        assert!(m.forward > 0.0, "{m:?}");
    }

    // ---- learning forward from a simulated drive ----

    /// Rotation matrices: a vector given in the vehicle's frame (x right, y forward, z up)
    /// becomes the phone's frame when multiplied with the matrix.
    fn rotation(about_z: f32, about_x: f32, about_y: f32) -> M3 {
        let (sz, cz) = about_z.sin_cos();
        let (sx, cx) = about_x.sin_cos();
        let (sy, cy) = about_y.sin_cos();
        let rz = [[cz, -sz, 0.0], [sz, cz, 0.0], [0.0, 0.0, 1.0]];
        let rx = [[1.0, 0.0, 0.0], [0.0, cx, -sx], [0.0, sx, cx]];
        let ry = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
        let mul = |a: M3, b: M3| -> M3 {
            let col = |j: usize| [b[0][j], b[1][j], b[2][j]];
            let mut out = [[0.0; 3]; 3];
            for (i, row) in out.iter_mut().enumerate() {
                for (j, cell) in row.iter_mut().enumerate() {
                    *cell = dot(a[i], col(j));
                }
            }
            out
        };
        mul(rz, mul(rx, ry))
    }

    /// Feeds the bundled mock drive, as a phone fixed in the vehicle with orientation `m` would report it,
    /// repeated `loops` times, with a little noise. Returns the fusion and the last 200 outputs
    /// next to what the recording says they should be.
    fn drive(m: M3, loops: usize, seconds_limit: Option<f32>) -> (Fusion, Vec<(Motion, Motion)>) {
        let recording = replay::load("tests/data/drive.csv").unwrap();
        let mut f = Fusion::new();
        let gravity = mat_vec(&m, [0.0, 0.0, 1000.0]).map(|v| v.round() as i16);
        let mut seed = 12345u32;
        let mut noise = move || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            ((seed >> 16) as f32 / 65535.0 - 0.5) * 200.0 // +-0.1 m/s² in mm/s²
        };
        let steps = (loops as f32 * 30.0 / DT) as usize;
        let steps = seconds_limit.map_or(steps, |s| steps.min((s / DT) as usize));
        let mut last = Vec::new();
        for i in 0..steps {
            let truth = replay::at(&recording, i as f32 * DT);
            let accel_v = [-truth.lateral * 1000.0 + noise(), truth.forward * 1000.0 + noise(), noise()];
            let gyro_v = [0.0, 0.0, truth.yaw * 1000.0];
            let accel = mat_vec(&m, accel_v).map(|v| v.round() as i16);
            let gyro = mat_vec(&m, gyro_v).map(|v| v.round() as i16);
            let out = f.process(gravity, &sample(accel, gyro), DT);
            if i + 200 >= steps {
                last.push((out, Motion { forward: truth.forward, lateral: truth.lateral, yaw: truth.yaw }));
            }
        }
        (f, last)
    }

    fn angle_degrees(a: V3, b: V3) -> f32 {
        (dot(a, b) / (norm(a) * norm(b))).clamp(-1.0, 1.0).acos().to_degrees()
    }

    fn assert_learned_forward(m: M3, name: &str) {
        let (f, last) = drive(m, 3, None);
        let truth = mat_vec(&m, [0.0, 1.0, 0.0]);
        let hint = f.forward_hint().unwrap();
        let error = angle_degrees(hint, truth);
        assert!(error < 8.0, "{name}: forward is {error:.1} degrees off");
        assert!(matches!(f.forward_state(), ForwardState::Learned { from_turns: true, .. }), "{name}: {:?}", f.forward_state());
        for (out, expected) in last {
            assert!((out.forward - expected.forward).abs() < 0.5, "{name}: forward {out:?} vs {expected:?}");
            assert!((out.lateral - expected.lateral).abs() < 0.5, "{name}: lateral {out:?} vs {expected:?}");
            assert!((out.yaw - expected.yaw).abs() < 0.05, "{name}: yaw {out:?} vs {expected:?}");
        }
    }

    #[test]
    fn learns_forward_with_the_phone_flat_top_forward() {
        assert_learned_forward(rotation(0.0, 0.0, 0.0), "flat, top forward");
    }

    #[test]
    fn learns_forward_with_the_phone_flat_pointing_sideways_or_backwards() {
        // The guess (+Y) is wrong for these, so the result comes from driving alone.
        assert_learned_forward(rotation(std::f32::consts::FRAC_PI_2, 0.0, 0.0), "flat, top to the left");
        assert_learned_forward(rotation(std::f32::consts::PI, 0.0, 0.0), "flat, top backwards");
        assert_learned_forward(rotation(-std::f32::consts::FRAC_PI_2, 0.0, 0.0), "flat, top to the right");
    }

    #[test]
    fn learns_forward_with_the_phone_upright_in_a_holder() {
        assert_learned_forward(rotation(0.0, std::f32::consts::FRAC_PI_2, 0.0), "upright");
    }

    #[test]
    fn learns_forward_with_the_phone_tilted_and_turned() {
        assert_learned_forward(rotation(0.7, 0.4, -0.3), "tilted a");
        assert_learned_forward(rotation(-2.1, 1.1, 0.5), "tilted b");
        assert_learned_forward(rotation(2.6, -0.8, 0.9), "tilted c");
    }

    #[test]
    fn nothing_is_learned_before_five_seconds_of_movement() {
        // The first 4 s of the drive are only 3 s of acceleration.
        let m = rotation(std::f32::consts::FRAC_PI_2, 0.0, 0.0);
        let (f, _) = drive(m, 1, Some(4.5));
        assert!(matches!(f.forward_state(), ForwardState::Guessing { .. }), "{:?}", f.forward_state());
        assert_eq!(f.forward_hint(), Some([0.0, 1.0, 0.0]), "still the pose guess");
    }

    #[test]
    fn straight_driving_alone_finds_the_axis_and_keeps_the_guessed_end() {
        // Speed up and brake in a straight line, no turns: the axis is learned, the front end is not confirmed.
        let mut f = Fusion::new();
        for round in 0..6 {
            let sign = if round % 2 == 0 { 2000 } else { -2000 };
            for _ in 0..150 {
                process(&mut f, FLAT, [0, sign, 0], [0; 3]);
            }
        }
        match f.forward_state() {
            ForwardState::Learned { from_turns: false, confidence } => assert!(confidence > 0.9),
            other => panic!("{other:?}"),
        }
        assert!(angle_degrees(f.forward_hint().unwrap(), [0.0, 1.0, 0.0]) < 1.0);
    }

    #[test]
    fn turning_alone_finds_forward() {
        // Phone flat, vehicle forward = device +X, so left = up x forward = device +Y.
        // A left turn has yaw > 0 and centripetal acceleration to the left.
        let mut f = Fusion::new();
        for _ in 0..200 {
            process(&mut f, FLAT, [0, 3000, 0], [0, 0, 400]);
        }
        let hint = f.forward_hint().unwrap();
        assert!(angle_degrees(hint, [1.0, 0.0, 0.0]) < 2.0, "{hint:?}");
    }

    #[test]
    fn a_knock_or_pothole_is_not_driving() {
        let mut f = Fusion::new();
        for _ in 0..1000 {
            process(&mut f, FLAT, [0, 20_000, 0], [0; 3]);
        }
        assert!(matches!(f.forward_state(), ForwardState::Guessing { moving_s } if moving_s == 0.0));
    }

    #[test]
    fn noise_alone_learns_nothing() {
        let mut f = Fusion::new();
        for i in 0..5000 {
            let wobble = if i % 2 == 0 { 100 } else { -100 };
            process(&mut f, FLAT, [wobble, wobble, 0], [0; 3]);
        }
        assert!(matches!(f.forward_state(), ForwardState::Guessing { .. }));
    }

    #[test]
    fn relearn_forgets_what_was_learned() {
        let (mut f, _) = drive(rotation(std::f32::consts::FRAC_PI_2, 0.0, 0.0), 2, None);
        assert!(matches!(f.forward_state(), ForwardState::Learned { .. }));
        f.relearn();
        assert!(matches!(f.forward_state(), ForwardState::Guessing { moving_s } if moving_s == 0.0));
    }

    #[test]
    fn pauses_in_the_data_are_not_counted_as_movement() {
        let mut f = Fusion::new();
        // 20 samples that each claim a 10 s gap would be 200 s of "movement" if taken at face value.
        for _ in 0..20 {
            f.process(FLAT, &sample([0, 2000, 0], [0; 3]), 10.0);
        }
        assert!(matches!(f.forward_state(), ForwardState::Guessing { moving_s } if moving_s < MIN_STRAIGHT_S));
    }
}

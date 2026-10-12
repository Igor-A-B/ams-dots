//! Turns the phone's raw readings into the vehicle's motion.
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
//! Without calibration the forward hint is guessed from the phone's pose: the
//! top edge (+Y) when it lies flat, the back (-Z) when it stands upright in a
//! holder. [`Fusion::calibrate`] replaces the guess with the real direction:
//! press it, then accelerate gently in a straight line.

use crate::protocol::RawSample;

type V3 = [f32; 3];

/// Phone readings are mm/s², mrad/s and milli-g.
const MILLI: f32 = 0.001;

/// Gravity shorter than this (in g) is not a real reading; motion is reported as zero.
const MIN_GRAVITY: f32 = 0.3;
/// The forward hint must keep at least this much horizontal length (a unit vector)
/// or the phone lies in an orientation where it cannot define "forward".
const MIN_HORIZONTAL_HINT: f32 = 0.1;

/// Calibration waits until the horizontal acceleration exceeds this (m/s²): the car is starting off.
const START_THRESHOLD: f32 = 1.0;
/// Then averages this many samples (1.5 s at 50 Hz).
const COLLECT_SAMPLES: usize = 75;
/// The average must reach this (m/s²) or the push was too weak or not straight to trust.
const MIN_MEAN: f32 = 0.5;
/// An armed calibration that sees no start gives up after this many samples.
const ARM_TIMEOUT_SAMPLES: usize = 3000;

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

/// What a calibration did; reported once through [`Fusion::take_event`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CalibrationEvent {
    /// The forward direction was set; the number is the mean horizontal push in m/s².
    Done { strength: f32 },
    /// The push was too weak or too uneven; nothing changed.
    TooWeak,
    /// No acceleration was seen in time; nothing changed.
    TimedOut,
}

enum Calibration {
    Idle,
    /// Waiting for the vehicle to start accelerating.
    Armed { waited: usize },
    /// Summing horizontal acceleration vectors.
    Collecting { sum: V3, count: usize },
}

pub struct Fusion {
    /// Forward direction in the phone's frame, or `None` until the first gravity reading guesses it.
    forward_hint: Option<V3>,
    calibration: Calibration,
    event: Option<CalibrationEvent>,
}

impl Fusion {
    pub fn new() -> Self {
        Self { forward_hint: None, calibration: Calibration::Idle, event: None }
    }

    /// Starts a calibration: the next clear acceleration defines "forward".
    pub fn calibrate(&mut self) {
        self.calibration = Calibration::Armed { waited: 0 };
        self.event = None;
    }

    /// The outcome of the last calibration, once; `None` while it is still running or nothing new happened.
    pub fn take_event(&mut self) -> Option<CalibrationEvent> {
        self.event.take()
    }

    /// Converts one sample. `gravity_mg` is the packet's gravity in milli-g.
    pub fn process(&mut self, gravity_mg: [i16; 3], sample: &RawSample) -> Motion {
        let gravity = gravity_mg.map(|v| f32::from(v) * MILLI);
        if norm(gravity) < MIN_GRAVITY {
            return Motion::default();
        }
        let up = normalize(gravity);
        let accel = sample.accel_mm_s2.map(|v| f32::from(v) * MILLI);
        let gyro = sample.gyro_mrad_s.map(|v| f32::from(v) * MILLI);

        self.advance_calibration(up, accel);

        let yaw = dot(gyro, up);
        let hint = *self.forward_hint.get_or_insert_with(|| guess_forward(up));
        let flat = sub(hint, scale(up, dot(hint, up)));
        if norm(flat) < MIN_HORIZONTAL_HINT {
            return Motion { yaw, ..Motion::default() };
        }
        let forward = normalize(flat);
        let right = cross(forward, up);
        Motion { forward: dot(accel, forward), lateral: -dot(accel, right), yaw }
    }

    fn advance_calibration(&mut self, up: V3, accel: V3) {
        let horizontal = sub(accel, scale(up, dot(accel, up)));
        match &mut self.calibration {
            Calibration::Idle => {}
            Calibration::Armed { waited } => {
                *waited += 1;
                if norm(horizontal) >= START_THRESHOLD {
                    self.calibration = Calibration::Collecting { sum: horizontal, count: 1 };
                } else if *waited >= ARM_TIMEOUT_SAMPLES {
                    self.calibration = Calibration::Idle;
                    self.event = Some(CalibrationEvent::TimedOut);
                }
            }
            Calibration::Collecting { sum, count } => {
                *sum = add(*sum, horizontal);
                *count += 1;
                if *count >= COLLECT_SAMPLES {
                    let mean = norm(*sum) / *count as f32;
                    if mean >= MIN_MEAN {
                        self.forward_hint = Some(normalize(*sum));
                        self.event = Some(CalibrationEvent::Done { strength: mean });
                    } else {
                        self.event = Some(CalibrationEvent::TooWeak);
                    }
                    self.calibration = Calibration::Idle;
                }
            }
        }
    }
}

/// Picks the likelier "forward" for an uncalibrated phone: whichever of the top
/// edge (+Y) and the back (-Z) lies more in the horizontal plane.
fn guess_forward(up: V3) -> V3 {
    let top = [0.0, 1.0, 0.0];
    let back = [0.0, 0.0, -1.0];
    if norm(sub(top, scale(up, dot(top, up)))) >= norm(sub(back, scale(up, dot(back, up)))) { top } else { back }
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

#[cfg(test)]
mod tests {
    use super::*;

    const FLAT: [i16; 3] = [0, 0, 1000];
    const UPRIGHT: [i16; 3] = [0, 1000, 0];

    fn sample(accel: [i16; 3], gyro: [i16; 3]) -> RawSample {
        RawSample { accel_mm_s2: accel, gyro_mrad_s: gyro }
    }

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn a_flat_phone_with_its_top_forward_reads_forward_acceleration() {
        let m = Fusion::new().process(FLAT, &sample([0, 2000, 0], [0; 3]));
        assert!(approx(m.forward, 2.0) && approx(m.lateral, 0.0), "{m:?}");
    }

    #[test]
    fn acceleration_to_the_right_pushes_passengers_left() {
        // Flat, top forward: +X is the vehicle's right. Speeding up to the right = lateral negative.
        let m = Fusion::new().process(FLAT, &sample([3000, 0, 0], [0; 3]));
        assert!(approx(m.lateral, -3.0) && approx(m.forward, 0.0), "{m:?}");
        let left = Fusion::new().process(FLAT, &sample([-3000, 0, 0], [0; 3]));
        assert!(approx(left.lateral, 3.0), "{left:?}");
    }

    #[test]
    fn an_upright_phone_uses_its_back_as_forward() {
        let m = Fusion::new().process(UPRIGHT, &sample([0, 0, -2000], [0; 3]));
        assert!(approx(m.forward, 2.0), "{m:?}");
        // In a holder facing the driver the phone's +X is the vehicle's right: speeding up that way is lateral negative.
        let m = Fusion::new().process(UPRIGHT, &sample([2000, 0, 0], [0; 3]));
        assert!(approx(m.lateral, -2.0), "{m:?}");
    }

    #[test]
    fn turning_left_is_positive_yaw_whatever_the_pose() {
        // Flat: rotation about +Z (up) is counter-clockwise seen from above = left.
        let flat = Fusion::new().process(FLAT, &sample([0; 3], [0, 0, 500]));
        assert!(approx(flat.yaw, 0.5), "{flat:?}");
        // Upright: up is +Y, so the same turn shows as rotation about +Y.
        let upright = Fusion::new().process(UPRIGHT, &sample([0; 3], [0, 500, 0]));
        assert!(approx(upright.yaw, 0.5), "{upright:?}");
    }

    #[test]
    fn gravity_is_not_part_of_the_motion() {
        let m = Fusion::new().process(FLAT, &sample([0, 0, 5000], [0; 3]));
        assert!(approx(m.forward, 0.0) && approx(m.lateral, 0.0), "{m:?}");
    }

    #[test]
    fn missing_gravity_gives_no_motion() {
        let m = Fusion::new().process([0, 0, 0], &sample([5000, 5000, 5000], [500; 3]));
        assert_eq!(m, Motion::default());
    }

    #[test]
    fn the_guess_does_not_flip_when_the_phone_is_tilted_later() {
        let mut f = Fusion::new();
        f.process(FLAT, &sample([0; 3], [0; 3])); // locks +Y as forward
        // Tilted 60 degrees towards upright: -Z would now be the likelier guess, but +Y stays.
        let m = f.process([0, 866, 500], &sample([0, 1000, 0], [0; 3]));
        // +Y flattened against up (0, .866, .5): y-component along the horizontal is 0.5 of 1.
        assert!(m.forward > 0.0, "{m:?}");
    }

    fn feed(f: &mut Fusion, count: usize, accel: [i16; 3]) {
        for _ in 0..count {
            f.process(FLAT, &sample(accel, [0; 3]));
        }
    }

    #[test]
    fn calibration_sets_forward_to_the_direction_of_the_push() {
        let mut f = Fusion::new();
        f.calibrate();
        feed(&mut f, 10, [100, 0, 0]); // quiet: still waiting
        assert_eq!(f.take_event(), None);
        feed(&mut f, COLLECT_SAMPLES, [2000, 0, 0]); // the phone's +X is forward
        assert!(matches!(f.take_event(), Some(CalibrationEvent::Done { .. })));
        let m = f.process(FLAT, &sample([2000, 0, 0], [0; 3]));
        assert!(approx(m.forward, 2.0) && approx(m.lateral, 0.0), "{m:?}");
        // The old default (+Y) is now sideways: +Y is the vehicle's left here.
        let m = f.process(FLAT, &sample([0, 2000, 0], [0; 3]));
        assert!(approx(m.lateral, 2.0), "{m:?}");
    }

    #[test]
    fn the_first_push_defines_forward_even_if_it_is_braking() {
        // Documented behaviour: whatever the first push is, that is "forward".
        let mut f = Fusion::new();
        f.calibrate();
        feed(&mut f, COLLECT_SAMPLES, [0, -2000, 0]);
        let m = f.process(FLAT, &sample([0, -2000, 0], [0; 3]));
        assert!(approx(m.forward, 2.0), "{m:?}");
    }

    #[test]
    fn a_weak_or_uneven_push_changes_nothing() {
        let mut f = Fusion::new();
        f.calibrate();
        // One jolt that starts it, then silence: the mean stays under the threshold.
        feed(&mut f, 1, [1500, 0, 0]);
        feed(&mut f, COLLECT_SAMPLES, [0; 3]);
        assert_eq!(f.take_event(), Some(CalibrationEvent::TooWeak));
        let m = f.process(FLAT, &sample([0, 2000, 0], [0; 3]));
        assert!(approx(m.forward, 2.0), "default forward must be kept: {m:?}");
    }

    #[test]
    fn calibration_gives_up_without_a_push() {
        let mut f = Fusion::new();
        f.calibrate();
        feed(&mut f, ARM_TIMEOUT_SAMPLES, [0; 3]);
        assert_eq!(f.take_event(), Some(CalibrationEvent::TimedOut));
    }

    #[test]
    fn calibration_ignores_the_vertical_part_of_the_push() {
        let mut f = Fusion::new();
        f.calibrate();
        // A bumpy road adds vertical acceleration (along up = +Z) that must not tilt "forward".
        feed(&mut f, COLLECT_SAMPLES, [0, 2000, 3000]);
        let m = f.process(FLAT, &sample([0, 2000, 0], [0; 3]));
        assert!(approx(m.forward, 2.0), "{m:?}");
    }
}

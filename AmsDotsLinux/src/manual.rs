//! Manual control of the dots: a direction and a speed set by hand (`ams-dots-linux move`, `speed`,
//! `direction`), for use without the phone or to try the dots out. While it is on it replaces whatever the
//! phone or a recording says; `auto` hands control back.
//!
//! The direction is in degrees clockwise from the top of the screen (0 = up, 90 = right, 180 = down, 270 = left)
//! and says where the dots flow. The speed is a percentage of `motion.max_speed`, the fastest the dots ever drift.
//!
//! The setting is turned into the same (lateral, forward) acceleration the phone would produce, so the spring
//! smoothing and the "more motion, more dots" rule apply unchanged.

use crate::config::MotionConfig;
use crate::replay::Sample;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// Where the dots flow when manual control is switched on by `speed` alone: down, as when speeding up.
pub const DEFAULT_DIRECTION: f32 = 180.0;
/// Speed (percent of the maximum) used until one is given.
pub const DEFAULT_SPEED: f32 = 50.0;

/// A value given absolutely (`90`) or as a change to the current one (`+15`, `-10`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Adjust {
    To(f32),
    By(f32),
}

impl Adjust {
    fn apply(self, current: f32) -> f32 {
        match self {
            Adjust::To(value) => value,
            Adjust::By(delta) => current + delta,
        }
    }
}

/// The eight named directions, as degrees clockwise from the top.
const NAMED: [(&str, f32); 8] = [
    ("up", 0.0),
    ("up-right", 45.0),
    ("right", 90.0),
    ("down-right", 135.0),
    ("down", 180.0),
    ("down-left", 225.0),
    ("left", 270.0),
    ("up-left", 315.0),
];

/// Name of `degrees` if it is one of the eight named directions.
pub fn direction_name(degrees: f32) -> Option<&'static str> {
    NAMED.iter().find(|(_, d)| (degrees - d).abs() < 0.05).map(|(name, _)| *name)
}

fn number(text: &str) -> Option<f32> {
    text.parse::<f32>().ok().filter(|v| v.is_finite())
}

/// `down`, `up-left`, `90`, `45.5`, `+15` (turn 15° clockwise), `-15` (counterclockwise).
pub fn parse_direction(text: &str) -> Result<Adjust, String> {
    let text = text.trim().to_lowercase();
    let squashed = text.replace(['-', '_'], "");
    if let Some((_, degrees)) = NAMED.iter().find(|(name, _)| name.replace('-', "") == squashed) {
        return Ok(Adjust::To(*degrees));
    }
    let digits = text.trim_end_matches('°').trim_end_matches("deg");
    let value = number(digits).ok_or_else(|| {
        format!("{text:?} is not a direction: use up, down, left, right, up-left, up-right, down-left, down-right, or degrees (0 = up, 90 = right, 180 = down)")
    })?;
    Ok(if digits.starts_with(['+', '-']) { Adjust::By(value) } else { Adjust::To(value) })
}

/// `50`, `50%`, `+10`, `-10`: percent of the maximum speed.
pub fn parse_speed(text: &str) -> Result<Adjust, String> {
    let text = text.trim();
    let digits = text.trim_end_matches('%');
    let value = number(digits).ok_or_else(|| format!("{text:?} is not a speed: use a percentage such as 50, or +10 / -10 to change it"))?;
    Ok(if digits.starts_with(['+', '-']) { Adjust::By(value) } else { Adjust::To(value) })
}

/// What the dots are told to do by hand.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Setting {
    /// Degrees clockwise from the top of the screen, in `0..360`.
    pub direction: f32,
    /// Percent of `motion.max_speed`, in `0..=100`.
    pub speed: f32,
}

impl Setting {
    /// "down at 50% (125 px/s)".
    pub fn describe(&self, motion: &MotionConfig) -> String {
        let direction = direction_name(self.direction).map_or_else(|| format!("{:.0}°", self.direction), str::to_string);
        format!("{direction} at {:.0}% ({:.0} px/s)", self.speed, self.speed / 100.0 * motion.max_speed)
    }
}

struct Inner {
    active: bool,
    /// Kept while inactive, so `speed 30` after `auto` resumes in the last direction.
    setting: Setting,
}

/// Shared between the command socket (writes) and the overlay (reads once per frame).
pub struct Manual(Mutex<Inner>);

impl Manual {
    pub fn new() -> Arc<Self> {
        Arc::new(Self(Mutex::new(Inner {
            active: false,
            setting: Setting { direction: DEFAULT_DIRECTION, speed: DEFAULT_SPEED },
        })))
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Changes direction and/or speed and switches manual control on. Directions wrap around, speeds stop at 0 and 100.
    pub fn set(&self, direction: Option<Adjust>, speed: Option<Adjust>) -> Setting {
        let mut inner = self.lock();
        if let Some(direction) = direction {
            inner.setting.direction = direction.apply(inner.setting.direction).rem_euclid(360.0);
        }
        if let Some(speed) = speed {
            inner.setting.speed = speed.apply(inner.setting.speed).clamp(0.0, 100.0);
        }
        inner.active = true;
        inner.setting
    }

    /// `auto`: the phone (or recording) drives the dots again. Returns whether manual control was on.
    pub fn clear(&self) -> bool {
        std::mem::replace(&mut self.lock().active, false)
    }

    /// The setting, while manual control is on.
    pub fn current(&self) -> Option<Setting> {
        let inner = self.lock();
        inner.active.then_some(inner.setting)
    }

    /// The motion that makes the dots flow as set, while manual control is on.
    /// The overlay turns acceleration into drift with `flow_gain`, so dividing by it gives exactly `speed` of
    /// `max_speed` in the chosen direction.
    pub fn sample(&self, elapsed: f32, motion: &MotionConfig) -> Option<Sample> {
        let setting = self.current()?;
        let drift = setting.speed / 100.0 * motion.max_speed;
        let acceleration = if motion.flow_gain > 0.0 { drift / motion.flow_gain } else { 0.0 };
        let angle = setting.direction.to_radians();
        // Screen y grows downwards and "forward" pushes the dots down, so up is negative forward.
        Some(Sample { t: elapsed, forward: -angle.cos() * acceleration, lateral: angle.sin() * acceleration, yaw: 0.0 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn directions_by_name_and_by_degrees() {
        assert_eq!(parse_direction("down"), Ok(Adjust::To(180.0)));
        assert_eq!(parse_direction("Up-Left"), Ok(Adjust::To(315.0)));
        assert_eq!(parse_direction("downright"), Ok(Adjust::To(135.0)));
        assert_eq!(parse_direction("90"), Ok(Adjust::To(90.0)));
        assert_eq!(parse_direction("45.5°"), Ok(Adjust::To(45.5)));
        assert_eq!(parse_direction("+15"), Ok(Adjust::By(15.0)));
        assert_eq!(parse_direction("-15"), Ok(Adjust::By(-15.0)));
        assert!(parse_direction("sideways").is_err());
        assert!(parse_direction("nan").is_err());
        assert!(parse_direction("").is_err());
    }

    #[test]
    fn speeds_are_percentages_absolute_or_relative() {
        assert_eq!(parse_speed("50"), Ok(Adjust::To(50.0)));
        assert_eq!(parse_speed("75%"), Ok(Adjust::To(75.0)));
        assert_eq!(parse_speed("+10"), Ok(Adjust::By(10.0)));
        assert_eq!(parse_speed("-10%"), Ok(Adjust::By(-10.0)));
        assert!(parse_speed("fast").is_err());
        assert!(parse_speed("inf").is_err());
    }

    #[test]
    fn off_until_set_and_off_again_after_clear() {
        let manual = Manual::new();
        assert_eq!(manual.current(), None);
        assert!(manual.sample(0.0, &MotionConfig::default()).is_none());
        manual.set(Some(Adjust::To(90.0)), Some(Adjust::To(20.0)));
        assert_eq!(manual.current(), Some(Setting { direction: 90.0, speed: 20.0 }));
        assert!(manual.clear());
        assert!(!manual.clear(), "already off");
        assert_eq!(manual.current(), None);
    }

    #[test]
    fn the_last_setting_is_kept_for_the_next_time() {
        let manual = Manual::new();
        manual.set(Some(Adjust::To(270.0)), Some(Adjust::To(80.0)));
        manual.clear();
        // Only the speed is given: it resumes to the left.
        assert_eq!(manual.set(None, Some(Adjust::To(30.0))), Setting { direction: 270.0, speed: 30.0 });
    }

    #[test]
    fn directions_wrap_and_speeds_stop_at_the_ends() {
        let manual = Manual::new();
        assert_eq!(manual.set(Some(Adjust::To(350.0)), None).direction, 350.0);
        assert_eq!(manual.set(Some(Adjust::By(20.0)), None).direction, 10.0);
        assert_eq!(manual.set(Some(Adjust::By(-30.0)), None).direction, 340.0);
        assert_eq!(manual.set(Some(Adjust::To(-90.0)), None).direction, 270.0);
        assert_eq!(manual.set(None, Some(Adjust::To(150.0))).speed, 100.0);
        assert_eq!(manual.set(None, Some(Adjust::By(-300.0))).speed, 0.0);
    }

    #[test]
    fn the_motion_makes_the_dots_flow_the_way_it_was_set() {
        let manual = Manual::new();
        let motion = MotionConfig::default();
        // (direction, expected sign of lateral = right, expected sign of forward = down)
        for (name, degrees, right, down) in [("up", 0.0, 0, -1), ("right", 90.0, 1, 0), ("down", 180.0, 0, 1), ("left", 270.0, -1, 0)] {
            manual.set(Some(Adjust::To(degrees)), Some(Adjust::To(100.0)));
            let s = manual.sample(1.0, &motion).unwrap();
            let sign = |v: f32| if v.abs() < 1e-3 { 0 } else { v.signum() as i32 };
            assert_eq!((sign(s.lateral), sign(s.forward)), (right, down), "{name}: {s:?}");
        }
    }

    #[test]
    fn the_drift_is_the_chosen_share_of_the_maximum_speed() {
        // The overlay multiplies the acceleration by flow_gain to get the drift in px/s.
        let motion = MotionConfig::default();
        let manual = Manual::new();
        manual.set(Some(Adjust::To(135.0)), Some(Adjust::To(40.0)));
        let s = manual.sample(0.0, &motion).unwrap();
        let drift = (s.lateral * motion.flow_gain).hypot(s.forward * motion.flow_gain);
        assert!(close(drift, 0.4 * motion.max_speed), "{drift}");
        // Down-right: right and down are equally strong.
        assert!(close(s.lateral, s.forward) && s.lateral > 0.0);
    }

    #[test]
    fn a_zero_flow_gain_cannot_divide_by_zero() {
        let motion = MotionConfig { flow_gain: 0.0, ..MotionConfig::default() };
        let manual = Manual::new();
        manual.set(None, Some(Adjust::To(100.0)));
        let s = manual.sample(0.0, &motion).unwrap();
        assert_eq!((s.lateral, s.forward), (0.0, 0.0));
    }

    #[test]
    fn settings_are_described_in_words() {
        let motion = MotionConfig::default();
        assert_eq!(Setting { direction: 180.0, speed: 50.0 }.describe(&motion), "down at 50% (125 px/s)");
        assert_eq!(Setting { direction: 100.0, speed: 100.0 }.describe(&motion), "100° at 100% (250 px/s)");
    }
}

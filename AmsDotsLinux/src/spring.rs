//! A damped spring used to smooth the raw motion signal.
//!
//! The spring's *target* is the latest acceleration reading and its *position*
//! chases that target. Sudden jumps in the data (a bump, the first instant of
//! braking) therefore turn into smooth ramps that overshoot slightly and
//! settle, which looks like physical inertia instead of twitching.

/// Longest span a single `step` call will simulate, in seconds. A stalled
/// frame (window hidden, laptop suspended) must not fling the spring.
const MAX_DT: f32 = 0.05;
/// Internal integration step. Small enough to stay stable for stiff springs.
const SUBSTEP: f32 = 0.004;

pub struct Spring {
    stiffness: f32,
    /// Friction coefficient derived from the damping ratio.
    damping: f32,
    pos: (f32, f32),
    vel: (f32, f32),
}

impl Spring {
    /// `damping_ratio` below 1 overshoots slightly, 1 settles without overshoot.
    pub fn new(stiffness: f32, damping_ratio: f32) -> Self {
        Self {
            stiffness,
            damping: 2.0 * damping_ratio * stiffness.sqrt(),
            pos: (0.0, 0.0),
            vel: (0.0, 0.0),
        }
    }

    /// Current smoothed value.
    pub fn pos(&self) -> (f32, f32) {
        self.pos
    }

    /// Advances the spring by `dt` seconds towards `target`.
    pub fn step(&mut self, target: (f32, f32), dt: f32) {
        let mut remaining = dt.clamp(0.0, MAX_DT);
        while remaining > 0.0 {
            let h = remaining.min(SUBSTEP);
            remaining -= h;
            // Semi-implicit Euler: update velocity first, then position.
            self.vel.0 += (self.stiffness * (target.0 - self.pos.0) - self.damping * self.vel.0) * h;
            self.vel.1 += (self.stiffness * (target.1 - self.pos.1) - self.damping * self.vel.1) * h;
            self.pos.0 += self.vel.0 * h;
            self.pos.1 += self.vel.1 * h;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(spring: &mut Spring, target: (f32, f32), seconds: f32) {
        for _ in 0..(seconds * 100.0) as usize {
            spring.step(target, 0.01);
        }
    }

    #[test]
    fn stays_at_rest_without_input() {
        let mut s = Spring::new(60.0, 0.6);
        run(&mut s, (0.0, 0.0), 2.0);
        assert_eq!(s.pos(), (0.0, 0.0));
    }

    #[test]
    fn converges_to_a_constant_target() {
        let mut s = Spring::new(60.0, 0.6);
        run(&mut s, (3.0, -2.0), 5.0);
        assert!((s.pos().0 - 3.0).abs() < 0.01 && (s.pos().1 + 2.0).abs() < 0.01);
    }

    #[test]
    fn underdamped_spring_overshoots_then_settles() {
        let mut s = Spring::new(60.0, 0.4);
        let mut peak = 0.0f32;
        for _ in 0..300 {
            s.step((1.0, 0.0), 0.01);
            peak = peak.max(s.pos().0);
        }
        assert!(peak > 1.05, "expected overshoot, peak was {peak}");
        assert!((s.pos().0 - 1.0).abs() < 0.01);
    }

    #[test]
    fn critically_damped_spring_never_overshoots() {
        let mut s = Spring::new(60.0, 1.0);
        for _ in 0..300 {
            s.step((1.0, 0.0), 0.01);
            assert!(s.pos().0 <= 1.001);
        }
    }

    #[test]
    fn huge_or_stiff_steps_stay_finite() {
        let mut s = Spring::new(5000.0, 0.6);
        s.step((10.0, 10.0), 100.0);
        s.step((10.0, 10.0), f32::MAX);
        assert!(s.pos().0.is_finite() && s.pos().1.is_finite());
    }
}

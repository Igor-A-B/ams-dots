//! The dot generator: spawns, moves and retires dots.
//!
//! Dots are not a fixed set. They are born at random spots in a band along the
//! left and right screen edges, grow, drift with the vehicle's motion, shrink
//! away and are removed, while new ones keep being born. How many exist
//! depends on how intense the motion is. This module knows nothing about
//! Wayland or drawing; it only produces the list of `Dot`s to paint.

use crate::config::{Config, DotsConfig};
use crate::dots::Dot;

/// Gap between a screen edge and the nearest possible spawn point, in pixels.
const EDGE_MARGIN: f32 = 20.0;
/// Each dot's radius is its maximum size times a random factor in this range.
const SIZE_VARIATION: (f32, f32) = (0.6, 1.0);
/// Longest time step the simulation accepts, in seconds.
const MAX_DT: f32 = 0.1;

#[derive(Debug, Clone, Copy)]
struct Particle {
    x: f32,
    y: f32,
    /// Seconds since birth.
    age: f32,
    /// Seconds from birth to disappearance.
    life: f32,
    /// Radius at full size, in pixels.
    radius: f32,
}

/// Small xorshift random number generator, so no external crate is needed.
struct Rng(u64);

impl Rng {
    fn next_f32(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next_f32()
    }
}

/// All live dots of one screen.
pub struct Field {
    width: f32,
    height: f32,
    dots_cfg: DotsConfig,
    flow_gain: f32,
    max_speed: f32,
    particles: Vec<Particle>,
    /// Fraction of a dot "owed" by the spawner; a dot is born each time it reaches 1.
    spawn_debt: f32,
    rng: Rng,
}

impl Field {
    /// Creates a field for a `width` x `height` screen, already holding the
    /// resting population at random ages so the screen does not start empty.
    pub fn new(width: u32, height: u32, config: &Config, seed: u64) -> Self {
        let mut field = Field {
            width: width as f32,
            height: height as f32,
            dots_cfg: config.dots.clone(),
            flow_gain: config.motion.flow_gain,
            max_speed: config.motion.max_speed,
            particles: Vec::new(),
            spawn_debt: 0.0,
            rng: Rng(seed | 1),
        };
        for _ in 0..config.dots.default_amount {
            let age = field.rng.range(0.0, field.dots_cfg.lifetime);
            field.spawn(age);
        }
        field
    }

    /// Advances the simulation by `dt` seconds.
    ///
    /// `accel` is the smoothed acceleration as (lateral, forward) in m/s²; it
    /// sets the drift. `intensity` (0..=1) sets how many dots are alive:
    /// from `default_amount` at 0 up to `default_amount + motion_extra_amount` at 1.
    pub fn step(&mut self, dt: f32, accel: (f32, f32), intensity: f32) {
        let dt = dt.clamp(0.0, MAX_DT);
        let flow = self.flow(accel);

        for p in &mut self.particles {
            p.age += dt;
            p.x += flow.0 * dt;
            p.y += flow.1 * dt;
        }
        let (w, h) = (self.width, self.height);
        self.particles
            .retain(|p| p.age < p.life && p.x > -p.radius && p.x < w + p.radius && p.y > -p.radius && p.y < h + p.radius);

        let target = self.dots_cfg.default_amount as f32 + self.dots_cfg.motion_extra_amount as f32 * intensity.clamp(0.0, 1.0);
        self.spawn_debt += target / self.dots_cfg.lifetime * dt;
        while self.spawn_debt >= 1.0 {
            self.spawn_debt -= 1.0;
            self.spawn(0.0);
        }
    }

    /// Drift velocity in px/s for a smoothed acceleration. Dots flow the way
    /// the passenger is thrown: speeding up pushes them down the screen,
    /// braking up, and a push to the right moves them right.
    fn flow(&self, accel: (f32, f32)) -> (f32, f32) {
        let (vx, vy) = (accel.0 * self.flow_gain, accel.1 * self.flow_gain);
        let speed = vx.hypot(vy);
        if speed > self.max_speed && speed > 0.0 {
            let k = self.max_speed / speed;
            (vx * k, vy * k)
        } else {
            (vx, vy)
        }
    }

    /// Number of dots currently alive.
    #[cfg(test)]
    pub fn count(&self) -> usize {
        self.particles.len()
    }

    /// The dots to paint right now, sized and faded by their age.
    pub fn dots(&self) -> Vec<Dot> {
        let fade_in = self.dots_cfg.fade_in / 100.0;
        let fade_out = self.dots_cfg.fade_out / 100.0;
        self.particles
            .iter()
            .map(|p| {
                let scale = envelope(p.age / p.life, fade_in, fade_out);
                Dot { x: p.x, y: p.y, radius: p.radius * scale, opacity: scale }
            })
            .collect()
    }

    /// Adds one dot at a random spot in the left or right band.
    fn spawn(&mut self, age: f32) {
        let band = self.width * self.dots_cfg.edge_band / 100.0;
        let left = self.rng.next_f32() < 0.5;
        let x = if left {
            self.rng.range(EDGE_MARGIN, EDGE_MARGIN + band)
        } else {
            self.rng.range(self.width - EDGE_MARGIN - band, self.width - EDGE_MARGIN)
        };
        let y = self.rng.range(0.0, self.height);
        let radius = self.dots_cfg.size * self.rng.range(SIZE_VARIATION.0, SIZE_VARIATION.1);
        self.particles.push(Particle { x, y, age, life: self.dots_cfg.lifetime, radius });
    }
}

/// Size factor (0..=1) over a dot's life. `t` is the age as a fraction of the
/// lifetime; `fade_in`/`fade_out` are the fractions spent growing and
/// shrinking. Zero at birth and death, one in between.
fn envelope(t: f32, fade_in: f32, fade_out: f32) -> f32 {
    let grow = if fade_in > 0.0 { smoothstep(t / fade_in) } else { 1.0 };
    let shrink = if fade_out > 0.0 { smoothstep((1.0 - t) / fade_out) } else { 1.0 };
    grow.min(shrink)
}

fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(config: &Config) -> Field {
        Field::new(1000, 800, config, 42)
    }

    fn run(field: &mut Field, seconds: f32, accel: (f32, f32), intensity: f32) {
        for _ in 0..(seconds * 30.0) as usize {
            field.step(1.0 / 30.0, accel, intensity);
        }
    }

    #[test]
    fn starts_with_the_resting_population() {
        let c = Config::default();
        assert_eq!(field(&c).count() as u32, c.dots.default_amount);
    }

    #[test]
    fn rest_population_stays_near_the_default() {
        let c = Config::default();
        let mut f = field(&c);
        run(&mut f, 10.0, (0.0, 0.0), 0.0);
        let n = f.count() as i32;
        assert!((n - c.dots.default_amount as i32).abs() <= 3, "count was {n}");
    }

    #[test]
    fn motion_raises_the_population_and_it_decays_afterwards() {
        let c = Config::default();
        let mut f = field(&c);
        run(&mut f, 10.0, (0.0, 0.0), 1.0);
        let busy = f.count() as u32;
        assert!(busy > c.dots.default_amount + c.dots.motion_extra_amount - 5, "count was {busy}");
        run(&mut f, 10.0, (0.0, 0.0), 0.0);
        assert!((f.count() as i32 - c.dots.default_amount as i32).abs() <= 3);
    }

    #[test]
    fn every_dot_dies_after_its_lifetime() {
        let mut c = Config::default();
        c.dots.default_amount = 0;
        c.dots.motion_extra_amount = 10;
        let mut f = field(&c);
        run(&mut f, 3.0, (0.0, 0.0), 1.0);
        assert!(f.count() > 0);
        run(&mut f, c.dots.lifetime + 0.5, (0.0, 0.0), 0.0);
        assert_eq!(f.count(), 0);
    }

    #[test]
    fn envelope_is_zero_at_the_ends_and_full_in_the_middle() {
        assert_eq!(envelope(0.0, 0.2, 0.4), 0.0);
        assert_eq!(envelope(1.0, 0.2, 0.4), 0.0);
        assert_eq!(envelope(0.5, 0.2, 0.4), 1.0);
        assert!(envelope(0.1, 0.2, 0.4) > 0.0 && envelope(0.1, 0.2, 0.4) < 1.0);
        // No fade configured: full size straight away.
        assert_eq!(envelope(0.0, 0.0, 0.0), 1.0);
    }

    #[test]
    fn dots_flow_with_the_felt_inertia() {
        let mut c = Config::default();
        c.dots.default_amount = 1;
        c.dots.fade_in = 0.0;
        c.dots.fade_out = 0.0;
        let mut f = field(&c);
        let before = f.dots()[0];
        f.step(0.05, (0.0, 2.0), 0.0); // speeding up
        let after = f.dots()[0];
        assert!(after.y > before.y && after.x == before.x, "speeding up moves dots down");
        f.step(0.05, (2.0, 0.0), 0.0); // pushed right
        assert!(f.dots()[0].x > after.x, "a push to the right moves dots right");
    }

    #[test]
    fn no_motion_means_no_drift() {
        let mut c = Config::default();
        c.dots.default_amount = 1;
        let mut f = field(&c);
        let before = f.dots()[0];
        f.step(0.05, (0.0, 0.0), 0.0);
        let after = f.dots()[0];
        assert_eq!((before.x, before.y), (after.x, after.y));
    }

    #[test]
    fn drift_speed_is_capped() {
        let mut c = Config::default();
        c.dots.default_amount = 1;
        c.motion.max_speed = 100.0;
        let mut f = field(&c);
        let before = f.dots()[0];
        f.step(0.1, (0.0, 1000.0), 0.0);
        let moved = f.dots()[0].y - before.y;
        assert!(moved <= 10.01, "moved {moved}");
    }

    #[test]
    fn dots_spawn_only_inside_the_edge_bands() {
        let c = Config::default();
        let mut f = field(&c);
        run(&mut f, 5.0, (0.0, 0.0), 1.0);
        let band = 1000.0 * c.dots.edge_band / 100.0;
        for d in f.dots() {
            let left = d.x >= EDGE_MARGIN && d.x <= EDGE_MARGIN + band;
            let right = d.x >= 1000.0 - EDGE_MARGIN - band && d.x <= 1000.0 - EDGE_MARGIN;
            assert!(left || right, "dot at x = {}", d.x);
        }
    }

    #[test]
    fn same_seed_gives_the_same_dots() {
        let c = Config::default();
        let (a, b) = (field(&c).dots(), field(&c).dots());
        assert_eq!(a.len(), b.len());
        assert!(a.iter().zip(&b).all(|(p, q)| p.x == q.x && p.y == q.y));
    }
}

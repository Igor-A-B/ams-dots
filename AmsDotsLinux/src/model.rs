//! Stage 3 — Motion model. Turns vehicle motion into dot positions.

use crate::config::Config;
use crate::types::{Dot, Vec2, VehicleMotion};

pub trait DotModel {
    /// Advance the simulation by `dt` seconds. `motion` is None when the source is disconnected.
    fn step(&mut self, motion: Option<VehicleMotion>, dt: f32);
    fn dots(&self) -> &[Dot];
}

/// Dots on spring-damper physics around fixed rest anchors.
pub struct SpringModel {
    dots: Vec<Dot>,
    rest: Vec<Vec2>,
    vel: Vec<Vec2>,
    cfg: Config,
}

impl SpringModel {
    pub fn new(cfg: Config) -> Self {
        // Placeholder layout: dots evenly along the left and right screen edges.
        let n = cfg.dot_count;
        let rest: Vec<Vec2> = (0..n)
            .map(|i| Vec2 { x: if i % 2 == 0 { 0.03 } else { 0.97 }, y: (i / 2 + 1) as f32 / (n / 2 + 1) as f32 })
            .collect();
        let dots = rest.iter().map(|&pos| Dot { pos, radius: cfg.dot_radius, opacity: cfg.opacity }).collect();
        Self { dots, vel: vec![Vec2::default(); n], rest, cfg }
    }
}

impl DotModel for SpringModel {
    fn step(&mut self, motion: Option<VehicleMotion>, dt: f32) {
        let m = motion.unwrap_or_default();
        // Dots slide opposite to the vehicle's acceleration.
        let push = Vec2 { x: -m.lateral * self.cfg.gain, y: m.forward * self.cfg.gain };
        for i in 0..self.dots.len() {
            let target = Vec2 { x: self.rest[i].x + push.x, y: self.rest[i].y + push.y };
            let d = &mut self.dots[i];
            let v = &mut self.vel[i];
            let (ax, ay) = (
                self.cfg.spring_k * (target.x - d.pos.x) - self.cfg.damping * v.x,
                self.cfg.spring_k * (target.y - d.pos.y) - self.cfg.damping * v.y,
            );
            v.x += ax * dt;
            v.y += ay * dt;
            d.pos.x += v.x * dt;
            d.pos.y += v.y * dt;
        }
    }

    fn dots(&self) -> &[Dot] {
        &self.dots
    }
}

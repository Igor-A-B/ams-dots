//! Wires the pipeline together and owns the main loop.

use crate::config::Config;
use crate::control::Command;
use crate::fusion::Fusion;
use crate::model::DotModel;
use crate::render::Renderer;
use crate::source::MotionSource;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

pub struct App {
    pub cfg: Config,
    pub source: Box<dyn MotionSource>,
    pub fusion: Box<dyn Fusion>,
    pub model: Box<dyn DotModel>,
    pub renderer: Box<dyn Renderer>,
    pub commands: Receiver<Command>,
}

impl App {
    /// Run until `Command::Quit` (or `max_runtime`, handy for demos/tests).
    pub fn run(&mut self, max_runtime: Option<Duration>) {
        let start = Instant::now();
        let frame = Duration::from_secs_f32(1.0 / self.cfg.fps as f32);
        let mut visible = true;
        self.renderer.set_visible(visible);

        loop {
            let tick = Instant::now();
            let now = start.elapsed();
            if max_runtime.is_some_and(|m| now >= m) {
                break;
            }

            while let Ok(cmd) = self.commands.try_recv() {
                match cmd {
                    Command::Quit => return,
                    Command::Toggle => {
                        visible = !visible;
                        self.renderer.set_visible(visible);
                    }
                    Command::Calibrate => self.fusion.calibrate(),
                    Command::ReloadConfig => {} // TODO: reload TOML
                }
            }

            while let Some(sample) = self.source.poll(now) {
                self.fusion.push(sample);
            }

            let motion = self.source.connected().then(|| self.fusion.current());
            self.model.step(motion, frame.as_secs_f32());
            self.renderer.present(self.model.dots());

            if let Some(rest) = frame.checked_sub(tick.elapsed()) {
                std::thread::sleep(rest);
            }
        }
    }
}

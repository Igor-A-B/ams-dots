mod app;
mod config;
mod control;
mod fusion;
mod model;
mod render;
mod source;
mod types;

use std::sync::mpsc;
use std::time::Duration;

fn main() {
    let cfg = config::Config::default();
    let (_cmd_tx, commands) = mpsc::channel(); // control socket will own the sender later

    let mut app = app::App {
        source: Box::new(source::SyntheticSource::new(50.0)),
        fusion: Box::new(fusion::PassthroughFusion::default()),
        model: Box::new(model::SpringModel::new(cfg.clone())),
        renderer: Box::new(render::ConsoleRenderer::default()),
        commands,
        cfg,
    };
    app.run(Some(Duration::from_secs(5)));
}

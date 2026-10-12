mod config;
mod dots;
mod overlay;
mod particles;
mod replay;
mod spring;

const USAGE: &str = "usage: ams-dots-linux [--config FILE] [recording.csv]";

/// Without a recording the dots behave as if the vehicle were at rest; with
/// one they react to the recorded motion. Settings come from
/// `~/.config/ams-dots/config.toml` unless `--config` names another file.
fn main() {
    if let Err(err) = start() {
        eprintln!("ams-dots: {err}");
        std::process::exit(1);
    }
}

fn start() -> Result<(), String> {
    let mut config_path: Option<String> = None;
    let mut recording_path: Option<String> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--config" {
            config_path = Some(args.next().ok_or(format!("--config needs a file\n{USAGE}"))?);
        } else if arg.starts_with("--") {
            return Err(format!("unknown option {arg}\n{USAGE}"));
        } else {
            recording_path = Some(arg);
        }
    }

    let config = config::load(config_path.as_deref())?;
    let motion = match recording_path {
        Some(path) => Some(replay::load(&path)?),
        None => None,
    };
    overlay::run(config, motion)
}

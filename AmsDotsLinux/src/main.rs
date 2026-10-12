mod ble;
mod config;
mod control;
mod dots;
mod fusion;
mod motion;
mod overlay;
mod particles;
mod protocol;
mod replay;
mod spring;

use motion::{Live, MotionSource};

const USAGE: &str = "usage: ams-dots-linux [--config FILE] [--rate HZ] [--no-phone] [recording.csv]\n       ams-dots-linux calibrate";

/// The dots follow the phone over Bluetooth by default; a CSV recording replays
/// instead (for development), and `--no-phone` shows a vehicle at rest. Settings
/// come from `~/.config/ams-dots/config.toml` unless `--config` names another file.
/// `ams-dots-linux calibrate` tells a running overlay which way is forward.
fn main() {
    if let Err(err) = start() {
        eprintln!("ams-dots: {err}");
        std::process::exit(1);
    }
}

fn start() -> Result<(), String> {
    let mut config_path: Option<String> = None;
    let mut recording_path: Option<String> = None;
    let mut rate_hz: Option<u8> = None;
    let mut use_phone = true;

    let mut args = std::env::args().skip(1).peekable();
    if args.peek().map(String::as_str) == Some("calibrate") {
        println!("{}", control::send("calibrate")?);
        return Ok(());
    }
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => config_path = Some(args.next().ok_or(format!("--config needs a file\n{USAGE}"))?),
            "--rate" => {
                let text = args.next().ok_or(format!("--rate needs a number of Hz\n{USAGE}"))?;
                let hz: u8 = text.parse().map_err(|_| format!("--rate: {text:?} is not a number of Hz\n{USAGE}"))?;
                if !(protocol::MIN_RATE_HZ..=protocol::MAX_RATE_HZ).contains(&hz) {
                    return Err(format!("--rate must be {}-{} Hz", protocol::MIN_RATE_HZ, protocol::MAX_RATE_HZ));
                }
                rate_hz = Some(hz);
            }
            "--no-phone" => use_phone = false,
            _ if arg.starts_with("--") => return Err(format!("unknown option {arg}\n{USAGE}")),
            _ => recording_path = Some(arg),
        }
    }

    let config = config::load(config_path.as_deref())?;
    let motion = match recording_path {
        Some(path) => MotionSource::Replay(replay::load(&path)?),
        None if use_phone => {
            let live = Live::new();
            ble::spawn(live.clone(), rate_hz);
            MotionSource::Live(live)
        }
        None => MotionSource::Rest,
    };
    overlay::run(config, motion)
}

mod ble;
mod config;
mod control;
mod dots;
mod fusion;
mod motion;
mod overlay;
mod particles;
mod picker;
mod protocol;
mod replay;
mod spring;

use motion::{Live, MotionSource};
use picker::Target;
use std::io::IsTerminal;
use std::time::Duration;

const USAGE: &str = "\
usage: ams-dots-linux [options] [recording.csv]
       ams-dots-linux status [--watch] | connect | disconnect | relearn

Starts the overlay. It scans for phones running the AmsDots app, asks which one
to follow, and shows dots that move with the phone. The other commands talk to
an overlay that is already running (from a second terminal).

options:
  --phone NAME   follow the phone with this Bluetooth name, or `any`; no question asked
  --rate HZ      ask the phone to sample at HZ (10-100; its default is 50)
  --config FILE  use another config file (default ~/.config/ams-dots/config.toml)
  --no-phone     no Bluetooth: dots stay at rest
  recording.csv  no Bluetooth: replay a recorded drive (tests/data/drive.csv)
  -h, --help     this text

commands:
  status           link state, phone, packet rate, forward direction
  status --watch   the same as one live line (shake the phone and watch it move)
  connect          start looking for the phone again
  disconnect       drop the phone and stop looking until `connect`
  relearn          forget the learned forward direction (after moving the phone)";

/// How long the start-up scan listens for phones.
const PICKER_SCAN: Duration = Duration::from_secs(4);

fn main() {
    if let Err(err) = start() {
        eprintln!("ams-dots: {err}");
        std::process::exit(1);
    }
}

fn start() -> Result<(), String> {
    let mut config_path: Option<String> = None;
    let mut recording_path: Option<String> = None;
    let mut phone_name: Option<String> = None;
    let mut rate_hz: Option<u8> = None;
    let mut use_phone = true;

    let mut args = std::env::args().skip(1).peekable();
    if let Some(command @ ("status" | "connect" | "disconnect" | "relearn")) = args.peek().map(String::as_str) {
        let command = command.to_string();
        args.next();
        if command == "status" && args.peek().map(String::as_str) == Some("--watch") {
            return control::watch();
        }
        println!("{}", control::send(&command)?);
        return Ok(());
    }
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            "--config" => config_path = Some(args.next().ok_or(format!("--config needs a file\n\n{USAGE}"))?),
            "--phone" => phone_name = Some(args.next().ok_or(format!("--phone needs a name (or `any`)\n\n{USAGE}"))?),
            "--rate" => {
                let text = args.next().ok_or(format!("--rate needs a number of Hz\n\n{USAGE}"))?;
                let hz: u8 = text.parse().map_err(|_| format!("--rate: {text:?} is not a number of Hz\n\n{USAGE}"))?;
                if !(protocol::MIN_RATE_HZ..=protocol::MAX_RATE_HZ).contains(&hz) {
                    return Err(format!("--rate must be {}-{} Hz", protocol::MIN_RATE_HZ, protocol::MAX_RATE_HZ));
                }
                rate_hz = Some(hz);
            }
            "--no-phone" => use_phone = false,
            _ if arg.starts_with('-') => return Err(format!("unknown option {arg}\n\n{USAGE}")),
            _ => recording_path = Some(arg),
        }
    }

    let config = config::load(config_path.as_deref())?;
    let motion = match recording_path {
        Some(path) => MotionSource::Replay(replay::load(&path)?),
        None if use_phone => {
            let Some(target) = choose_target(phone_name)? else { return Ok(()) };
            eprintln!("ams-dots: following {}. Stop with Ctrl+C.", target.describe());
            let exe = std::env::current_exe().map_or_else(|_| "ams-dots-linux".to_string(), |p| p.display().to_string());
            eprintln!("ams-dots: to watch the link, run in another terminal:  {exe} status --watch");
            let live = Live::new();
            ble::spawn(live.clone(), rate_hz, target);
            ble::disconnect_on_exit(live.clone());
            MotionSource::Live(live)
        }
        None => MotionSource::Rest,
    };
    overlay::run(config, motion)
}

/// Decides which phone to follow: the one named on the command line, the one the user picks from the
/// list of phones in range, or (without a terminal to ask on) the one chosen last time. `None` = the user quit.
fn choose_target(phone_name: Option<String>) -> Result<Option<Target>, String> {
    if let Some(name) = phone_name {
        return Ok(Some(if name == "any" { Target::any() } else { Target::named(&name) }));
    }
    if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
        return Ok(Some(picker::remembered().map_or_else(Target::any, |name| Target::named(&name))));
    }
    let last = picker::remembered();
    picker::ask(
        last.as_deref(),
        picker::remember,
        || ble::scan(PICKER_SCAN),
        &mut std::io::stdin().lock(),
        &mut std::io::stdout(),
    )
}

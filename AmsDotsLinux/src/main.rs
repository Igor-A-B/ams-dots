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
use picker::Startup;
use std::io::IsTerminal;
use std::time::Duration;

const USAGE: &str = "\
usage: ams-dots-linux [options] [recording.csv]
       ams-dots-linux status [--watch] | phones | select [N|NAME|any] | forget | connect | disconnect | relearn

Starts the overlay and shows dots that move with the phone running the AmsDots
app. The first time it lists the phones in range and asks which one to follow;
after that it connects to the last phone by itself. The other commands talk to
an overlay that is already running (from a second terminal).

options:
  --phone NAME   follow the phone with this Bluetooth name, or `any`; no question asked
  --pick         list the phones and ask, even if a last phone is remembered
  --rate HZ      ask the phone to sample at HZ (10-100; its default is 50)
  --record FILE  write every sample from the phone (raw and fused) to FILE as CSV
  --config FILE  use another config file (default ~/.config/ams-dots/config.toml)
  --no-phone     no Bluetooth: dots stay at rest
  recording.csv  no Bluetooth: replay a recorded drive (tests/data/drive.csv)
  -h, --help     this text

commands:
  status           link state, phone, packet rate, forward direction
  status --watch   the same as one live line (shake the phone and watch it move)
  phones           phones heard lately (numbered), marking the connected and the last one
  select           switch phone: shows the list and asks; or `select 2`, `select NAME`, `select any`
  forget           forget the last phone (the next start asks again)
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
    let mut record_path: Option<String> = None;
    let mut use_phone = true;
    let mut pick = false;

    let mut args = std::env::args().skip(1).peekable();
    if let Some(command @ ("status" | "phones" | "select" | "forget" | "connect" | "disconnect" | "relearn")) =
        args.peek().map(String::as_str)
    {
        let command = command.to_string();
        args.next();
        let rest: Vec<String> = args.collect();
        if command == "status" && rest.first().map(String::as_str) == Some("--watch") {
            return control::watch();
        }
        if command == "select" && rest.is_empty() && std::io::stdin().is_terminal() {
            println!("{}", control::select_interactive()?);
            return Ok(());
        }
        println!("{}", control::send(&format!("{command} {}", rest.join(" ")))?);
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
            "--record" => record_path = Some(args.next().ok_or(format!("--record needs a file\n\n{USAGE}"))?),
            "--pick" => pick = true,
            "--no-phone" => use_phone = false,
            _ if arg.starts_with('-') => return Err(format!("unknown option {arg}\n\n{USAGE}")),
            _ => recording_path = Some(arg),
        }
    }

    let config = config::load(config_path.as_deref())?;
    let motion = match recording_path {
        Some(path) => MotionSource::Replay(replay::load(&path)?),
        None if use_phone => {
            let Some((target, remember)) = choose_target(phone_name, pick)? else { return Ok(()) };
            eprintln!("ams-dots: following {}. Stop with Ctrl+C.", target.describe());
            eprintln!("ams-dots: to change phone while it runs:  ams-dots-linux select   (list: ams-dots-linux phones)");
            let exe = std::env::current_exe().map_or_else(|_| "ams-dots-linux".to_string(), |p| p.display().to_string());
            eprintln!("ams-dots: to watch the link, run in another terminal:  {exe} status --watch");
            let live = Live::new();
            live.set_target(target);
            if let Some(path) = &record_path {
                let file = std::fs::File::create(path).map_err(|e| format!("--record {path}: {e}"))?;
                live.record_to(Box::new(std::io::BufWriter::new(file))).map_err(|e| format!("--record {path}: {e}"))?;
                eprintln!("ams-dots: recording every sample to {path}");
            }
            ble::spawn(live.clone(), rate_hz, remember);
            ble::disconnect_on_exit(live.clone());
            MotionSource::Live(live)
        }
        None => MotionSource::Rest,
    };
    overlay::run(config, motion)
}

/// Decides which phone to follow, and whether to store the phone that connects as the last known one.
/// A phone chosen before (or `--phone`) is used without a question; with none remembered the user picks from
/// the list of phones in range (or, without a terminal to ask on, the first one found is used).
/// `None` = the user quit.
fn choose_target(phone_name: Option<String>, pick: bool) -> Result<Option<(picker::Target, bool)>, String> {
    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let last = picker::remembered();
    match picker::startup(phone_name.as_deref(), pick, last.as_deref(), interactive)? {
        Startup::Follow { target, remember } => Ok(Some((target, remember))),
        Startup::Ask => Ok(picker::ask(
            last.as_deref(),
            picker::remember,
            || ble::scan(PICKER_SCAN),
            &mut std::io::stdin().lock(),
            &mut std::io::stdout(),
        )?
        .map(|target| (target, true))),
    }
}

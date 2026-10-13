mod ble;
mod cli;
mod config;
mod control;
mod dots;
mod fusion;
mod manual;
mod motion;
mod overlay;
mod particles;
mod picker;
mod protocol;
mod replay;
mod service;
mod spring;

use cli::{Options, Parsed, USAGE};
use config::Settings;
use manual::Manual;
use motion::{Live, MotionSource};
use picker::{Startup, Target};
use std::io::IsTerminal;
use std::sync::Arc;
use std::time::Duration;

/// Commands that are sent to the running overlay as they are.
const REMOTE_COMMANDS: [&str; 12] =
    ["status", "phones", "select", "forget", "connect", "disconnect", "relearn", "move", "speed", "direction", "auto", "reload"];

/// How long the start-up scan listens for phones.
const PICKER_SCAN: Duration = Duration::from_secs(4);

fn main() {
    if let Err(err) = dispatch() {
        eprintln!("ams-dots: {err}");
        std::process::exit(1);
    }
}

fn dispatch() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (command, rest) = match args.split_first() {
        Some((first, rest)) => (first.as_str(), rest),
        None => ("start", &args[..]),
    };
    match command {
        "start" => start(rest),
        "stop" => {
            println!("{}", service::stop()?);
            Ok(())
        }
        "restart" => {
            // Check the options and the config before stopping what works.
            if let Parsed::Start(options) = Options::parse(rest)? {
                Settings::load(options.config_path)?;
            }
            service::stop()?;
            start(rest)
        }
        "status" if rest.first().map(String::as_str) == Some("--watch") => control::watch(),
        "select" if rest.is_empty() && std::io::stdin().is_terminal() => {
            println!("{}", control::select_interactive()?);
            Ok(())
        }
        command if REMOTE_COMMANDS.contains(&command) => {
            println!("{}", control::send(&format!("{command} {}", rest.join(" ")))?);
            Ok(())
        }
        // Options without a command word: `ams-dots-linux --no-phone`.
        _ => start(&args),
    }
}

/// `start`: in the background by default, so the terminal is free again.
fn start(args: &[String]) -> Result<(), String> {
    let options = match Options::parse(args)? {
        Parsed::Help => {
            println!("{USAGE}");
            return Ok(());
        }
        Parsed::Start(options) => options,
    };
    if control::is_running() {
        let note = if args.is_empty() { "" } else { " (the options were not applied; `ams-dots-linux restart` with them does)" };
        println!("ams-dots is already running{note}. `ams-dots-linux status` shows what it does, `--help` lists the controls.");
        return Ok(());
    }
    let mut options = options;
    // Questions are asked here, where the terminal is, before the background process takes over.
    if let Some(chosen) = ask_for_phone(&options)? {
        match chosen {
            // The chosen phone is stored by the picker, and the overlay picks it up from there.
            Some(Target { name: Some(_) }) => {}
            Some(Target { name: None }) => options.phone_name = Some("any".to_string()),
            None => return Ok(()),
        }
    }
    if options.foreground {
        return run(options);
    }
    let message = service::launch(&options.background_args()?)?;
    println!("ams-dots {message}");
    println!("Control it from any terminal: `ams-dots-linux move down 60`, `auto`, `status`, `stop`; `--help` lists everything.");
    Ok(())
}

/// Runs the phone list when the start-up rules (see `picker::startup`) call for it and there is a terminal.
/// `None`: nothing to ask. `Some(None)`: the user quit. `Some(Some(target))`: the choice.
fn ask_for_phone(options: &Options) -> Result<Option<Option<Target>>, String> {
    if options.no_phone || options.recording_path.is_some() {
        return Ok(None);
    }
    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let last = picker::remembered();
    match picker::startup(options.phone_name.as_deref(), options.pick, last.as_deref(), interactive)? {
        Startup::Follow { .. } => Ok(None),
        Startup::Ask => Ok(Some(picker::ask(
            last.as_deref(),
            picker::remember,
            || ble::scan(PICKER_SCAN),
            &mut std::io::stdin().lock(),
            &mut std::io::stdout(),
        )?)),
    }
}

/// The overlay itself, in this process: what the background process runs, and `--foreground` runs here.
fn run(options: Options) -> Result<(), String> {
    let settings = Settings::load(options.config_path.clone())?;
    let manual = Manual::new();
    let live = Live::new();

    let replay = match &options.recording_path {
        Some(path) => Some(replay::load(path)?),
        None => None,
    };
    let has_phone = replay.is_none();
    let mut remember = false;
    if has_phone {
        // Without a terminal (the background process) the choice made at the prompt is found in the memory.
        let (target, remember_it) = choose_target(&options)?;
        live.set_target(target.clone());
        remember = remember_it;
        if options.no_phone {
            // Nothing touches Bluetooth until `connect`.
            live.set_enabled(false);
            eprintln!("ams-dots: no phone for now; `ams-dots-linux connect` looks for {}", target.describe());
        } else {
            eprintln!("ams-dots: following {}", target.describe());
        }
        if let Some(path) = &options.record_path {
            let file = std::fs::File::create(path).map_err(|e| format!("--record {path}: {e}"))?;
            live.record_to(Box::new(std::io::BufWriter::new(file))).map_err(|e| format!("--record {path}: {e}"))?;
            eprintln!("ams-dots: recording every sample to {path}");
        }
    }
    let motion = match replay {
        Some(samples) => MotionSource::Replay(samples),
        None => MotionSource::Live(live.clone()),
    };

    let context = Arc::new(control::Context { live: live.clone(), manual: manual.clone(), settings: settings.clone(), phone: has_phone });
    let rate_hz = options.rate_hz;
    let ready = move || {
        control::spawn(context);
        service::shutdown_on_signals(live.clone());
        if has_phone {
            ble::spawn(live, rate_hz, remember);
        }
        eprintln!("ams-dots: running. Control it from another terminal: `ams-dots-linux --help` lists the commands, `ams-dots-linux stop` ends it.");
    };
    overlay::run(settings, manual, motion, ready)
}

/// Decides which phone to follow, and whether to store the phone that connects as the last known one.
/// A phone chosen before (or `--phone`) is used without a question; with none remembered the user picks from
/// the list of phones in range (or, without a terminal to ask on, the first one found is used).
/// With `--no-phone` nothing is asked: the target is what `connect` will look for later.
fn choose_target(options: &Options) -> Result<(Target, bool), String> {
    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let last = picker::remembered();
    if options.no_phone {
        return Ok((last.as_deref().map_or_else(Target::any, Target::named), true));
    }
    match picker::startup(options.phone_name.as_deref(), options.pick, last.as_deref(), interactive)? {
        Startup::Follow { target, remember } => Ok((target, remember)),
        Startup::Ask => {
            let chosen = picker::ask(
                last.as_deref(),
                picker::remember,
                || ble::scan(PICKER_SCAN),
                &mut std::io::stdin().lock(),
                &mut std::io::stdout(),
            )?;
            chosen.map(|target| (target, true)).ok_or_else(|| "no phone chosen".to_string())
        }
    }
}

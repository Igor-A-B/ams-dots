//! Talking to the running overlay from a terminal or a key binding.
//!
//! The overlay listens on a Unix socket; `ams-dots-linux <command>` connects,
//! sends one line and prints the answer:
//!
//! | Command | Effect |
//! |---|---|
//! | `status` | link state, phone followed and remembered, packet statistics, motion, how sure the forward direction is |
//! | `status --watch` | the same as one line that refreshes five times a second (shake the phone to see it move) |
//! | `phones` | numbered list of the phones heard lately, marking the connected and the remembered one |
//! | `select [N\|NAME\|any]` | follow another phone (and remember it); without an argument, ask on the terminal |
//! | `forget` | forget the remembered phone (the next start asks again) |
//! | `connect` | (re)start looking for the phone |
//! | `disconnect` | drop the phone and stop looking until `connect` |
//! | `relearn` | forget the learned forward direction, e.g. after moving the phone |
//! | `move DIR [SPEED]` | manual control: dots flow towards `DIR` at `SPEED` % of the maximum (see `manual.rs`) |
//! | `speed N\|+N\|-N` | change the manual speed (switches manual control on) |
//! | `direction D\|+D\|-D` | change the manual direction (switches manual control on) |
//! | `auto` | the phone (or recording) drives the dots again |
//! | `reload` | read the config file again and apply it |
//! | `stop` | end the overlay |
//!
//! The server runs whatever the motion source is (phone, recording, nothing), because manual control,
//! `reload` and `stop` do not need Bluetooth.
//!
//! Example key bindings for Hyprland: `bind = $mod, F9, exec, ams-dots-linux disconnect`,
//! `bind = $mod, equal, exec, ams-dots-linux speed +10`.

use crate::config::Settings;
use crate::manual::{self, Manual};
use crate::motion::Live;
use crate::picker::{self, Candidate, FileMemory, PhoneMemory, Target};
use crate::service;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as AsyncBufReader};
use tokio::net::UnixListener;

/// How often `status --watch` refreshes.
const WATCH_INTERVAL: Duration = Duration::from_millis(200);

/// Socket in the user's runtime directory (`/run/user/<uid>`), which only they can use.
pub fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    dir.join("ams-dots.sock")
}

/// What the commands act on.
pub struct Context {
    pub live: Arc<Live>,
    pub manual: Arc<Manual>,
    pub settings: Arc<Settings>,
    /// False while replaying a recording: there is no Bluetooth receiver then, so the phone cannot be (dis)connected.
    pub phone: bool,
}

const NO_PHONE: &str = "error: a recording is being replayed, so there is no phone link; run `ams-dots-linux restart` without the recording to use the phone";

/// What to answer one (non-streaming) command line. Kept separate from the socket so it can be tested.
pub fn handle_command(ctx: &Context, memory: &dyn PhoneMemory, line: &str) -> String {
    let live = &*ctx.live;
    let line = line.trim();
    let (command, argument) = line.split_once(char::is_whitespace).map_or((line, ""), |(c, a)| (c, a.trim()));
    match command {
        "status" => {
            let remembered = memory.get();
            let report = live.status(Instant::now()).report();
            format!("{report}\nlast phone: {}\ndots:     {}", remembered.as_deref().unwrap_or("-"), dots_source(ctx))
        }
        "move" => manual_move(ctx, argument),
        "speed" => manual_one(ctx, argument, "speed"),
        "direction" => manual_one(ctx, argument, "direction"),
        "auto" => auto(ctx),
        "reload" => match ctx.settings.reload() {
            Ok(()) => format!("ok: applied {}", ctx.settings.source()),
            Err(err) => format!("error: {err} (the running settings are unchanged)"),
        },
        "phones" if argument == "--raw" => picker::to_lines(&live.phones(Instant::now())),
        "phones" => phones_report(live, memory),
        "select" | "connect" if !ctx.phone => NO_PHONE.to_string(),
        "select" => select(live, memory, argument),
        "forget" => {
            memory.forget();
            "ok: forgot the last phone; the next start asks which phone to use".to_string()
        }
        "connect" => {
            live.set_enabled(true);
            "ok: looking for the phone".to_string()
        }
        "disconnect" => {
            live.set_enabled(false);
            "ok: disconnected; run `ams-dots-linux connect` to look for the phone again".to_string()
        }
        "relearn" => {
            live.relearn();
            "ok: forgot the forward direction; it is learned again from the next drive".to_string()
        }
        other => format!(
            "error: unknown command {other:?} (known: status, status --watch, phones, select, forget, connect, disconnect, relearn, move, speed, direction, auto, reload, stop)"
        ),
    }
}

/// What drives the dots right now, for `status`.
fn dots_source(ctx: &Context) -> String {
    match ctx.manual.current() {
        Some(setting) => format!(
            "moved by hand: {} (phone ignored; `ams-dots-linux auto` gives control back)",
            setting.describe(&ctx.settings.config().motion)
        ),
        None if ctx.phone && ctx.live.is_connected() => "follow the phone".to_string(),
        None if ctx.phone => "at rest (no phone connected; `ams-dots-linux move` sets them by hand)".to_string(),
        None => "replaying a recording".to_string(),
    }
}

/// The reply to a command that changed the manual setting.
fn manual_reply(ctx: &Context, setting: manual::Setting) -> String {
    format!("ok: dots flow {}; `ams-dots-linux auto` gives control back", setting.describe(&ctx.settings.config().motion))
}

/// `move DIR [SPEED]`.
fn manual_move(ctx: &Context, argument: &str) -> String {
    let mut words = argument.split_whitespace();
    let (Some(direction), speed, None) = (words.next(), words.next(), words.next()) else {
        return "error: usage: move DIRECTION [SPEED]   e.g. `move down 60`, `move up-left`, `move 45 30`".to_string();
    };
    let parsed = manual::parse_direction(direction).and_then(|d| speed.map(manual::parse_speed).transpose().map(|s| (d, s)));
    match parsed {
        Ok((direction, speed)) => manual_reply(ctx, ctx.manual.set(Some(direction), speed)),
        Err(err) => format!("error: {err}"),
    }
}

/// `speed N` and `direction D`.
fn manual_one(ctx: &Context, argument: &str, what: &str) -> String {
    if argument.is_empty() {
        return match what {
            "speed" => "error: usage: speed PERCENT   e.g. `speed 70`, `speed +10`, `speed 0`".to_string(),
            _ => "error: usage: direction DIRECTION   e.g. `direction left`, `direction 135`, `direction +15` (turns clockwise)".to_string(),
        };
    }
    let setting = match what {
        "speed" => manual::parse_speed(argument).map(|s| ctx.manual.set(None, Some(s))),
        _ => manual::parse_direction(argument).map(|d| ctx.manual.set(Some(d), None)),
    };
    match setting {
        Ok(setting) => manual_reply(ctx, setting),
        Err(err) => format!("error: {err}"),
    }
}

fn auto(ctx: &Context) -> String {
    let was_manual = ctx.manual.clear();
    let now = if ctx.phone { "follow the phone" } else { "replay the recording" };
    if was_manual { format!("ok: the dots {now} again") } else { format!("ok: already automatic (the dots {now})") }
}

/// The numbered list `phones` prints and `select N` counts in.
fn phones_report(live: &Live, memory: &dyn PhoneMemory) -> String {
    let list = live.phones(Instant::now());
    let (connected, last) = (live.connected_name(), memory.get());
    let (target, _) = live.target();
    let mut text = String::new();
    for (i, phone) in list.iter().enumerate() {
        let signal = phone.rssi.map_or(String::new(), |r| format!("{r} dBm"));
        let mut marks = Vec::new();
        if phone.name.is_some() && phone.name == connected {
            marks.push("connected");
        }
        if phone.name.is_some() && phone.name == last {
            marks.push("last phone");
        }
        let marks = if marks.is_empty() { String::new() } else { format!("  <- {}", marks.join(", ")) };
        text.push_str(&format!("  [{}] {:<28} {:>8}  {}{}\n", i + 1, phone.label(), signal, phone.address, marks));
    }
    if list.is_empty() {
        text.push_str("  no phone heard lately (is the app started on the phone?)\n");
    }
    text.push_str(&format!("following: {}\nswitch with `ams-dots-linux select`", target.describe()));
    text
}

/// `select N`, `select NAME`, `select any`: follow another phone, remember it and connect.
fn select(live: &Live, memory: &dyn PhoneMemory, argument: &str) -> String {
    if argument.is_empty() {
        return "error: select needs a number from `phones`, a phone name, or `any` (run it in a terminal to be asked)".to_string();
    }
    let target = if argument == "any" {
        Target::any()
    } else if let Ok(number) = argument.parse::<usize>() {
        let list = live.phones(Instant::now());
        match number.checked_sub(1).and_then(|i| list.get(i)) {
            Some(Candidate { name: Some(name), .. }) => Target::named(name),
            Some(_) => Target::any(),
            None => return format!("error: no phone number {number}; `phones` shows the list ({} now)", list.len()),
        }
    } else {
        Target::named(argument)
    };
    if let Some(name) = &target.name {
        memory.set(name);
    }
    live.set_target(target.clone());
    live.set_enabled(true);
    format!("ok: following {}", target.describe())
}

/// Runs the command server on a thread of its own, so that it works whether or not there is a Bluetooth receiver
/// and keeps answering whatever the receiver is doing.
pub fn spawn(ctx: Arc<Context>) {
    std::thread::Builder::new()
        .name("ams-dots-control".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("async runtime");
            runtime.block_on(serve(ctx));
        })
        .expect("spawn the control thread");
}

/// Whether an overlay is already answering on the control socket (a socket file left by a crash does not count).
pub fn is_running() -> bool {
    std::os::unix::net::UnixStream::connect(socket_path()).is_ok()
}

/// Accepts commands until the process ends. A socket that cannot be opened is
/// reported and the overlay carries on without remote commands.
async fn serve(ctx: Arc<Context>) {
    let path = socket_path();
    // A socket file left behind by a crashed run would make `bind` fail. (That no other overlay is
    // still using it was checked at start-up, see `is_running`.)
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("ams-dots: cannot listen on {}: {err} (the control commands will not work)", path.display());
            return std::future::pending().await;
        }
    };
    loop {
        let Ok((stream, _)) = listener.accept().await else { continue };
        let ctx = ctx.clone();
        let memory = FileMemory::default_location();
        tokio::spawn(async move {
            let (read, mut write) = stream.into_split();
            let mut line = String::new();
            if AsyncBufReader::new(read).read_line(&mut line).await.is_err() {
                return;
            }
            match line.trim() {
                // A bare connection (`is_running`) says nothing.
                "" => {}
                "watch" => {
                    // Stream until the client goes away (the write then fails).
                    loop {
                        let text = ctx.live.status(Instant::now()).line();
                        if write.write_all(format!("{text}\n").as_bytes()).await.is_err() {
                            break;
                        }
                        tokio::time::sleep(WATCH_INTERVAL).await;
                    }
                }
                "stop" => {
                    let _ = write.write_all(b"ok: stopping\n").await;
                    service::shutdown(&ctx.live, 0);
                }
                _ => {
                    let reply = handle_command(&ctx, &memory, &line);
                    let _ = write.write_all(format!("{reply}\n").as_bytes()).await;
                }
            }
        });
    }
}

fn connect_to_overlay() -> Result<std::os::unix::net::UnixStream, String> {
    let path = socket_path();
    std::os::unix::net::UnixStream::connect(&path)
        .map_err(|e| format!("not running (nothing answers at {}: {e}); start it with `ams-dots-linux`", path.display()))
}

/// Client side of a command: returns the overlay's answer (`status` takes several lines).
pub fn send(command: &str) -> Result<String, String> {
    let mut stream = connect_to_overlay()?;
    writeln!(stream, "{command}").map_err(|e| e.to_string())?;
    let mut reply = String::new();
    std::io::Read::read_to_string(&mut stream, &mut reply).map_err(|e| e.to_string())?;
    Ok(reply.trim_end().to_string())
}

/// Client side of `select` without an argument: shows the overlay's list of phones, asks which one and sends
/// the choice. The overlay stores it and switches.
pub fn select_interactive() -> Result<String, String> {
    let last = picker::remembered();
    let target = picker::ask(
        last.as_deref(),
        |_| {},
        || Ok(picker::from_lines(&send("phones --raw")?)),
        &mut std::io::stdin().lock(),
        &mut std::io::stdout(),
    )?;
    match target {
        Some(Target { name: Some(name) }) => send(&format!("select {name}")),
        Some(Target { name: None }) => send("select any"),
        None => Ok("nothing changed".to_string()),
    }
}

/// Client side of `status --watch`: one refreshing line until Ctrl+C or the overlay stops.
pub fn watch() -> Result<(), String> {
    let mut stream = connect_to_overlay()?;
    writeln!(stream, "watch").map_err(|e| e.to_string())?;
    for line in BufReader::new(stream).lines() {
        let line = line.map_err(|e| e.to_string())?;
        // \r returns to the start of the line, \x1b[K erases what is left of the longer old one.
        print!("\r{line}\x1b[K");
        std::io::stdout().flush().map_err(|e| e.to_string())?;
    }
    println!();
    Err("the overlay stopped".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::LinkState;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Memory(RefCell<Option<String>>);

    impl PhoneMemory for Memory {
        fn get(&self) -> Option<String> {
            self.0.borrow().clone()
        }
        fn set(&self, name: &str) {
            *self.0.borrow_mut() = Some(name.to_string());
        }
        fn forget(&self) {
            *self.0.borrow_mut() = None;
        }
    }

    fn phone(name: &str, rssi: i16) -> Candidate {
        Candidate { name: Some(name.to_string()), address: format!("addr-{name}"), rssi: Some(rssi) }
    }

    /// A receiver that has heard two phones.
    fn hearing_two() -> std::sync::Arc<Live> {
        let live = Live::new();
        live.report_sightings(&[phone("Redmi", -60), phone("Pixel", -40)], Instant::now());
        live
    }

    fn context(live: &Arc<Live>) -> Context {
        let settings = Settings::load(Some(std::env::var("CARGO_MANIFEST_DIR").unwrap() + "/config.example.toml")).unwrap();
        Context { live: live.clone(), manual: Manual::new(), settings, phone: true }
    }

    fn run(live: &Arc<Live>, memory: &Memory, line: &str) -> String {
        handle_command(&context(live), memory, line)
    }

    fn ask(ctx: &Context, line: &str) -> String {
        handle_command(ctx, &Memory::default(), line)
    }

    #[test]
    fn status_reports_the_link_the_target_and_the_last_phone() {
        let (live, memory) = (Live::new(), Memory::default());
        let report = run(&live, &memory, "status\n");
        assert!(report.contains("scanning for the first phone found"), "{report}");
        assert!(report.contains("last phone: -"), "{report}");
        memory.set("Redmi");
        live.set_target(Target::named("Redmi"));
        let report = run(&live, &memory, "status");
        assert!(report.contains("scanning for \"Redmi\""), "{report}");
        assert!(report.contains("last phone: Redmi"), "{report}");
        live.set_link(LinkState::Connected);
        assert!(run(&live, &memory, "status").contains("connected"));
    }

    #[test]
    fn phones_lists_the_strongest_first_and_marks_the_last_one() {
        let (live, memory) = (hearing_two(), Memory::default());
        memory.set("Redmi");
        let text = run(&live, &memory, "phones");
        let (first, second) = (text.lines().next().unwrap(), text.lines().nth(1).unwrap());
        assert!(first.contains("[1]") && first.contains("Pixel") && first.contains("-40 dBm"), "{text}");
        assert!(second.contains("[2]") && second.contains("Redmi") && second.contains("last phone"), "{text}");
        assert!(text.contains("following: the first phone found"), "{text}");
    }

    #[test]
    fn phones_without_any_says_so() {
        let text = run(&Live::new(), &Memory::default(), "phones");
        assert!(text.contains("no phone heard lately"), "{text}");
    }

    #[test]
    fn the_connected_phone_stays_in_the_list_though_it_no_longer_advertises() {
        let (live, memory) = (Live::new(), Memory::default());
        live.set_phone(&phone("Redmi", -60));
        live.set_link(LinkState::Connected);
        let text = run(&live, &memory, "phones");
        assert!(text.contains("Redmi") && text.contains("connected"), "{text}");
    }

    #[test]
    fn the_raw_list_is_what_the_interactive_select_reads() {
        let live = hearing_two();
        let back = picker::from_lines(&run(&live, &Memory::default(), "phones --raw"));
        assert_eq!(back.iter().map(|c| c.label()).collect::<Vec<_>>(), ["Pixel", "Redmi"]);
    }

    #[test]
    fn select_by_number_switches_remembers_and_wakes_the_receiver() {
        let (live, memory) = (hearing_two(), Memory::default());
        live.set_enabled(false);
        let generation = live.target().1;
        assert_eq!(run(&live, &memory, "select 2"), "ok: following \"Redmi\"");
        assert_eq!(live.target().0, Target::named("Redmi"));
        assert!(live.target().1 > generation, "the receiver must notice the change");
        assert_eq!(memory.get().as_deref(), Some("Redmi"));
        assert!(live.is_enabled(), "selecting a phone also turns the receiver back on");
    }

    #[test]
    fn select_by_name_works_for_a_phone_not_in_range() {
        let (live, memory) = (Live::new(), Memory::default());
        assert!(run(&live, &memory, "select Redmi Note 12 Pro 5G").starts_with("ok"));
        assert_eq!(live.target().0, Target::named("Redmi Note 12 Pro 5G"));
        assert_eq!(memory.get().as_deref(), Some("Redmi Note 12 Pro 5G"));
    }

    #[test]
    fn select_any_follows_the_first_phone_and_keeps_the_memory() {
        let (live, memory) = (Live::new(), Memory::default());
        memory.set("Redmi");
        live.set_target(Target::named("Redmi"));
        assert!(run(&live, &memory, "select any").starts_with("ok"));
        assert_eq!(live.target().0, Target::any());
        assert_eq!(memory.get().as_deref(), Some("Redmi"));
    }

    #[test]
    fn select_refuses_a_bad_number_and_a_missing_argument() {
        let (live, memory) = (hearing_two(), Memory::default());
        let generation = live.target().1;
        for line in ["select 0", "select 3", "select"] {
            assert!(run(&live, &memory, line).starts_with("error"), "{line}");
        }
        assert_eq!(live.target().1, generation, "a refused select changes nothing");
        assert_eq!(memory.get(), None);
    }

    #[test]
    fn forget_clears_the_memory_but_not_the_running_target() {
        let (live, memory) = (Live::new(), Memory::default());
        memory.set("Redmi");
        live.set_target(Target::named("Redmi"));
        assert!(run(&live, &memory, "forget").starts_with("ok"));
        assert_eq!(memory.get(), None);
        assert_eq!(live.target().0, Target::named("Redmi"));
    }

    #[test]
    fn connect_and_disconnect_switch_the_receiver() {
        let (live, memory) = (Live::new(), Memory::default());
        assert!(run(&live, &memory, "disconnect").starts_with("ok"));
        assert!(!live.is_enabled());
        assert!(run(&live, &memory, "connect").starts_with("ok"));
        assert!(live.is_enabled());
    }

    #[test]
    fn relearn_is_accepted() {
        assert!(run(&Live::new(), &Memory::default(), "relearn").starts_with("ok"));
    }

    #[test]
    fn move_switches_manual_control_on_and_auto_off() {
        let ctx = context(&Live::new());
        assert_eq!(ctx.manual.current(), None);
        let reply = ask(&ctx, "move left 80");
        assert!(reply.starts_with("ok: dots flow left at 80% (200 px/s)"), "{reply}");
        assert_eq!(ctx.manual.current(), Some(manual::Setting { direction: 270.0, speed: 80.0 }));
        assert!(ask(&ctx, "auto").starts_with("ok: the dots follow the phone again"));
        assert_eq!(ctx.manual.current(), None);
        assert!(ask(&ctx, "auto").contains("already automatic"));
    }

    #[test]
    fn move_without_a_speed_keeps_the_current_one() {
        let ctx = context(&Live::new());
        ask(&ctx, "move up 30");
        ask(&ctx, "move right");
        assert_eq!(ctx.manual.current(), Some(manual::Setting { direction: 90.0, speed: 30.0 }));
    }

    #[test]
    fn speed_and_direction_change_one_thing_at_a_time() {
        let ctx = context(&Live::new());
        ask(&ctx, "move down 50");
        assert!(ask(&ctx, "speed +10").starts_with("ok"));
        assert!(ask(&ctx, "direction +90").starts_with("ok"));
        assert_eq!(ctx.manual.current(), Some(manual::Setting { direction: 270.0, speed: 60.0 }));
        ask(&ctx, "speed 0");
        assert_eq!(ctx.manual.current().map(|s| s.speed), Some(0.0), "speed 0 is manual and still, not automatic");
    }

    #[test]
    fn manual_commands_refuse_nonsense_and_change_nothing() {
        let ctx = context(&Live::new());
        for line in ["move", "move sideways", "move down fast", "move down 50 extra", "speed", "speed fast", "direction", "direction nowhere"] {
            assert!(ask(&ctx, line).starts_with("error"), "{line}");
        }
        assert_eq!(ctx.manual.current(), None);
    }

    #[test]
    fn status_says_who_drives_the_dots() {
        let ctx = context(&Live::new());
        assert!(ask(&ctx, "status").contains("dots:     at rest (no phone connected"));
        ctx.live.set_link(crate::motion::LinkState::Connected);
        assert!(ask(&ctx, "status").contains("dots:     follow the phone"));
        ask(&ctx, "move down-right 100");
        let status = ask(&ctx, "status");
        assert!(status.contains("moved by hand: down-right at 100% (250 px/s)"), "{status}");
    }

    #[test]
    fn reload_reports_a_refused_file_and_keeps_running() {
        let path = std::env::temp_dir().join(format!("ams-dots-control-test-{}.toml", std::process::id()));
        std::fs::write(&path, "[dots]\nsize = 9.0\n").unwrap();
        let settings = Settings::load(Some(path.display().to_string())).unwrap();
        let ctx = Context { settings, ..context(&Live::new()) };
        assert!(ask(&ctx, "reload").starts_with("ok: applied"));

        std::fs::write(&path, "[dots]\nsize = 0.0\n").unwrap();
        let reply = ask(&ctx, "reload");
        assert!(reply.starts_with("error") && reply.contains("dots.size") && reply.contains("unchanged"), "{reply}");
        assert_eq!(ctx.settings.config().dots.size, 9.0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn replaying_a_recording_has_no_phone_to_connect() {
        let ctx = Context { phone: false, ..context(&Live::new()) };
        assert!(ask(&ctx, "connect").starts_with("error"));
        assert!(ask(&ctx, "select any").starts_with("error"));
        assert!(ask(&ctx, "status").contains("replaying a recording"));
        assert!(ask(&ctx, "move up").starts_with("ok"), "manual control works with a recording too");
    }

    #[test]
    fn unknown_commands_are_refused() {
        assert!(run(&Live::new(), &Memory::default(), "reboot").starts_with("error: unknown command"));
        // The old manual calibration is gone: forward is learned.
        assert!(run(&Live::new(), &Memory::default(), "calibrate").starts_with("error"));
    }
}

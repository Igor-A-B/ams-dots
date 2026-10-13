//! Running as a background service: `ams-dots-linux` starts the overlay detached from the terminal and
//! returns at once; everything afterwards (`move`, `select`, `reload`, `stop`, ...) talks to it over the
//! control socket, so changing a setting never means stopping and starting again.
//!
//! Starting: the launcher runs the same program again with `--foreground` in a new session (so closing the
//! terminal does not end it), with its output going to a log file, and waits until the new process answers on
//! the control socket. The control socket only opens once the overlay is up, so a failure to start
//! (no Wayland, no layer-shell) is reported right there with the end of the log.
//!
//! Stopping: `stop` (or SIGINT / SIGTERM) disconnects the phone cleanly, removes the socket and exits.

use crate::control;
use crate::motion::Live;
use crate::picker;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long the launcher waits for the background process to come up.
const STARTUP_WAIT: Duration = Duration::from_secs(10);
/// How long `stop` waits for the process to go away.
const STOP_WAIT: Duration = Duration::from_secs(5);
/// How long a shutdown waits for the phone to be disconnected (a killed process would leave the link open in BlueZ).
const DISCONNECT_WAIT: Duration = Duration::from_secs(2);
/// Lines of the log shown when the background process fails to start.
const LOG_TAIL_LINES: usize = 8;

/// Where the background process writes its messages (`ams-dots.log.old` holds the run before).
pub fn log_path() -> Option<PathBuf> {
    picker::state_dir().map(|dir| dir.join("ams-dots.log"))
}

/// Starts the overlay in the background with `args` (the options of the foreground run) and returns once it
/// is up: the message to show, or why it did not start.
pub fn launch(args: &[String]) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot find the program to start: {e}"))?;
    let log = log_path().ok_or("cannot tell where to keep the log (HOME is not set)")?;
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let _ = std::fs::rename(&log, log.with_extension("log.old"));
    let file = std::fs::File::create(&log).map_err(|e| format!("cannot write {}: {e}", log.display()))?;
    let errors = file.try_clone().map_err(|e| e.to_string())?;

    let mut command = Command::new(exe);
    command.arg("--foreground").args(args).stdin(Stdio::null()).stdout(file).stderr(errors);
    // SAFETY: only async-signal-safe calls (setsid) between fork and exec. A new session means the terminal's
    // hang-up and Ctrl+C no longer reach the service.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(|e| format!("cannot start the background process: {e}"))?;

    let deadline = Instant::now() + STARTUP_WAIT;
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return Err(format!("the background process stopped right away ({status}). End of {}:\n{}", log.display(), log_tail(&log)));
        }
        if control::is_running() {
            return Ok(format!("started in the background (pid {}); messages go to {}", child.id(), log.display()));
        }
        if Instant::now() >= deadline {
            return Err(format!("the background process (pid {}) is not answering after {}s; see {}", child.id(), STARTUP_WAIT.as_secs(), log.display()));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn log_tail(log: &std::path::Path) -> String {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(LOG_TAIL_LINES)..].join("\n")
}

/// Asks the running overlay to quit and waits until it has.
pub fn stop() -> Result<String, String> {
    if !control::is_running() {
        return Ok("ams-dots is not running".to_string());
    }
    control::send("stop")?;
    let deadline = Instant::now() + STOP_WAIT;
    while control::is_running() {
        if Instant::now() >= deadline {
            return Err(format!("ams-dots did not stop within {}s", STOP_WAIT.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok("stopped".to_string())
}

/// Disconnects the phone, removes the control socket and ends the process.
pub fn shutdown(live: &Live, exit_code: i32) -> ! {
    live.set_enabled(false);
    let deadline = Instant::now() + DISCONNECT_WAIT;
    while live.is_connected() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = std::fs::remove_file(control::socket_path());
    std::process::exit(exit_code);
}

/// Makes Ctrl+C and SIGTERM shut down cleanly (see [`shutdown`]). Killing the process while the phone is
/// connected would leave the connection open in BlueZ: the phone then stops advertising and the next run
/// cannot find it.
pub fn shutdown_on_signals(live: Arc<Live>) {
    use signal_hook::consts::{SIGINT, SIGTERM};
    let Ok(mut signals) = signal_hook::iterator::Signals::new([SIGINT, SIGTERM]) else { return };
    std::thread::spawn(move || {
        if signals.forever().next().is_some() {
            shutdown(&live, 130);
        }
    });
}

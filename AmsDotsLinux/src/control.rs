//! Talking to the running overlay from a terminal or a key binding.
//!
//! The overlay listens on a Unix socket; `ams-dots-linux <command>` connects,
//! sends one line and prints the answer:
//!
//! | Command | Effect |
//! |---|---|
//! | `status` | link state, phone, packet statistics, motion, how sure the forward direction is |
//! | `status --watch` | the same as one line that refreshes five times a second (shake the phone to see it move) |
//! | `connect` | (re)start looking for the phone |
//! | `disconnect` | drop the phone and stop looking until `connect` |
//! | `relearn` | forget the learned forward direction, e.g. after moving the phone |
//!
//! Example key binding for Hyprland: `bind = $mod, F9, exec, ams-dots-linux disconnect`.

use crate::motion::Live;
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

/// What to answer one (non-streaming) command line. Kept separate from the socket so it can be tested.
pub fn handle_command(live: &Live, line: &str) -> String {
    match line.trim() {
        "status" => live.status(Instant::now()).report(),
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
        other => format!("error: unknown command {other:?} (known: status, status --watch, connect, disconnect, relearn)"),
    }
}

/// Accepts commands until the process ends. A socket that cannot be opened is
/// reported and the overlay carries on without remote commands.
pub async fn serve(live: Arc<Live>) {
    let path = socket_path();
    // A socket file left behind by a crashed run would make `bind` fail.
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
        let live = live.clone();
        tokio::spawn(async move {
            let (read, mut write) = stream.into_split();
            let mut line = String::new();
            if AsyncBufReader::new(read).read_line(&mut line).await.is_err() {
                return;
            }
            if line.trim() == "watch" {
                // Stream until the client goes away (the write then fails).
                loop {
                    let text = live.status(Instant::now()).line();
                    if write.write_all(format!("{text}\n").as_bytes()).await.is_err() {
                        break;
                    }
                    tokio::time::sleep(WATCH_INTERVAL).await;
                }
            } else {
                let reply = handle_command(&live, &line);
                let _ = write.write_all(format!("{reply}\n").as_bytes()).await;
            }
        });
    }
}

fn connect_to_overlay() -> Result<std::os::unix::net::UnixStream, String> {
    let path = socket_path();
    std::os::unix::net::UnixStream::connect(&path)
        .map_err(|e| format!("cannot reach the overlay at {} ({e}); is it running?", path.display()))
}

/// Client side of a command: returns the overlay's answer (`status` takes several lines).
pub fn send(command: &str) -> Result<String, String> {
    let mut stream = connect_to_overlay()?;
    writeln!(stream, "{command}").map_err(|e| e.to_string())?;
    let mut reply = String::new();
    std::io::Read::read_to_string(&mut stream, &mut reply).map_err(|e| e.to_string())?;
    Ok(reply.trim_end().to_string())
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

    #[test]
    fn status_reports_the_link() {
        let live = Live::new();
        let report = handle_command(&live, "status\n");
        assert!(report.contains("scanning"), "{report}");
        live.set_link(LinkState::Connected);
        assert!(handle_command(&live, "status").contains("connected"));
    }

    #[test]
    fn connect_and_disconnect_switch_the_receiver() {
        let live = Live::new();
        assert!(handle_command(&live, "disconnect").starts_with("ok"));
        assert!(!live.is_enabled());
        assert!(handle_command(&live, "connect").starts_with("ok"));
        assert!(live.is_enabled());
    }

    #[test]
    fn relearn_is_accepted() {
        assert!(handle_command(&Live::new(), "relearn").starts_with("ok"));
    }

    #[test]
    fn unknown_commands_are_refused() {
        assert!(handle_command(&Live::new(), "reboot").starts_with("error: unknown command"));
        // The old manual calibration is gone: forward is learned.
        assert!(handle_command(&Live::new(), "calibrate").starts_with("error"));
    }
}

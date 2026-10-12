//! Talking to the running overlay from another terminal or a key binding.
//!
//! The overlay listens on a Unix socket; `ams-dots-linux calibrate` connects,
//! sends one line and prints the answer. This is how the *calibrate* command
//! reaches the fusion, e.g. from Hyprland:
//!
//! ```text
//! bind = $mod, F9, exec, ams-dots-linux calibrate
//! ```

use crate::motion::Live;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as AsyncBufReader};
use tokio::net::UnixListener;

/// Socket in the user's runtime directory (`/run/user/<uid>`), which only they can use.
pub fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    dir.join("ams-dots.sock")
}

/// What to answer one command line. Kept separate from the socket so it can be tested.
pub fn handle_command(live: &Live, line: &str) -> String {
    match line.trim() {
        "calibrate" => {
            if !live.is_connected() {
                return "error: the phone is not connected".to_string();
            }
            live.calibrate();
            "ok: calibrating. Accelerate gently in a straight line now; the result is printed by the overlay".to_string()
        }
        other => format!("error: unknown command {other:?} (known: calibrate)"),
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
            eprintln!("ams-dots: cannot listen on {}: {err} (the calibrate command will not work)", path.display());
            return std::future::pending().await;
        }
    };
    loop {
        let Ok((stream, _)) = listener.accept().await else { continue };
        let live = live.clone();
        tokio::spawn(async move {
            let (read, mut write) = stream.into_split();
            let mut line = String::new();
            if AsyncBufReader::new(read).read_line(&mut line).await.is_ok() {
                let reply = handle_command(&live, &line);
                let _ = write.write_all(format!("{reply}\n").as_bytes()).await;
            }
        });
    }
}

/// The `calibrate` subcommand: asks the running overlay and prints its answer.
pub fn send(command: &str) -> Result<String, String> {
    let path = socket_path();
    let mut stream = std::os::unix::net::UnixStream::connect(&path)
        .map_err(|e| format!("cannot reach the overlay at {} ({e}); is it running?", path.display()))?;
    writeln!(stream, "{command}").map_err(|e| e.to_string())?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply).map_err(|e| e.to_string())?;
    Ok(reply.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Packet, RawSample};
    use std::time::Instant;

    #[test]
    fn calibrate_needs_a_connected_phone() {
        let live = Live::new();
        assert!(handle_command(&live, "calibrate\n").starts_with("error"));
        live.set_connected(true);
        assert!(handle_command(&live, "calibrate\n").starts_with("ok"));
    }

    #[test]
    fn unknown_commands_are_refused() {
        assert!(handle_command(&Live::new(), "reboot").starts_with("error: unknown command"));
    }

    #[test]
    fn an_accepted_calibrate_really_arms_the_fusion() {
        let live = Live::new();
        live.set_connected(true);
        handle_command(&live, "calibrate");
        let sample = RawSample { accel_mm_s2: [2000, 0, 0], gyro_mrad_s: [0; 3] };
        let packet = |sequence| Packet { sequence, first_sample_ms: 0, gravity_mg: [0, 0, 1000], samples: vec![sample; 4] };
        let mut done = false;
        for seq in 0..30 {
            done |= live.push(&packet(seq), Instant::now()).calibration.is_some();
        }
        assert!(done);
    }
}

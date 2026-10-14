//! The command line: the options for starting the overlay and the text of `--help`.

use crate::protocol;

pub const USAGE: &str = "\
usage: ams-dots-linux [start] [options] [recording.csv]   start in the background (and return)
       ams-dots-linux stop | restart [options]
       ams-dots-linux move DIRECTION [SPEED] | speed N | direction D | auto
       ams-dots-linux reload
       ams-dots-linux status [--watch] | phones | select [N|NAME|any] | forget | connect | disconnect | relearn

Shows dots along the screen edges that move with the vehicle, as seen by the phone
running the AmsDots app, or as you set them by hand. Starting puts the overlay in the
background and gives the terminal back; every other command talks to that running
overlay, so nothing needs restarting to change something. The first start lists the
phones in range and asks which one to follow; after that it connects to the last
phone by itself.

options (for start and restart):
  --foreground   stay in this terminal (messages on screen, Ctrl+C stops); for debugging or a service manager
  --phone NAME   follow the phone with this Bluetooth name, or `any`; no question asked
  --pick         list the phones and ask, even if a last phone is remembered
  --rate HZ      ask the phone to sample at HZ (10-100; its default is 50)
  --record FILE  write every sample from the phone (raw and fused) to FILE as CSV
  --config FILE  use another config file (default ~/.config/ams-dots/config.toml)
  --no-phone     start without Bluetooth (dots at rest, or as set by hand); `connect` adds the phone later
  recording.csv  no Bluetooth: replay a recorded drive (tests/data/drive.csv)
  -h, --help     this text

manual control (works with or without a phone; `auto` gives control back to the phone):
  move DIR [SPEED]  dots flow towards DIR at SPEED percent of the maximum speed (default 50), e.g. `move down 60`
  speed N           change the speed: `speed 80`, `speed +10`, `speed -10`, `speed 0`
  direction D       change the direction: `direction left`, `direction 135`, `direction +15` (turn clockwise)
  auto              the phone drives the dots again
  DIR is up, down, left, right, up-left, up-right, down-left, down-right, or degrees clockwise from the top (90 = right)

commands:
  stop             end the overlay (also disconnects the phone)
  restart          stop, then start again (with these options)
  reload           read the config file again and apply it
  status           link state, phone, what drives the dots, packet rate, forward direction
  status --watch   the same as one live line (shake the phone and watch it move)
  phones           phones heard lately (numbered), marking the connected and the last one
  select           switch phone: shows the list and asks; or `select 2`, `select NAME`, `select any`
  forget           forget the last phone (the next start asks again)
  connect          start looking for the phone again
  disconnect       drop the phone and stop looking until `connect`
  relearn          forget the learned forward direction (after moving the phone)";

/// How the overlay is started.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Options {
    pub config_path: Option<String>,
    pub recording_path: Option<String>,
    pub phone_name: Option<String>,
    pub rate_hz: Option<u8>,
    pub record_path: Option<String>,
    pub no_phone: bool,
    pub pick: bool,
    pub foreground: bool,
}

#[derive(Debug, PartialEq)]
pub enum Parsed {
    Help,
    Start(Options),
}

impl Options {
    pub fn parse(args: &[String]) -> Result<Parsed, String> {
        let mut options = Options::default();
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let mut value = |what: &str| args.next().cloned().ok_or(format!("{arg} needs {what}\n\n{USAGE}"));
            match arg.as_str() {
                "-h" | "--help" => return Ok(Parsed::Help),
                "--foreground" => options.foreground = true,
                "--config" => options.config_path = Some(value("a file")?),
                "--phone" => options.phone_name = Some(value("a name (or `any`)")?),
                "--rate" => {
                    let text = value("a number of Hz")?;
                    let hz: u8 = text.parse().map_err(|_| format!("--rate: {text:?} is not a number of Hz\n\n{USAGE}"))?;
                    if !(protocol::MIN_RATE_HZ..=protocol::MAX_RATE_HZ).contains(&hz) {
                        return Err(format!("--rate must be {}-{} Hz", protocol::MIN_RATE_HZ, protocol::MAX_RATE_HZ));
                    }
                    options.rate_hz = Some(hz);
                }
                "--record" => options.record_path = Some(value("a file")?),
                "--pick" => options.pick = true,
                "--no-phone" => options.no_phone = true,
                _ if arg.starts_with('-') => return Err(format!("unknown option {arg}\n\n{USAGE}")),
                _ => options.recording_path = Some(arg.clone()),
            }
        }
        Ok(Parsed::Start(options))
    }

    /// The options for the background process, which has no terminal and may not share this one's working
    /// directory for long: files are given by absolute path, and `--pick` is dropped (it was answered here).
    pub fn background_args(&self) -> Result<Vec<String>, String> {
        let absolute = |what: &str, path: &str| {
            std::path::absolute(path).map(|p| p.display().to_string()).map_err(|e| format!("{what} {path}: {e}"))
        };
        let mut args = Vec::new();
        if let Some(path) = &self.config_path {
            args.extend(["--config".to_string(), absolute("--config", path)?]);
        }
        if let Some(name) = &self.phone_name {
            args.extend(["--phone".to_string(), name.clone()]);
        }
        if let Some(hz) = self.rate_hz {
            args.extend(["--rate".to_string(), hz.to_string()]);
        }
        if let Some(path) = &self.record_path {
            args.extend(["--record".to_string(), absolute("--record", path)?]);
        }
        if self.no_phone {
            args.push("--no-phone".to_string());
        }
        if let Some(path) = &self.recording_path {
            args.push(absolute("recording", path)?);
        }
        Ok(args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Parsed, String> {
        Options::parse(&args.iter().map(|a| a.to_string()).collect::<Vec<_>>())
    }

    fn options(args: &[&str]) -> Options {
        match parse(args) {
            Ok(Parsed::Start(options)) => options,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn no_arguments_start_with_the_defaults() {
        assert_eq!(options(&[]), Options::default());
    }

    #[test]
    fn every_option_is_understood() {
        let o = options(&["--foreground", "--phone", "Pixel", "--rate", "30", "--record", "r.csv", "--config", "c.toml", "--pick", "--no-phone", "drive.csv"]);
        assert_eq!(
            o,
            Options {
                config_path: Some("c.toml".into()),
                recording_path: Some("drive.csv".into()),
                phone_name: Some("Pixel".into()),
                rate_hz: Some(30),
                record_path: Some("r.csv".into()),
                no_phone: true,
                pick: true,
                foreground: true,
            }
        );
    }

    #[test]
    fn help_wins_and_mistakes_are_explained() {
        assert_eq!(parse(&["--no-phone", "-h"]), Ok(Parsed::Help));
        assert!(parse(&["--wat"]).unwrap_err().contains("unknown option --wat"));
        assert!(parse(&["--phone"]).unwrap_err().contains("--phone needs"));
        assert!(parse(&["--rate", "fast"]).is_err());
        assert!(parse(&["--rate", "500"]).is_err());
    }

    #[test]
    fn the_background_gets_absolute_paths_and_no_pick() {
        let o = options(&["--pick", "--config", "my.toml", "--no-phone", "--rate", "25", "--phone", "any"]);
        let args = o.background_args().unwrap();
        assert!(!args.contains(&"--pick".to_string()));
        let at = args.iter().position(|a| a == "--config").unwrap();
        assert!(std::path::Path::new(&args[at + 1]).is_absolute(), "{args:?}");
        assert!(args.contains(&"--no-phone".to_string()));
        // What the background process is given must parse back to the same choices.
        let mut again = vec!["--foreground".to_string()];
        again.extend(args);
        let back = match Options::parse(&again) {
            Ok(Parsed::Start(back)) => back,
            other => panic!("{other:?}"),
        };
        assert!(back.foreground && back.no_phone && !back.pick);
        assert_eq!((back.rate_hz, back.phone_name.as_deref()), (Some(25), Some("any")));
    }
}

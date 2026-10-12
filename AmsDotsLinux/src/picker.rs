//! Choosing which phone to follow.
//!
//! On start the overlay scans for a few seconds and lists the phones that run
//! the AmsDots app, then asks which one to use. The choice is remembered, so
//! next time Enter is enough. Without a terminal (autostart) or with
//! `--phone NAME`, nothing is asked.
//!
//! Phones are told apart by their Bluetooth name, not their address: the phone
//! advertises with a random address that changes every time the app starts its
//! link, so an address is useless for finding the same phone again.

use std::io::{BufRead, Write};
use std::path::PathBuf;

/// A phone seen during a scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Bluetooth name from the advertisement, if the phone sent one.
    pub name: Option<String>,
    /// Current (temporary) address, as text.
    pub address: String,
    /// Signal strength in dBm; closer to 0 is nearer.
    pub rssi: Option<i16>,
}

impl Candidate {
    pub fn label(&self) -> String {
        self.name.clone().unwrap_or_else(|| "(unnamed phone)".to_string())
    }
}

/// Which phone the receiver should connect to.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Target {
    /// Match by Bluetooth name; `None` means any phone.
    pub name: Option<String>,
}

impl Target {
    pub fn any() -> Self {
        Self { name: None }
    }

    pub fn named(name: &str) -> Self {
        Self { name: Some(name.to_string()) }
    }

    pub fn matches(&self, candidate: &Candidate) -> bool {
        match &self.name {
            Some(wanted) => candidate.name.as_deref() == Some(wanted.as_str()),
            None => true,
        }
    }

    pub fn describe(&self) -> String {
        self.name.as_ref().map_or("the first phone found".to_string(), |n| format!("\"{n}\""))
    }
}

/// What the user typed at the prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    /// Take the n-th entry of the list (0-based).
    Pick(usize),
    Rescan,
    /// Follow the first phone that shows up.
    Any,
    Quit,
}

/// Interprets one line of input. `default` is what a bare Enter means (an index into the list).
/// Anything not understood gives `None` and the prompt is shown again.
pub fn parse_choice(input: &str, count: usize, default: Option<usize>) -> Option<Choice> {
    let text = input.trim().to_lowercase();
    match text.as_str() {
        "" => default.map(Choice::Pick).or(if count == 0 { Some(Choice::Rescan) } else { None }),
        "r" => Some(Choice::Rescan),
        "a" => Some(Choice::Any),
        "q" => Some(Choice::Quit),
        number => number.parse::<usize>().ok().filter(|n| (1..=count).contains(n)).map(|n| Choice::Pick(n - 1)),
    }
}

/// Where the last choice is stored (`$XDG_STATE_HOME/ams-dots/phone`, default `~/.local/state/ams-dots/phone`).
fn remembered_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(base.join("ams-dots").join("phone"))
}

/// The phone chosen last time, if any.
pub fn remembered() -> Option<String> {
    let text = std::fs::read_to_string(remembered_path()?).ok()?;
    let name = text.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// Stores the choice for next time. Failing to store it is not worth interrupting the user for.
pub fn remember(name: &str) {
    if let Some(path) = remembered_path() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, format!("{name}\n"));
    }
}

/// Prints the list. Returns the index Enter should pick.
pub fn show_list(candidates: &[Candidate], remembered: Option<&str>, out: &mut impl Write) -> Option<usize> {
    let default = remembered
        .and_then(|name| candidates.iter().position(|c| c.name.as_deref() == Some(name)))
        .or(if candidates.len() == 1 { Some(0) } else { None });
    for (i, c) in candidates.iter().enumerate() {
        let signal = c.rssi.map_or(String::new(), |r| format!("{r} dBm"));
        let mark = if Some(i) == default { " <- Enter" } else { "" };
        let _ = writeln!(out, "  [{}] {:<28} {:>8}  {}{}", i + 1, c.label(), signal, c.address, mark);
    }
    default
}

/// Asks the user which phone to follow. `scan` finds the phones (called again on "r"),
/// `last` is the phone chosen before and `store` saves a new choice.
/// Returns the target, or `None` if the user quit.
pub fn ask(
    last: Option<&str>,
    mut store: impl FnMut(&str),
    mut scan: impl FnMut() -> Result<Vec<Candidate>, String>,
    input: &mut impl BufRead,
    out: &mut impl Write,
) -> Result<Option<Target>, String> {
    loop {
        let _ = writeln!(out, "Looking for phones running the AmsDots app (press Start in the app on the phone)...");
        let candidates = scan()?;
        if candidates.is_empty() {
            let _ = writeln!(out, "No phone found.");
        }
        let default = show_list(&candidates, last, out);
        let hint = match (default, candidates.is_empty()) {
            (Some(_), _) => "number or Enter to connect",
            (None, true) => "Enter or r to scan again",
            (None, false) => "number to connect",
        };
        loop {
            let _ = write!(out, "{hint}, r = rescan, a = first phone that appears, q = quit: ");
            let _ = out.flush();
            let mut line = String::new();
            if input.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
                return Ok(None); // end of input (Ctrl+D)
            }
            match parse_choice(&line, candidates.len(), default) {
                Some(Choice::Pick(i)) => {
                    let chosen = &candidates[i];
                    return Ok(Some(match &chosen.name {
                        Some(name) => {
                            store(name);
                            Target::named(name)
                        }
                        None => Target::any(),
                    }));
                }
                Some(Choice::Any) => return Ok(Some(Target::any())),
                Some(Choice::Quit) => return Ok(None),
                Some(Choice::Rescan) => break,
                None => {
                    let _ = writeln!(out, "Not understood.");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn phone(name: &str, rssi: i16) -> Candidate {
        Candidate { name: Some(name.to_string()), address: "AA:BB:CC:DD:EE:FF".to_string(), rssi: Some(rssi) }
    }

    #[test]
    fn numbers_pick_entries_one_based() {
        assert_eq!(parse_choice("2", 3, None), Some(Choice::Pick(1)));
        assert_eq!(parse_choice(" 1 \n", 3, None), Some(Choice::Pick(0)));
    }

    #[test]
    fn out_of_range_or_garbage_is_not_understood() {
        assert_eq!(parse_choice("0", 3, None), None);
        assert_eq!(parse_choice("4", 3, None), None);
        assert_eq!(parse_choice("banana", 3, None), None);
        assert_eq!(parse_choice("-1", 3, None), None);
    }

    #[test]
    fn letters_rescan_any_and_quit() {
        assert_eq!(parse_choice("r", 0, None), Some(Choice::Rescan));
        assert_eq!(parse_choice("A", 2, None), Some(Choice::Any));
        assert_eq!(parse_choice("q", 2, None), Some(Choice::Quit));
    }

    #[test]
    fn enter_takes_the_default_or_rescans_an_empty_list() {
        assert_eq!(parse_choice("", 2, Some(1)), Some(Choice::Pick(1)));
        assert_eq!(parse_choice("", 2, None), None, "with several phones and no default the user must choose");
        assert_eq!(parse_choice("", 0, None), Some(Choice::Rescan));
    }

    #[test]
    fn targets_match_by_name() {
        let a = phone("Pixel", -50);
        assert!(Target::any().matches(&a));
        assert!(Target::named("Pixel").matches(&a));
        assert!(!Target::named("Redmi").matches(&a));
        let unnamed = Candidate { name: None, ..a };
        assert!(Target::any().matches(&unnamed));
        assert!(!Target::named("Pixel").matches(&unnamed));
    }

    #[test]
    fn a_single_phone_is_the_default() {
        let mut out = Vec::new();
        assert_eq!(show_list(&[phone("Pixel", -50)], None, &mut out), Some(0));
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("[1]") && text.contains("Pixel") && text.contains("-50 dBm") && text.contains("Enter"), "{text}");
    }

    #[test]
    fn several_phones_have_no_default_unless_remembered() {
        let list = [phone("Pixel", -50), phone("Redmi", -60)];
        assert_eq!(show_list(&list, None, &mut Vec::new()), None);
        assert_eq!(show_list(&list, Some("Redmi"), &mut Vec::new()), Some(1));
        assert_eq!(show_list(&list, Some("Gone"), &mut Vec::new()), None);
    }

    #[test]
    fn the_dialogue_picks_a_phone_by_number() {
        let list = vec![phone("Pixel", -50), phone("Redmi", -60)];
        let mut input = Cursor::new("7\n2\n");
        let mut out = Vec::new();
        let mut stored = Vec::new();
        let target = ask(None, |n| stored.push(n.to_string()), || Ok(list.clone()), &mut input, &mut out).unwrap();
        assert_eq!(target, Some(Target::named("Redmi")));
        assert_eq!(stored, ["Redmi"], "the choice is remembered");
        assert!(String::from_utf8(out).unwrap().contains("Not understood"));
    }

    #[test]
    fn enter_takes_the_remembered_phone() {
        let list = vec![phone("Pixel", -50), phone("Redmi", -60)];
        let target = ask(Some("Redmi"), |_| {}, || Ok(list.clone()), &mut Cursor::new("\n"), &mut Vec::new()).unwrap();
        assert_eq!(target, Some(Target::named("Redmi")));
    }

    #[test]
    fn the_dialogue_rescans_until_a_phone_appears() {
        let mut scans = 0;
        let mut input = Cursor::new("\nr\n1\n");
        let target = ask(
            None,
            |_| {},
            || {
                scans += 1;
                Ok(if scans < 3 { vec![] } else { vec![phone("Pixel", -50), phone("Redmi", -60)] })
            },
            &mut input,
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(scans, 3);
        assert_eq!(target, Some(Target::named("Pixel")));
    }

    #[test]
    fn quitting_and_end_of_input_give_no_target() {
        let list = vec![phone("Pixel", -50)];
        assert_eq!(ask(None, |_| {}, || Ok(list.clone()), &mut Cursor::new("q\n"), &mut Vec::new()).unwrap(), None);
        assert_eq!(ask(None, |_| {}, || Ok(list.clone()), &mut Cursor::new(""), &mut Vec::new()).unwrap(), None);
    }

    #[test]
    fn a_scan_error_is_passed_on() {
        let result = ask(None, |_| {}, || Err("no adapter".to_string()), &mut Cursor::new(""), &mut Vec::new());
        assert_eq!(result, Err("no adapter".to_string()));
    }
}

//! Runtime commands (from a future Unix-socket CLI / Hyprland keybinds).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Toggle,
    Calibrate,
    ReloadConfig,
    Quit,
}

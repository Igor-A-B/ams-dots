//! User settings, read from a TOML file.
//!
//! Every key is optional: anything missing keeps its default, so a config file
//! only needs the values the user wants to change. Unknown keys are rejected
//! so typos do not silently do nothing. `config.example.toml` documents every
//! key.

use serde::Deserialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub dots: DotsConfig,
    pub motion: MotionConfig,
    pub spring: SpringConfig,
    pub display: DisplayConfig,
}

/// Look and population of the dots.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DotsConfig {
    /// Maximum dot radius in pixels.
    pub size: f32,
    /// Seconds from a dot's birth to its disappearance.
    pub lifetime: f32,
    /// Dots alive while the vehicle is at rest.
    pub default_amount: u32,
    /// Additional dots alive at full motion intensity.
    pub motion_extra_amount: u32,
    /// Percent of the lifetime spent growing.
    pub fade_in: f32,
    /// Percent of the lifetime spent shrinking away.
    pub fade_out: f32,
    /// Width of each side's spawn band, percent of the screen width.
    pub edge_band: f32,
}

/// How vehicle motion turns into dot behaviour.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MotionConfig {
    /// Acceleration (m/s²) that counts as full intensity.
    pub acceleration_full_scale: f32,
    /// Turning speed (rad/s) that counts as full intensity.
    pub rotation_full_scale: f32,
    /// Dot drift speed (px/s) per m/s² of smoothed acceleration.
    pub flow_gain: f32,
    /// Cap for the drift speed, px/s.
    pub max_speed: f32,
}

/// The spring that smooths the motion signal.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpringConfig {
    pub stiffness: f32,
    pub damping_ratio: f32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DisplayConfig {
    /// Redraws per second.
    pub fps: u32,
}

impl Default for DotsConfig {
    fn default() -> Self {
        Self {
            size: 7.0,
            lifetime: 2.5,
            default_amount: 24,
            motion_extra_amount: 40,
            fade_in: 20.0,
            fade_out: 40.0,
            edge_band: 8.0,
        }
    }
}

impl Default for MotionConfig {
    fn default() -> Self {
        Self { acceleration_full_scale: 4.0, rotation_full_scale: 0.5, flow_gain: 60.0, max_speed: 250.0 }
    }
}

impl Default for SpringConfig {
    fn default() -> Self {
        Self { stiffness: 60.0, damping_ratio: 0.6 }
    }
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self { fps: 30 }
    }
}

/// Where the config lives when `--config` is not given.
fn default_path() -> Option<String> {
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        return Some(format!("{dir}/ams-dots/config.toml"));
    }
    std::env::var("HOME").ok().map(|home| format!("{home}/.config/ams-dots/config.toml"))
}

/// Loads the settings. An explicit `path` must exist; the default location is
/// optional and falls back to the built-in defaults when absent.
pub fn load(path: Option<&str>) -> Result<Config, String> {
    let (path, required) = match path {
        Some(p) => (p.to_string(), true),
        None => match default_path() {
            Some(p) => (p, false),
            None => return Ok(Config::default()),
        },
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text).map_err(|e| format!("{path}: {e}")),
        Err(err) if !required && err.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(err) => Err(format!("cannot read {path}: {err}")),
    }
}

/// The settings of the running overlay. `ams-dots-linux reload` re-reads the file into it; the overlay notices
/// the new [`Settings::generation`] on its next frame and starts again with the new values.
pub struct Settings {
    /// `--config`, or `None` for the default location.
    path: Option<String>,
    config: Mutex<Config>,
    generation: AtomicU64,
}

impl Settings {
    /// Reads the settings for the first time; a broken file is an error here (the overlay does not start).
    pub fn load(path: Option<String>) -> Result<Arc<Self>, String> {
        let config = load(path.as_deref())?;
        Ok(Self::new(path, config))
    }

    fn new(path: Option<String>, config: Config) -> Arc<Self> {
        Arc::new(Self { path, config: Mutex::new(config), generation: AtomicU64::new(0) })
    }

    fn lock(&self) -> MutexGuard<'_, Config> {
        self.config.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn config(&self) -> Config {
        self.lock().clone()
    }

    /// Changes whenever a [`Settings::reload`] succeeds; cheap enough to check every frame.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// The settings and the generation they belong to.
    pub fn snapshot(&self) -> (Config, u64) {
        let config = self.lock();
        (config.clone(), self.generation())
    }

    /// Reads the file again. If it cannot be read or is invalid the running settings stay as they are.
    pub fn reload(&self) -> Result<(), String> {
        let config = load(self.path.as_deref())?;
        let mut current = self.lock();
        *current = config;
        self.generation.fetch_add(1, Ordering::Release);
        Ok(())
    }

    /// The file the settings come from, for messages.
    pub fn source(&self) -> String {
        self.path.clone().or_else(default_path).unwrap_or_else(|| "the built-in defaults".to_string())
    }
}

/// Parses and validates TOML text.
pub fn parse(text: &str) -> Result<Config, String> {
    let config: Config = toml::from_str(text).map_err(|e| e.to_string())?;
    validate(&config)?;
    Ok(config)
}

fn validate(c: &Config) -> Result<(), String> {
    let checks: [(bool, &str); 10] = [
        (c.dots.size > 0.0, "dots.size must be greater than 0"),
        (c.dots.lifetime > 0.0, "dots.lifetime must be greater than 0"),
        (c.dots.fade_in >= 0.0 && c.dots.fade_out >= 0.0, "dots.fade_in and dots.fade_out cannot be negative"),
        (c.dots.fade_in + c.dots.fade_out <= 100.0, "dots.fade_in + dots.fade_out cannot exceed 100"),
        (c.dots.edge_band > 0.0 && c.dots.edge_band <= 50.0, "dots.edge_band must be in (0, 50]"),
        (c.motion.acceleration_full_scale > 0.0, "motion.acceleration_full_scale must be greater than 0"),
        (c.motion.rotation_full_scale > 0.0, "motion.rotation_full_scale must be greater than 0"),
        (c.motion.max_speed >= 0.0 && c.motion.flow_gain >= 0.0, "motion.flow_gain and motion.max_speed cannot be negative"),
        (c.spring.stiffness > 0.0 && c.spring.damping_ratio > 0.0, "spring.stiffness and spring.damping_ratio must be greater than 0"),
        (c.display.fps >= 1, "display.fps must be at least 1"),
    ];
    match checks.iter().find(|(ok, _)| !ok) {
        Some((_, message)) => Err(message.to_string()),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_gives_defaults() {
        let c = parse("").unwrap();
        assert_eq!(c.dots.default_amount, 24);
        assert_eq!(c.display.fps, 30);
    }

    #[test]
    fn partial_file_keeps_other_defaults() {
        let c = parse("[dots]\nsize = 12.0\n").unwrap();
        assert_eq!(c.dots.size, 12.0);
        assert_eq!(c.dots.lifetime, 2.5);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(parse("[dots]\nsise = 12.0\n").is_err());
        assert!(parse("[nope]\n").is_err());
    }

    #[test]
    fn invalid_values_are_rejected_with_the_key_name() {
        let err = parse("[dots]\nlifetime = 0.0\n").unwrap_err();
        assert!(err.contains("dots.lifetime"));
        assert!(parse("[dots]\nfade_in = 70.0\nfade_out = 40.0\n").is_err());
        assert!(parse("[display]\nfps = 0\n").is_err());
    }

    /// A config file in the temp directory, removed when dropped.
    struct TempConfig(std::path::PathBuf);

    impl TempConfig {
        fn new(name: &str, text: &str) -> Self {
            let path = std::env::temp_dir().join(format!("ams-dots-test-{}-{name}.toml", std::process::id()));
            std::fs::write(&path, text).unwrap();
            Self(path)
        }

        fn path(&self) -> String {
            self.0.display().to_string()
        }
    }

    impl Drop for TempConfig {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn reload_applies_the_changed_file_and_bumps_the_generation() {
        let file = TempConfig::new("reload-ok", "[dots]\nsize = 12.0\n");
        let settings = Settings::load(Some(file.path())).unwrap();
        assert_eq!(settings.snapshot().0.dots.size, 12.0);
        let before = settings.generation();
        std::fs::write(&file.0, "[dots]\nsize = 20.0\n").unwrap();
        settings.reload().unwrap();
        assert_eq!(settings.config().dots.size, 20.0);
        assert!(settings.generation() > before);
    }

    #[test]
    fn a_broken_file_on_reload_changes_nothing() {
        let file = TempConfig::new("reload-broken", "[dots]\nsize = 12.0\n");
        let settings = Settings::load(Some(file.path())).unwrap();
        let before = settings.generation();
        std::fs::write(&file.0, "[dots]\nsize = -1.0\n").unwrap();
        let err = settings.reload().unwrap_err();
        assert!(err.contains("dots.size"), "{err}");
        assert_eq!(settings.config().dots.size, 12.0);
        assert_eq!(settings.generation(), before, "the overlay must not restart for a refused file");
    }

    #[test]
    fn a_missing_explicit_file_is_refused_at_load() {
        assert!(Settings::load(Some("/nonexistent/ams-dots.toml".to_string())).is_err());
    }

    #[test]
    fn the_example_file_matches_the_defaults() {
        let from_file = parse(include_str!("../config.example.toml")).unwrap();
        let defaults = Config::default();
        assert_eq!(from_file.dots.size, defaults.dots.size);
        assert_eq!(from_file.dots.default_amount, defaults.dots.default_amount);
        assert_eq!(from_file.motion.flow_gain, defaults.motion.flow_gain);
        assert_eq!(from_file.spring.damping_ratio, defaults.spring.damping_ratio);
        assert_eq!(from_file.display.fps, defaults.display.fps);
    }
}

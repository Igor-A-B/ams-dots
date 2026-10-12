//! User-tunable settings. Will later be loaded from TOML with hot reload.

#[derive(Debug, Clone)]
pub struct Config {
    pub dot_count: usize,
    pub dot_radius: f32,
    pub opacity: f32,
    /// How strongly acceleration displaces dots.
    pub gain: f32,
    pub spring_k: f32,
    pub damping: f32,
    pub fps: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self { dot_count: 12, dot_radius: 0.008, opacity: 0.5, gain: 0.02, spring_k: 30.0, damping: 6.0, fps: 60 }
    }
}

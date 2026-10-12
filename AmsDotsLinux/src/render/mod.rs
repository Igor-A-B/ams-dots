//! Stage 4 — Output. Anything that can show dots.

mod console;
mod layer_shell;

pub use console::ConsoleRenderer;
#[allow(unused_imports)]
pub use layer_shell::LayerShellOverlay;

use crate::types::Dot;

pub trait Renderer {
    fn present(&mut self, dots: &[Dot]);
    fn set_visible(&mut self, visible: bool);
}

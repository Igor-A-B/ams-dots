//! The real overlay: wlr-layer-shell surface per output, `overlay` layer, empty
//! input region (click-through), no keyboard focus. Not implemented yet.

use super::Renderer;
use crate::types::Dot;

pub struct LayerShellOverlay;

impl Renderer for LayerShellOverlay {
    fn present(&mut self, _dots: &[Dot]) {}
    fn set_visible(&mut self, _visible: bool) {}
}

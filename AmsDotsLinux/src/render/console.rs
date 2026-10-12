//! Debug renderer: prints the first dot's position. Lets the pipeline run without Wayland.

use super::Renderer;
use crate::types::Dot;

#[derive(Default)]
pub struct ConsoleRenderer {
    visible: bool,
    frame: u64,
}

impl Renderer for ConsoleRenderer {
    fn present(&mut self, dots: &[Dot]) {
        self.frame += 1;
        if self.visible && self.frame % 15 == 0 {
            if let Some(d) = dots.first() {
                println!("frame {:5}  dot0 = ({:.3}, {:.3})", self.frame, d.pos.x, d.pos.y);
            }
        }
    }

    fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }
}

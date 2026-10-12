//! Painting: turns a list of circles into pixels.
//!
//! This module knows nothing about Wayland or about where dots come from. It
//! paints into a `tiny_skia::Pixmap` (an in-memory RGBA image).

use tiny_skia::{Color, FillRule, Paint, PathBuilder, PixmapMut, Stroke, Transform};

/// Alpha of a fully opaque dot's white fill (0-255).
const FILL_ALPHA: f32 = 170.0;
/// Alpha of the thin dark outline that keeps dots visible on white backgrounds.
const OUTLINE_ALPHA: f32 = 90.0;

/// One circle, in pixels, with the origin at the top-left of the screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dot {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
    /// 0.0 (invisible) to 1.0 (full).
    pub opacity: f32,
}

/// Extra pixels around a dot that its outline and anti-aliasing may touch.
const EDGE_PAD: f32 = 2.0;

/// A pixel rectangle `(x0, y0, x1, y1)`; the end coordinates are exclusive.
pub type Rect = (i32, i32, i32, i32);

/// Where the visible dots are, as one rectangle for the dots on the left half
/// of a `width` x `height` screen and one for the right half, clamped to the
/// screen. Dots live in two separate bands, so two small rectangles are much
/// cheaper to clear and report than one that spans the whole screen.
pub fn bounds_by_side(dots: &[Dot], width: u32, height: u32) -> [Option<Rect>; 2] {
    let mut sides: [Option<Rect>; 2] = [None, None];
    for dot in dots.iter().filter(|d| d.radius > 0.0 && d.opacity > 0.0) {
        let reach = dot.radius + EDGE_PAD;
        let rect: Rect = (
            ((dot.x - reach).floor() as i32).max(0),
            ((dot.y - reach).floor() as i32).max(0),
            ((dot.x + reach).ceil() as i32).min(width as i32),
            ((dot.y + reach).ceil() as i32).min(height as i32),
        );
        if rect.2 <= rect.0 || rect.3 <= rect.1 {
            continue; // entirely off screen
        }
        let side = if dot.x < width as f32 / 2.0 { 0 } else { 1 };
        sides[side] = union(sides[side], Some(rect));
    }
    sides
}

/// Smallest rectangle containing both (either may be absent).
pub fn union(a: Option<Rect>, b: Option<Rect>) -> Option<Rect> {
    match (a, b) {
        (Some(a), Some(b)) => Some((a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))),
        (a, None) => a,
        (None, b) => b,
    }
}

/// Sets the pixels of `rect` to transparent in a `width`-pixel-wide ARGB
/// canvas. `rect` must lie inside the canvas (see `bounds_by_side`).
pub fn clear(canvas: &mut [u8], width: u32, rect: Rect) {
    let stride = width as usize * 4;
    for y in rect.1..rect.3 {
        let row = y as usize * stride;
        canvas[row + rect.0 as usize * 4..row + rect.2 as usize * 4].fill(0);
    }
}

/// Paints `dots` onto an already transparent `pixmap`: translucent white discs
/// with a thin dark outline. `pixmap` can wrap shared-memory bytes directly, so
/// no intermediate image is needed. Erasing the previous frame is the caller's
/// job (see `clear`), which lets it clear only the parts that had dots.
pub fn draw(pixmap: &mut PixmapMut, dots: &[Dot]) {
    let stroke = Stroke { width: 1.5, ..Stroke::default() };

    for dot in dots {
        let opacity = dot.opacity.clamp(0.0, 1.0);
        if dot.radius <= 0.0 || opacity <= 0.0 {
            continue;
        }
        let mut fill = Paint::default();
        fill.set_color(Color::from_rgba8(255, 255, 255, (FILL_ALPHA * opacity) as u8));
        fill.anti_alias = true;
        let mut outline = Paint::default();
        outline.set_color(Color::from_rgba8(0, 0, 0, (OUTLINE_ALPHA * opacity) as u8));
        outline.anti_alias = true;

        if let Some(circle) = PathBuilder::from_circle(dot.x, dot.y, dot.radius) {
            pixmap.fill_path(&circle, &fill, FillRule::Winding, Transform::identity(), None);
            pixmap.stroke_path(&circle, &outline, &stroke, Transform::identity(), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia::Pixmap;

    fn canvas() -> Pixmap {
        Pixmap::new(100, 100).unwrap()
    }

    fn paint(img: &mut Pixmap, dots: &[Dot]) {
        draw(&mut img.as_mut(), dots);
    }

    #[test]
    fn background_stays_transparent_and_dots_are_painted() {
        let mut img = canvas();
        paint(&mut img, &[Dot { x: 50.0, y: 50.0, radius: 10.0, opacity: 1.0 }]);
        assert_eq!(img.pixel(2, 2).unwrap().alpha(), 0);
        assert!(img.pixel(50, 50).unwrap().alpha() > 0);
    }

    #[test]
    fn clear_erases_only_the_given_rectangle() {
        let mut bytes = vec![255u8; 10 * 10 * 4];
        clear(&mut bytes, 10, (2, 3, 5, 6));
        let alpha = |x: usize, y: usize| bytes[(y * 10 + x) * 4 + 3];
        assert_eq!(alpha(2, 3), 0);
        assert_eq!(alpha(4, 5), 0);
        assert_eq!(alpha(5, 5), 255, "end is exclusive");
        assert_eq!(alpha(4, 6), 255, "end is exclusive");
        assert_eq!(alpha(1, 3), 255);
    }

    #[test]
    fn lower_opacity_is_more_transparent() {
        let (mut full, mut faint) = (canvas(), canvas());
        paint(&mut full, &[Dot { x: 50.0, y: 50.0, radius: 10.0, opacity: 1.0 }]);
        paint(&mut faint, &[Dot { x: 50.0, y: 50.0, radius: 10.0, opacity: 0.3 }]);
        assert!(faint.pixel(50, 50).unwrap().alpha() < full.pixel(50, 50).unwrap().alpha());
    }

    #[test]
    fn bounds_are_reported_per_side_and_ignore_invisible_dots() {
        let dots = [
            Dot { x: 10.0, y: 20.0, radius: 5.0, opacity: 1.0 },
            Dot { x: 20.0, y: 40.0, radius: 5.0, opacity: 1.0 },
            Dot { x: 90.0, y: 60.0, radius: 5.0, opacity: 1.0 },
            Dot { x: 50.0, y: 50.0, radius: 5.0, opacity: 0.0 }, // invisible: ignored
        ];
        let [left, right] = bounds_by_side(&dots, 100, 100);
        let left = left.unwrap();
        let right = right.unwrap();
        assert!(left.0 <= 3 && left.1 <= 13 && left.2 >= 25 && left.3 >= 47);
        assert!(left.2 < 50, "the left rectangle stays on the left half");
        assert!(right.0 > 50 && right.2 <= 100, "clamped to the screen");
        assert_eq!(bounds_by_side(&[], 100, 100), [None, None]);
    }

    #[test]
    fn union_covers_both_rectangles() {
        assert_eq!(union(Some((0, 0, 5, 5)), Some((3, 4, 9, 6))), Some((0, 0, 9, 6)));
        assert_eq!(union(None, Some((1, 2, 3, 4))), Some((1, 2, 3, 4)));
        assert_eq!(union(None, None), None);
    }

    #[test]
    fn painted_pixels_stay_inside_the_bounds() {
        let dots = [Dot { x: 30.3, y: 40.7, radius: 6.0, opacity: 1.0 }];
        let mut img = canvas();
        paint(&mut img, &dots);
        let (x0, y0, x1, y1) = bounds_by_side(&dots, 100, 100)[0].unwrap();
        for y in 0..100 {
            for x in 0..100 {
                if img.pixel(x, y).unwrap().alpha() > 0 {
                    assert!((x as i32) >= x0 && (x as i32) < x1 && (y as i32) >= y0 && (y as i32) < y1);
                }
            }
        }
    }

    #[test]
    fn zero_size_or_invisible_dots_are_skipped() {
        let mut img = canvas();
        paint(&mut img, &[Dot { x: 50.0, y: 50.0, radius: 0.0, opacity: 1.0 }, Dot { x: 20.0, y: 20.0, radius: 5.0, opacity: 0.0 }]);
        assert!(img.pixels().iter().all(|p| p.alpha() == 0));
    }
}

//! The highlight over the place a pane held by its bar would land.

use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Size};

use super::{Canvas, Palette, physical};

/// How opaque the highlight's fill is: the pane under it stays readable.
const FILL_ALPHA: f32 = 0.25;
/// How thick the highlight's outline is, in logical pixels.
const OUTLINE: f64 = 2.0;

/// The highlight, `size` big: the accent laid thinly over the area, with a
/// solid accent outline.
pub fn pane_drop_highlight(
    size: Size<i32, Logical>,
    scale: f64,
    p: &Palette,
) -> MemoryRenderBuffer {
    highlight(size, scale, p).into_buffer(false)
}

fn highlight(size: Size<i32, Logical>, scale: f64, p: &Palette) -> Canvas {
    let mut c = Canvas::new(physical(size, scale));
    // Premultiplied, as the canvas keeps its pixels.
    let [r, g, b] = p.accent;
    let pre = |v: u8| (v as f32 * FILL_ALPHA).round() as u8;
    let fill = [pre(b), pre(g), pre(r), pre(0xff)];
    for pixel in c.px.as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&fill);
    }
    let edge = ((OUTLINE * scale).round() as i32).max(1);
    let (w, h) = (c.w, c.h);
    c.outline(0, 0, w, h, edge, p.accent);
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chrome::Theme;

    #[test]
    fn the_fill_is_see_through_and_the_outline_solid() {
        let c = highlight((20, 10).into(), 2.0, Theme::Dark.palette());
        let alpha = |x: i32, y: i32| c.px[((y * c.w + x) * 4 + 3) as usize];
        assert_eq!((c.w, c.h), (40, 20));
        // Outline: 2 logical pixels, 4 physical at scale 2.
        assert_eq!(alpha(0, 0), 0xff);
        assert_eq!(alpha(3, 10), 0xff);
        assert_eq!(alpha(39, 19), 0xff);
        assert_eq!(alpha(4, 10), 64);
        assert_eq!(alpha(20, 10), 64);
    }
}

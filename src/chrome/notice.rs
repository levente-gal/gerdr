//! The notice at the top of the app area.

use super::{
    Canvas, FONT_SIZE, NOTICE_HEIGHT, NOTICE_PADDING, Painter, Palette, SHADOW, band, phys,
    physical,
};
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Rectangle, Size};

impl Painter {
    /// How wide the notice showing `text` is, at most `max`.
    pub(super) fn notice_width(&self, text: &str, max: i32, scale: f64) -> i32 {
        let text_w = (self.text_width(text, FONT_SIZE, scale) / scale as f32).ceil() as i32;
        (text_w + 2 * NOTICE_PADDING).min(max).max(NOTICE_HEIGHT)
    }

    /// The notice: `text` on a pill `w` wide, inside a margin of `SHADOW`
    /// holding its drop shadow.
    pub(super) fn notice(&self, text: &str, w: i32, scale: f64, p: &Palette) -> MemoryRenderBuffer {
        let size = Size::from((w, NOTICE_HEIGHT));
        let outer = Size::from((w + 2 * SHADOW, NOTICE_HEIGHT + 2 * SHADOW));
        let mut c = Canvas::new(physical(outer, scale));
        c.popup(size, NOTICE_HEIGHT as f32 / 2.0, scale, p);
        let rect = Rectangle::new(
            (SHADOW + NOTICE_PADDING, SHADOW).into(),
            ((w - 2 * NOTICE_PADDING).max(0), NOTICE_HEIGHT).into(),
        );
        let (x, right) = phys(rect, scale);
        // A pixel of slack for the rounding of the measured width.
        let right = right + scale.ceil() as f32;
        self.text(
            &mut c,
            text,
            FONT_SIZE,
            x,
            right,
            band(rect, scale),
            scale,
            p.text,
        );
        c.into_buffer(false)
    }
}

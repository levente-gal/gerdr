//! The tooltip over the thing under the pointer: a small popup below it
//! (above it near the bottom) with what it is and the key doing the same.

use super::{Canvas, Painter, Palette, Rect, SHADOW, SMALL_FONT_SIZE, band, physical, rect_px};
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Rectangle, Size};

/// A tooltip: this high, its text this far in from its ends, the keycap
/// this far after the text, this far from what it is about, and at least
/// this far in from the output's edges.
pub const TOOLTIP_HEIGHT: i32 = 28;
const TOOLTIP_PADDING: i32 = 10;
const TOOLTIP_KEY_GAP: i32 = 8;
const TOOLTIP_GAP: i32 = 6;
const TOOLTIP_MARGIN: i32 = 4;
const TOOLTIP_RADIUS: f32 = 6.0;

/// What the tooltip shows: its text, the key doing the same, and what it
/// is about (window coordinates), to sit below or above it.
#[derive(Clone, Debug, PartialEq)]
pub struct TooltipView {
    pub text: String,
    pub key: Option<String>,
    pub anchor: Rect,
}

/// Where a tooltip `w` wide about `anchor` goes on an output of `output`:
/// centred under the anchor, above it when there is no room below, and
/// kept on the output.
pub fn tooltip_layout(anchor: Rect, w: i32, output: Size<i32, Logical>) -> Rect {
    let below = anchor.loc.y + anchor.size.h + TOOLTIP_GAP;
    let y = if below + TOOLTIP_HEIGHT + TOOLTIP_MARGIN > output.h {
        (anchor.loc.y - TOOLTIP_GAP - TOOLTIP_HEIGHT).max(TOOLTIP_MARGIN)
    } else {
        below
    };
    let x = (anchor.loc.x + (anchor.size.w - w) / 2)
        .min(output.w - TOOLTIP_MARGIN - w)
        .max(TOOLTIP_MARGIN);
    Rectangle::new((x, y).into(), (w, TOOLTIP_HEIGHT).into())
}

impl Painter {
    /// How wide the tooltip showing `view` is, at most `max`.
    pub(super) fn tooltip_width(&self, view: &TooltipView, max: i32, scale: f64) -> i32 {
        let mut w = self.text_width(&view.text, SMALL_FONT_SIZE, scale);
        if let Some(key) = &view.key {
            w += TOOLTIP_KEY_GAP as f32 * scale as f32 + self.keycaps_width(key, scale);
        }
        let w = (w / scale as f32).ceil() as i32 + 2 * TOOLTIP_PADDING;
        w.min(max).max(TOOLTIP_HEIGHT)
    }

    /// The tooltip `w` wide, inside a margin of `SHADOW` holding its drop
    /// shadow: its text, then its key as a keycap.
    pub(super) fn tooltip(
        &self,
        view: &TooltipView,
        w: i32,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let size = Size::from((w, TOOLTIP_HEIGHT));
        let outer = Size::from((w + 2 * SHADOW, TOOLTIP_HEIGHT + 2 * SHADOW));
        let mut c = Canvas::new(physical(outer, scale));
        let s = scale as f32;
        c.popup(size, TOOLTIP_RADIUS, scale, p);
        let panel = Rectangle::new((SHADOW, SHADOW).into(), size);
        let (x, _, pw, _) = rect_px(panel, scale);
        let pad = TOOLTIP_PADDING as f32 * s;
        let tooltip_band = band(panel, scale);
        let mut right = x + pw - pad;
        if let Some(key) = &view.key {
            let key_w = self.keycaps_width(key, scale);
            let key_x = (right - key_w).max(x + pad);
            self.keycaps(&mut c, key, key_x, tooltip_band, scale, p);
            right = key_x - TOOLTIP_KEY_GAP as f32 * s;
        }
        self.text(
            &mut c,
            &view.text,
            SMALL_FONT_SIZE,
            x + pad,
            right,
            tooltip_band,
            scale,
            p.text,
        );
        c.into_buffer(false)
    }
}

#[cfg(test)]
mod tests {
    use super::{TOOLTIP_GAP, TOOLTIP_HEIGHT, TOOLTIP_MARGIN, tooltip_layout};
    use crate::chrome::Rect;

    #[test]
    fn the_tooltip_sits_under_its_anchor_and_stays_on_the_output() {
        let output = (800, 600).into();
        let anchor = Rect::new((100, 10).into(), (28, 28).into());
        let tip = tooltip_layout(anchor, 120, output);
        assert_eq!(tip.loc.y, 10 + 28 + TOOLTIP_GAP);
        assert_eq!(tip.loc.x + tip.size.w / 2, 100 + 14);
        assert_eq!(tip.size, (120, TOOLTIP_HEIGHT).into());
        // Near the bottom it goes above; near the edges it moves in.
        let low = Rect::new((790, 580).into(), (10, 20).into());
        let tip = tooltip_layout(low, 120, output);
        assert_eq!(tip.loc.y + TOOLTIP_HEIGHT + TOOLTIP_GAP, 580);
        assert_eq!(tip.loc.x + tip.size.w, 800 - TOOLTIP_MARGIN);
        let left = Rect::new((0, 0).into(), (10, 20).into());
        assert_eq!(tooltip_layout(left, 120, output).loc.x, TOOLTIP_MARGIN);
    }
}

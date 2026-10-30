//! The confirm dialog over everything: whether to do something that can't
//! be undone (see `crate::confirm`).

use super::{
    BUTTON_RADIUS, FONT_SIZE, HELP_RADIUS, LABEL_FONT_SIZE, Painter, Palette, QUIT_BODY,
    QUIT_BUTTON, QUIT_BUTTON_GAP, QUIT_LINE_GAP, QUIT_PADDING, QUIT_TITLE, Rect, SHADOW, band,
    focus_ring, mix, phys, rect_px,
};
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// The dialog's width.
const CONFIRM_WIDTH: i32 = 560;
/// The widths of Cancel and of the button doing it.
const BUTTONS: [i32; 2] = [84, 140];

/// What the confirm dialog shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfirmView {
    pub title: String,
    pub body: String,
    /// The label of the button doing it.
    pub go: &'static str,
    /// The selected button: 0 Cancel, 1 the one doing it.
    pub selected: usize,
    /// The button under the pointer.
    pub hover: Option<usize>,
}

/// Where the parts of the confirm dialog go, in output coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfirmLayout {
    pub panel: Rect,
    pub title: Rect,
    pub body: Rect,
    /// Cancel, then the one doing it.
    pub buttons: [Rect; 2],
}

/// Where the confirm dialog's parts go, centred on an output of `size`.
pub fn confirm_layout(size: Size<i32, Logical>) -> ConfirmLayout {
    let height =
        2 * QUIT_PADDING + QUIT_TITLE + QUIT_LINE_GAP + QUIT_BODY + 2 * QUIT_LINE_GAP + QUIT_BUTTON;
    let width = CONFIRM_WIDTH.min(size.w);
    let panel = Rectangle::new(
        (
            ((size.w - width) / 2).max(0),
            ((size.h - height) / 2).max(0),
        )
            .into(),
        (width, height).into(),
    );
    let (x, y) = (panel.loc.x + QUIT_PADDING, panel.loc.y + QUIT_PADDING);
    let inner = width - 2 * QUIT_PADDING;
    let title = Rectangle::new((x, y).into(), (inner, QUIT_TITLE).into());
    let body_y = y + QUIT_TITLE + QUIT_LINE_GAP;
    let body = Rectangle::new((x, body_y).into(), (inner, QUIT_BODY).into());
    // Right-aligned, in order.
    let total = BUTTONS.iter().sum::<i32>() + QUIT_BUTTON_GAP;
    let left = panel.loc.x + width - QUIT_PADDING - total;
    let button_y = panel.loc.y + height - QUIT_PADDING - QUIT_BUTTON;
    let buttons = std::array::from_fn(|i| {
        let before = BUTTONS[..i]
            .iter()
            .map(|w| w + QUIT_BUTTON_GAP)
            .sum::<i32>();
        Rectangle::new(
            (left + before, button_y).into(),
            (BUTTONS[i], QUIT_BUTTON).into(),
        )
    });
    ConfirmLayout {
        panel,
        title,
        body,
        buttons,
    }
}

impl Painter {
    /// The confirm dialog for an output of `size`, inside a margin of
    /// `SHADOW` holding its drop shadow: its title, what it does, and
    /// Cancel and the button doing it, in the error colour.
    pub(super) fn confirm_dialog(
        &self,
        view: &ConfirmView,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let layout = confirm_layout(size);
        let panel = layout.panel;
        let mut c = self.popup(panel.size, HELP_RADIUS, scale, p);
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        // Rects relative to the buffer, which starts `SHADOW` before the panel.
        let shift = |r: Rect| {
            Rectangle::new(
                r.loc + Point::from((SHADOW - panel.loc.x, SHADOW - panel.loc.y)),
                r.size,
            )
        };
        for (rect, text, font, color) in [
            (layout.title, &view.title, LABEL_FONT_SIZE, p.text),
            (layout.body, &view.body, FONT_SIZE, p.muted),
        ] {
            let rect = shift(rect);
            let (x, end) = phys(rect, scale);
            self.text(&mut c, text, font, x, end, band(rect, scale), scale, color);
        }

        let r = BUTTON_RADIUS * s;
        let b = px(1).max(1) as f32;
        for (i, rect) in layout.buttons.iter().enumerate() {
            let rect = shift(*rect);
            let (x, y, w, h) = rect_px(rect, scale);
            let hovered = view.hover == Some(i);
            if view.selected == i {
                focus_ring(&mut c, rect, BUTTON_RADIUS, p.elevated, scale, p.error);
            }
            let (label, color) = if i == 1 {
                let fill = if hovered {
                    mix(p.error, p.text, 0.12)
                } else {
                    p.error
                };
                c.rounded(x, y, w, h, r, fill);
                (view.go, p.on_accent)
            } else {
                let fill = if hovered { p.selected } else { p.element };
                c.rounded(x, y, w, h, r, p.border);
                c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, fill);
                ("Cancel", p.text)
            };
            self.centered(&mut c, label, FONT_SIZE, rect, scale, color);
        }
        c.into_buffer(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chrome::inside;

    #[test]
    fn the_buttons_sit_inside_the_panel_below_the_text() {
        let l = confirm_layout((1920, 1080).into());
        for part in l.buttons.iter().chain([&l.title, &l.body]) {
            assert!(inside(l.panel, *part));
        }
        assert!(l.buttons[0].loc.y >= l.body.loc.y + l.body.size.h);
        assert!(l.buttons[0].loc.x + l.buttons[0].size.w < l.buttons[1].loc.x);
    }
}

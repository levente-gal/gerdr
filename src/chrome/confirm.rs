//! The confirm dialog over everything: whether to do something that can't
//! be undone (see `crate::confirm`). Also the look every modal dialog's
//! buttons share (`DialogButton`), and the keys a dialog takes besides
//! them, said at the left of its buttons (`Painter::dialog_keys`).

use super::{
    BUTTON_RADIUS, Canvas, DIALOG_WIDTH, FONT_SIZE, LABEL_FONT_SIZE, POPUP_RADIUS, Painter,
    Palette, QUIT_BODY, QUIT_BUTTON, QUIT_BUTTON_GAP, QUIT_LINE_GAP, QUIT_PADDING, QUIT_TITLE,
    Rect, SHADOW, band, focus_ring, hint_spots, mix, phys, rect_px,
};
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// The widths of Cancel and of the button doing it.
const BUTTONS: [i32; 2] = [84, 140];

/// How a dialog's button looks: Cancel and the like plain, the action
/// that only adds in the accent, one that stops or deletes in the error
/// colour, one that can't be pressed now dimmed (still readable).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DialogButton {
    Plain,
    Primary,
    Destructive,
    Off,
}

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
    let width = DIALOG_WIDTH.min(size.w);
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
    /// A dialog's button labelled `label` in `rect` (buffer coordinates),
    /// looking as `look` says; `hovered` lightens it, `focused` rings it
    /// (in the error colour for a destructive one).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn dialog_button(
        &self,
        c: &mut Canvas,
        label: &str,
        rect: Rect,
        look: DialogButton,
        (hovered, focused): (bool, bool),
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        let (x, y, w, h) = rect_px(rect, scale);
        let (b, r) = ((scale.round() as f32).max(1.0), BUTTON_RADIUS * s);
        let hovered = hovered && look != DialogButton::Off;
        if focused {
            let ring = if look == DialogButton::Destructive {
                p.error
            } else {
                p.accent
            };
            focus_ring(c, rect, BUTTON_RADIUS, p.elevated, scale, ring);
        }
        let color = match look {
            DialogButton::Primary | DialogButton::Destructive => {
                let base = if look == DialogButton::Primary {
                    p.accent
                } else {
                    p.error
                };
                let fill = if hovered {
                    mix(base, p.text, 0.12)
                } else {
                    base
                };
                c.rounded(x, y, w, h, r, fill);
                p.on_accent
            }
            DialogButton::Plain | DialogButton::Off => {
                let fill = if hovered { p.selected } else { p.element };
                c.rounded(x, y, w, h, r, p.border);
                c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, fill);
                if look == DialogButton::Off {
                    p.muted
                } else {
                    p.text
                }
            }
        };
        self.centered(c, label, FONT_SIZE, rect, scale, color);
    }

    /// The keys a dialog takes besides its buttons (`hints`, keycaps then
    /// what each does) from the left of `row` (buffer coordinates), the
    /// band its buttons sit in, up to `end`: as many as fit.
    pub(super) fn dialog_keys(
        &self,
        c: &mut Canvas,
        hints: &[(&str, &str)],
        row: Rect,
        end: i32,
        scale: f64,
        p: &Palette,
    ) {
        let room = Rectangle::new(row.loc, ((end - row.loc.x).max(0), row.size.h).into());
        let mut spots = hint_spots(hints, room);
        // Centred by `hint_spots`; moved to the left edge, as the
        // dialog's text is.
        let shift = spots.first().map_or(0, |(_, spot)| spot.loc.x - row.loc.x);
        for (_, spot) in &mut spots {
            spot.loc.x -= shift;
        }
        self.hints(c, hints, &spots, None, scale, p);
    }

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
        let mut c = self.popup(panel.size, POPUP_RADIUS, scale, p);
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

        for (i, rect) in layout.buttons.iter().enumerate() {
            let (label, look) = if i == 1 {
                (view.go, DialogButton::Destructive)
            } else {
                ("Cancel", DialogButton::Plain)
            };
            let state = (view.hover == Some(i), view.selected == i);
            self.dialog_button(&mut c, label, shift(*rect), look, state, scale, p);
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

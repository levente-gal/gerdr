//! The confirm dialog over everything: whether to do something that can't
//! be undone (see `crate::confirm`). Also where every modal dialog's
//! buttons go (`dialog_buttons`, painted by `Painter::button`), and the
//! keys a dialog takes besides them, said at the left of its buttons
//! (`Painter::dialog_keys`).

use super::{
    ButtonStyle, Canvas, DIALOG_WIDTH, FONT_SIZE, LABEL_FONT_SIZE, POPUP_RADIUS, Painter, Palette,
    QUIT_BODY, QUIT_BUTTON, QUIT_BUTTON_GAP, QUIT_LINE_GAP, QUIT_PADDING, QUIT_TITLE, Rect, SHADOW,
    band, button_width, hint_spots, phys,
};
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// Where a modal dialog's buttons labelled `labels` go, in order: in the
/// band `QUIT_PADDING` above `panel`'s bottom, right-aligned, each as wide
/// as its label needs (`button_width`), `QUIT_BUTTON_GAP` apart.
pub(super) fn dialog_buttons(panel: Rect, labels: &[&str]) -> Vec<Rect> {
    let widths: Vec<i32> = labels.iter().map(|label| button_width(label)).collect();
    let total = widths.iter().sum::<i32>() + (widths.len() as i32 - 1).max(0) * QUIT_BUTTON_GAP;
    let y = panel.loc.y + panel.size.h - QUIT_PADDING - QUIT_BUTTON;
    let mut x = panel.loc.x + panel.size.w - QUIT_PADDING - total;
    widths
        .into_iter()
        .map(|w| {
            let rect = Rectangle::new((x, y).into(), (w, QUIT_BUTTON).into());
            x += w + QUIT_BUTTON_GAP;
            rect
        })
        .collect()
}

/// What the confirm dialog shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfirmView {
    pub title: String,
    /// What it does: a line, or several split by `\n` (the Ship sheet's
    /// steps).
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

/// Where the confirm dialog's parts go, centred on an output of `size`,
/// its button doing it sized for a label as short as Cancel; the dialog
/// itself is laid out by `confirm_lines_layout`, for its label.
#[cfg(test)]
pub fn confirm_layout(size: Size<i32, Logical>) -> ConfirmLayout {
    confirm_dialog_layout(size, "")
}

/// Where the confirm dialog's parts go, centred on an output of `size`,
/// its button doing it labelled `go`, its body one line.
#[cfg(test)]
pub fn confirm_dialog_layout(size: Size<i32, Logical>, go: &str) -> ConfirmLayout {
    confirm_lines_layout(size, go, 1)
}

/// How many lines the confirm dialog's `body` takes.
pub fn confirm_body_lines(body: &str) -> usize {
    body.lines().count().max(1)
}

/// Where the confirm dialog's parts go, centred on an output of `size`,
/// its button doing it labelled `go`, its body `lines` lines high (one
/// for every dialog but the Ship sheet).
pub fn confirm_lines_layout(size: Size<i32, Logical>, go: &str, lines: usize) -> ConfirmLayout {
    let body_h = QUIT_BODY * lines.max(1) as i32;
    let height =
        2 * QUIT_PADDING + QUIT_TITLE + QUIT_LINE_GAP + body_h + 2 * QUIT_LINE_GAP + QUIT_BUTTON;
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
    let body = Rectangle::new((x, body_y).into(), (inner, body_h).into());
    let buttons = dialog_buttons(panel, &["Cancel", go]);
    ConfirmLayout {
        panel,
        title,
        body,
        buttons: [buttons[0], buttons[1]],
    }
}

impl Painter {
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
        let layout = confirm_lines_layout(size, view.go, confirm_body_lines(&view.body));
        let panel = layout.panel;
        let mut c = self.popup(panel.size, POPUP_RADIUS, scale, p);
        // Rects relative to the buffer, which starts `SHADOW` before the panel.
        let shift = |r: Rect| {
            Rectangle::new(
                r.loc + Point::from((SHADOW - panel.loc.x, SHADOW - panel.loc.y)),
                r.size,
            )
        };
        let title = shift(layout.title);
        let (x, end) = phys(title, scale);
        let title_band = band(title, scale);
        let text = &view.title;
        self.text(&mut c, text, LABEL_FONT_SIZE, x, end, title_band, scale, p.text);
        // The body, a line per `QUIT_BODY`.
        let body = shift(layout.body);
        for (i, line) in view.body.lines().enumerate() {
            let rect = Rectangle::new(
                (body.loc.x, body.loc.y + i as i32 * QUIT_BODY).into(),
                (body.size.w, QUIT_BODY).into(),
            );
            let (x, end) = phys(rect, scale);
            self.text(&mut c, line, FONT_SIZE, x, end, band(rect, scale), scale, p.muted);
        }

        for (i, rect) in layout.buttons.iter().enumerate() {
            let (label, style) = if i == 1 {
                (view.go, ButtonStyle::Destructive)
            } else {
                ("Cancel", ButtonStyle::Plain)
            };
            let state = (view.hover == Some(i), view.selected == i);
            self.button(&mut c, label, shift(*rect), state, style, scale, p);
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

    #[test]
    fn dialog_buttons_end_at_the_right_padding_each_wide_enough_for_its_label() {
        let panel = Rectangle::new((100, 50).into(), (DIALOG_WIDTH, 200).into());
        let labels = ["Cancel", "Open in new gerdr"];
        let rects = dialog_buttons(panel, &labels);
        assert_eq!(rects.len(), 2);
        let last = rects[1];
        assert_eq!(
            last.loc.x + last.size.w,
            panel.loc.x + panel.size.w - QUIT_PADDING
        );
        assert_eq!(
            last.loc.y + last.size.h,
            panel.loc.y + panel.size.h - QUIT_PADDING
        );
        assert_eq!(
            rects[0].loc.x + rects[0].size.w + QUIT_BUTTON_GAP,
            last.loc.x
        );
        for (rect, label) in rects.iter().zip(labels) {
            assert_eq!(rect.size.w, button_width(label));
        }
        assert!(dialog_buttons(panel, &[]).is_empty());
    }

    #[test]
    fn the_confirm_button_is_as_wide_as_its_label_needs() {
        let size = (1920, 1080).into();
        let long = confirm_dialog_layout(size, "Close pane and stop it");
        let short = confirm_dialog_layout(size, "Close");
        assert!(long.buttons[1].size.w > short.buttons[1].size.w);
        assert_eq!(confirm_layout(size).panel, long.panel);
    }

    #[test]
    fn a_body_of_several_lines_makes_the_dialog_taller() {
        let size = (1920, 1080).into();
        let one = confirm_dialog_layout(size, "Ship");
        assert_eq!(one, confirm_lines_layout(size, "Ship", 1));
        let three = confirm_lines_layout(size, "Ship", 3);
        assert_eq!(three.body.size.h, 3 * one.body.size.h);
        assert_eq!(three.panel.size.h - one.panel.size.h, 2 * QUIT_BODY);
        assert!(three.buttons[0].loc.y >= three.body.loc.y + three.body.size.h);
        assert_eq!(confirm_body_lines("a\nb\nc"), 3);
        assert_eq!(confirm_body_lines(""), 1);
    }
}

//! The restart dialog over everything: which of a workspace's commands
//! restarting it runs again (see `crate::restart`).

use super::{
    ButtonStyle, CHECKBOX_GAP, CHECKBOX_ROW, Check, DIALOG_WIDTH, FONT_SIZE, LABEL_FONT_SIZE,
    POPUP_RADIUS, Painter, Palette, QUIT_BODY, QUIT_BUTTON, QUIT_BUTTON_GAP, QUIT_LINE_GAP,
    QUIT_PADDING, QUIT_TITLE, ROW_HIGHLIGHT_RADIUS, Rect, SHADOW, band, dialog_buttons, mix, phys,
    rect_px,
};
use crate::restart::more_rows;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// The keys the dialog takes besides its buttons, said by them while all
/// rows show.
const KEYS: [(&str, &str); 1] = [("Space", "check")];
/// The gap between rows, and the room between the last and the buttons.
const ROW_GAP: i32 = 4;
const ROWS_END: i32 = 16;
/// How far a row's checkbox sits in from the row's left edge.
const ROW_INSET: i32 = 6;
/// The gap between a command and its folder.
const PLACE_GAP: i32 = 12;
/// The room kept above and below the dialog on a short output.
const MARGIN: i32 = 40;

/// What the restart dialog shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestartView {
    /// What it asks.
    pub title: String,
    /// What the checked ones do.
    pub body: &'static str,
    /// The label of the button going on.
    pub action: &'static str,
    pub rows: Vec<RestartRow>,
    /// The row under the keyboard's cursor.
    pub cursor: usize,
    /// The first row shown, when not all fit.
    pub top: usize,
    /// The selected button: 0 Cancel, 1 Restart.
    pub selected: usize,
    pub hover: Option<RestartHit>,
}

/// A command of the restart dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestartRow {
    pub label: String,
    /// Where it runs.
    pub place: String,
    pub exited: bool,
    pub checked: bool,
}

/// What the pointer is over in the restart dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestartHit {
    /// Index as in `RestartView::rows` (not counting from `top`).
    Row(usize),
    /// Index as in `RestartView::selected`.
    Button(usize),
}

/// Where the parts of the restart dialog go, in output coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestartLayout {
    pub panel: Rect,
    pub title: Rect,
    pub body: Rect,
    /// One per row that fits: the slots the rows from `RestartView::top`
    /// are shown in.
    pub rows: Vec<Rect>,
    /// Cancel, and the button going on.
    pub buttons: [Rect; 2],
}

/// Where the restart dialog's parts go, with `rows` commands and `action` as
/// the button going on, centred on an output of `size`: as many rows as fit.
pub fn restart_layout(size: Size<i32, Logical>, rows: usize, action: &str) -> RestartLayout {
    let fixed = 2 * QUIT_PADDING + QUIT_TITLE + 2 * QUIT_LINE_GAP + QUIT_BODY + ROWS_END - ROW_GAP
        + QUIT_BUTTON;
    let fit = ((size.h - 2 * MARGIN - fixed) / (CHECKBOX_ROW + ROW_GAP)).max(1) as usize;
    let shown = rows.min(fit);
    let height = fixed + shown as i32 * (CHECKBOX_ROW + ROW_GAP);
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
    let rows_y = body_y + QUIT_BODY + QUIT_LINE_GAP;
    let rows = (0..shown as i32)
        .map(|i| {
            let y = rows_y + i * (CHECKBOX_ROW + ROW_GAP);
            Rectangle::new((x, y).into(), (inner, CHECKBOX_ROW).into())
        })
        .collect();
    let buttons = dialog_buttons(panel, &["Cancel", action]);
    let buttons = [buttons[0], buttons[1]];
    RestartLayout {
        panel,
        title,
        body,
        rows,
        buttons,
    }
}

impl Painter {
    /// The restart dialog for an output of `size`, inside a margin of
    /// `SHADOW` holding its drop shadow: a checkbox row per command shown
    /// (from `top`), its place after it, how many more there are, and
    /// Cancel and the button going on.
    pub(super) fn restart_dialog(
        &self,
        view: &RestartView,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let layout = restart_layout(size, view.rows.len(), view.action);
        let panel = layout.panel;
        let mut c = self.popup(panel.size, POPUP_RADIUS, scale, p);
        let s = scale as f32;
        // Rects relative to the buffer, which starts `SHADOW` before the panel.
        let shift = |r: Rect| {
            Rectangle::new(
                r.loc + Point::from((SHADOW - panel.loc.x, SHADOW - panel.loc.y)),
                r.size,
            )
        };

        let title = shift(layout.title);
        let (title_x, title_end) = phys(title, scale);
        self.text(
            &mut c,
            &view.title,
            LABEL_FONT_SIZE,
            title_x,
            title_end,
            band(title, scale),
            scale,
            p.text,
        );
        let body = shift(layout.body);
        let (body_x, body_end) = phys(body, scale);
        self.text(
            &mut c,
            view.body,
            FONT_SIZE,
            body_x,
            body_end,
            band(body, scale),
            scale,
            p.muted,
        );

        // A row per command: lit under the keyboard's cursor or the
        // pointer, its checkbox, the command, then where it runs.
        let r = ROW_HIGHLIGHT_RADIUS * s;
        let shown = view.rows.iter().enumerate().skip(view.top);
        for (rect, (i, row)) in layout.rows.iter().zip(shown) {
            let rect = shift(*rect);
            let hovered = view.hover == Some(RestartHit::Row(i));
            let (x, y, w, h) = rect_px(rect, scale);
            if view.cursor == i {
                c.rounded(x, y, w, h, r, p.selected);
            } else if hovered {
                c.rounded(x, y, w, h, r, mix(p.elevated, p.selected, 0.5));
            }
            let check = Rectangle::new(
                (rect.loc.x + ROW_INSET, rect.loc.y).into(),
                (rect.size.w - ROW_INSET, rect.size.h).into(),
            );
            let state = if row.checked { Check::On } else { Check::Off };
            // Never lit itself: the row is, and a lit box would melt into
            // it.
            let box_end = self.checkbox(&mut c, check, state, false, scale, p);
            let end = phys(rect, scale).1 - (ROW_INSET as f32 * s).round();
            let row_band = band(rect, scale);
            let after = self.text(
                &mut c,
                &row.label,
                FONT_SIZE,
                box_end + (CHECKBOX_GAP as f32 * s).round(),
                end,
                row_band,
                scale,
                if row.checked { p.text } else { p.muted },
            );
            let place = match (row.exited, row.place.is_empty()) {
                (true, true) => "exited".to_owned(),
                (true, false) => format!("exited \u{b7} {}", row.place),
                (false, _) => row.place.clone(),
            };
            self.text(
                &mut c,
                &place,
                FONT_SIZE,
                after + (PLACE_GAP as f32 * s).round(),
                end,
                row_band,
                scale,
                p.placeholder,
            );
        }

        // At the left of the buttons, how many rows don't show, else the
        // keys besides the buttons; then the buttons, the one going on, the
        // default, in the accent.
        let first = shift(layout.buttons[0]);
        let left = shift(layout.body).loc.x;
        let end = first.loc.x - QUIT_BUTTON_GAP;
        match more_rows(view.top, layout.rows.len(), view.rows.len()) {
            Some(more) => {
                let spot = Rectangle::new(
                    (left, first.loc.y).into(),
                    (end - left, first.size.h).into(),
                );
                let (x, end) = phys(spot, scale);
                self.text(
                    &mut c,
                    &more,
                    FONT_SIZE,
                    x,
                    end,
                    band(spot, scale),
                    scale,
                    p.muted,
                );
            }
            None => {
                let row = Rectangle::new((left, first.loc.y).into(), (0, first.size.h).into());
                let enter = match view.selected {
                    0 => "Cancel",
                    _ => view.action,
                };
                self.dialog_keys(&mut c, &KEYS, Some(enter), row, end, scale, p);
            }
        }
        for (i, (rect, label)) in layout
            .buttons
            .iter()
            .zip(["Cancel", view.action])
            .enumerate()
        {
            let style = if i == 1 {
                ButtonStyle::Primary
            } else {
                ButtonStyle::Plain
            };
            let state = (
                view.hover == Some(RestartHit::Button(i)),
                view.selected == i,
            );
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
    fn rows_stack_inside_the_panel_above_the_buttons() {
        let size: Size<i32, Logical> = (1920, 1080).into();
        let l = restart_layout(size, 3, "Restart");
        assert_eq!(l.rows.len(), 3);
        assert_eq!(l.panel.loc.x, 1920 - (l.panel.loc.x + l.panel.size.w));
        for part in l.rows.iter().chain(&l.buttons).chain([&l.title, &l.body]) {
            assert!(inside(l.panel, *part));
        }
        assert!(
            l.rows
                .windows(2)
                .all(|r| r[0].loc.y + r[0].size.h < r[1].loc.y)
        );
        let last = l.rows[2];
        assert!(
            l.buttons
                .iter()
                .all(|b| b.loc.y >= last.loc.y + last.size.h)
        );
        assert!(l.buttons[0].loc.x + l.buttons[0].size.w < l.buttons[1].loc.x);
    }

    #[test]
    fn a_short_output_shows_the_rows_that_fit() {
        let l = restart_layout((800, 300).into(), 20, "Restart");
        assert!(!l.rows.is_empty() && l.rows.len() < 20);
        assert!(l.panel.size.h <= 300);
        assert_eq!(
            restart_layout((800, 2000).into(), 20, "Start").rows.len(),
            20
        );
    }
}

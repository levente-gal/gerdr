//! The quit dialog over everything; also asked when switching projects.

use super::{
    BUTTON_RADIUS, CHECKBOX_GAP, CHECKBOX_ROW, Check, FONT_SIZE, HELP_RADIUS, LABEL_FONT_SIZE,
    Painter, Palette, QUIT_BODY, QUIT_BUTTON, QUIT_BUTTON_GAP, QUIT_BUTTONS, QUIT_LINE_GAP,
    QUIT_PADDING, QUIT_TITLE, QUIT_WIDTH, Rect, SHADOW, SWITCH_BUTTONS, band, focus_ring, mix,
    phys, rect_px,
};
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// The gap between rows, and the room between the last and the buttons.
const ROW_GAP: i32 = 4;
const ROWS_END: i32 = 16;
/// How far a row's checkbox sits in from the row's left edge.
const ROW_INSET: i32 = 6;
/// The gap between a row's label and its detail.
const DETAIL_GAP: i32 = 12;

/// What the quit dialog shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuitView {
    /// The project's name.
    pub project: String,
    /// The checkboxes: what to stop, then "Don't ask again" when quitting.
    pub rows: Vec<QuitRow>,
    /// The row under the keyboard's cursor.
    pub cursor: usize,
    /// The selected button: 0 Cancel, 1 Quit; 0 Cancel, 1 Switch, 2 Open
    /// in new gerdr when switching.
    pub selected: usize,
    pub hover: Option<QuitHit>,
    /// The project to switch to, when switching.
    pub switch_to: Option<String>,
}

/// A checkbox of the quit dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuitRow {
    pub label: String,
    /// Said after the label, dimmed.
    pub detail: String,
    pub checked: bool,
}

/// What the pointer is over in the quit dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuitHit {
    /// Index as in `QuitView::rows`.
    Row(usize),
    /// Index as in `QuitView::selected`.
    Button(usize),
}

/// Where the parts of the quit dialog go, in output coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuitLayout {
    pub panel: Rect,
    pub title: Rect,
    pub body: Rect,
    /// One per row.
    pub rows: Vec<Rect>,
    /// Cancel, Quit; Cancel, Switch, Open in new gerdr when switching.
    pub buttons: Vec<Rect>,
}

/// The buttons' labels and widths.
fn buttons(switching: bool) -> &'static [(&'static str, i32)] {
    if switching {
        SWITCH_BUTTONS
    } else {
        QUIT_BUTTONS
    }
}

/// Where the quit dialog's parts go, with `rows` checkboxes, centred on an
/// output of `size`; asking about a switch to another project if
/// `switching`.
pub fn quit_layout(size: Size<i32, Logical>, rows: usize, switching: bool) -> QuitLayout {
    let height = 2 * QUIT_PADDING
        + QUIT_TITLE
        + 2 * QUIT_LINE_GAP
        + QUIT_BODY
        + rows as i32 * (CHECKBOX_ROW + ROW_GAP)
        - ROW_GAP
        + ROWS_END
        + QUIT_BUTTON;
    let width = QUIT_WIDTH.min(size.w);
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
    let rows = (0..rows as i32)
        .map(|i| {
            let y = rows_y + i * (CHECKBOX_ROW + ROW_GAP);
            Rectangle::new((x, y).into(), (inner, CHECKBOX_ROW).into())
        })
        .collect();
    let labels = buttons(switching);
    // Right-aligned, in order.
    let total =
        labels.iter().map(|(_, w)| w).sum::<i32>() + (labels.len() as i32 - 1) * QUIT_BUTTON_GAP;
    let left = panel.loc.x + width - QUIT_PADDING - total;
    let button_y = panel.loc.y + height - QUIT_PADDING - QUIT_BUTTON;
    let buttons = (0..labels.len())
        .map(|i| {
            let before = labels[..i]
                .iter()
                .map(|(_, w)| w + QUIT_BUTTON_GAP)
                .sum::<i32>();
            Rectangle::new(
                (left + before, button_y).into(),
                (labels[i].1, QUIT_BUTTON).into(),
            )
        })
        .collect();
    QuitLayout {
        panel,
        title,
        body,
        rows,
        buttons,
    }
}

impl Painter {
    /// The quit dialog for an output of `size`, inside a margin of `SHADOW`
    /// holding its drop shadow: the question, a checkbox row per thing to
    /// stop and "Don't ask again", and Cancel and Quit; or, switching,
    /// Cancel, Switch and Open in new gerdr.
    pub(super) fn quit_dialog(
        &self,
        view: &QuitView,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let switching = view.switch_to.is_some();
        let layout = quit_layout(size, view.rows.len(), switching);
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

        let title = shift(layout.title);
        let (title_x, title_end) = phys(title, scale);
        let title_text = match &view.switch_to {
            Some(to) => format!("Switch to \u{201c}{to}\u{201d}?"),
            None => "Quit gerdr?".into(),
        };
        self.text(
            &mut c,
            &title_text,
            LABEL_FONT_SIZE,
            title_x,
            title_end,
            band(title, scale),
            scale,
            p.text,
        );
        let body = shift(layout.body);
        let (body_x, body_end) = phys(body, scale);
        let body_text = if switching {
            format!(
                "Switch stops what is checked of \u{201c}{}\u{201d}.",
                view.project
            )
        } else {
            format!(
                "Stop what is checked of \u{201c}{}\u{201d}; the rest keeps running.",
                view.project
            )
        };
        self.text(
            &mut c,
            &body_text,
            FONT_SIZE,
            body_x,
            body_end,
            band(body, scale),
            scale,
            p.muted,
        );

        // A row per checkbox: lit under the keyboard's cursor or the
        // pointer, its box, the label, then its detail.
        let r = BUTTON_RADIUS * s;
        for (i, (rect, row)) in layout.rows.iter().zip(&view.rows).enumerate() {
            let rect = shift(*rect);
            let hovered = view.hover == Some(QuitHit::Row(i));
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
                p.text,
            );
            self.text(
                &mut c,
                &row.detail,
                FONT_SIZE,
                after + (DETAIL_GAP as f32 * s).round(),
                end,
                row_band,
                scale,
                p.placeholder,
            );
        }

        // The buttons; the last, the default, in the accent.
        let b = px(1).max(1) as f32;
        let labels = buttons(switching);
        let last = labels.len() - 1;
        for (i, (rect, (label, _))) in layout.buttons.iter().zip(labels).enumerate() {
            let rect = shift(*rect);
            let (x, y, w, h) = rect_px(rect, scale);
            let hovered = view.hover == Some(QuitHit::Button(i));
            if view.selected == i {
                focus_ring(&mut c, rect, BUTTON_RADIUS, p.elevated, scale, p.accent);
            }
            let color = if i == last {
                let fill = if hovered {
                    mix(p.accent, p.text, 0.12)
                } else {
                    p.accent
                };
                c.rounded(x, y, w, h, r, fill);
                p.on_accent
            } else {
                let fill = if hovered { p.selected } else { p.element };
                c.rounded(x, y, w, h, r, p.border);
                c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, fill);
                p.text
            };
            self.centered(&mut c, label, FONT_SIZE, rect, scale, color);
        }
        c.into_buffer(false)
    }
}

#[cfg(test)]
mod tests {
    use super::quit_layout;
    use crate::chrome::inside;
    use smithay::utils::{Logical, Size};

    #[test]
    fn switching_has_three_buttons() {
        let l = quit_layout((1920, 1080).into(), 2, true);
        assert_eq!((l.rows.len(), l.buttons.len()), (2, 3));
        assert!(l.buttons.iter().all(|b| inside(l.panel, *b)));
    }

    #[test]
    fn quit_layout_is_centred_with_rows_above_the_buttons() {
        let size: Size<i32, Logical> = (1920, 1080).into();
        let l = quit_layout(size, 3, false);
        let p = l.panel;
        assert_eq!(l.buttons.len(), 2);
        assert_eq!(p.loc.x, 1920 - (p.loc.x + p.size.w));
        assert_eq!(p.loc.y, 1080 - (p.loc.y + p.size.h));
        for part in l.rows.iter().chain([&l.title, &l.body]) {
            assert!(inside(p, *part));
        }
        assert!(l.buttons.iter().all(|b| inside(p, *b)));
        assert!(
            l.buttons
                .windows(2)
                .all(|b| b[0].loc.x + b[0].size.w < b[1].loc.x)
        );
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
    }
}

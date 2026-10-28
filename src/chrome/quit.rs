//! The quit dialog over everything; also asked when switching projects.

use super::{
    BUTTON_RADIUS, CHECKBOX_GAP, CHECKBOX_ROW, CHECKBOX_WIDTH, Check, FONT_SIZE, HELP_RADIUS,
    LABEL_FONT_SIZE, Painter, Palette, QUIT_BODY, QUIT_BUTTON, QUIT_BUTTON_GAP, QUIT_BUTTONS,
    QUIT_HEIGHT, QUIT_LINE_GAP, QUIT_PADDING, QUIT_TITLE, QUIT_WIDTH, Rect, SHADOW, SWITCH_BUTTONS,
    band, focus_ring, mix, phys, rect_px,
};
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// What the quit dialog shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuitView {
    /// The project's name.
    pub project: String,
    /// The selected button: 0 Cancel, 1 Stop project, 2 Keep running; 0
    /// Cancel, 1 Switch, 2 Open in new gerdr when switching.
    pub selected: usize,
    pub dont_ask: bool,
    pub hover: Option<QuitHit>,
    /// The project to switch to, when switching: no checkbox then.
    pub switch_to: Option<String>,
}

/// What the pointer is over in the quit dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuitHit {
    /// Index as in `QuitView::selected`.
    Button(usize),
    Checkbox,
}

/// Where the parts of the quit dialog go, in output coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuitLayout {
    pub panel: Rect,
    pub title: Rect,
    pub body: Rect,
    /// Empty when switching.
    pub checkbox: Rect,
    /// The line where the checkbox goes, which says more when switching.
    pub note: Rect,
    /// Cancel, Stop project, Keep running; Cancel, Switch, Open in new
    /// gerdr when switching.
    pub buttons: [Rect; 3],
}

/// The buttons' labels and widths.
fn buttons(switching: bool) -> [(&'static str, i32); 3] {
    if switching {
        SWITCH_BUTTONS
    } else {
        QUIT_BUTTONS
    }
}

/// Where the quit dialog's parts go, centred on an output of `size`;
/// asking about a switch to another project if `switching`.
pub fn quit_layout(size: Size<i32, Logical>, switching: bool) -> QuitLayout {
    let panel = Rectangle::new(
        (
            ((size.w - QUIT_WIDTH) / 2).max(0),
            ((size.h - QUIT_HEIGHT) / 2).max(0),
        )
            .into(),
        (QUIT_WIDTH, QUIT_HEIGHT).into(),
    );
    let (x, y) = (panel.loc.x + QUIT_PADDING, panel.loc.y + QUIT_PADDING);
    let inner = QUIT_WIDTH - 2 * QUIT_PADDING;
    let title = Rectangle::new((x, y).into(), (inner, QUIT_TITLE).into());
    let body_y = y + QUIT_TITLE + QUIT_LINE_GAP;
    let body = Rectangle::new((x, body_y).into(), (inner, QUIT_BODY).into());
    let note_y = body_y + QUIT_BODY + QUIT_LINE_GAP;
    let note = Rectangle::new((x, note_y).into(), (inner, CHECKBOX_ROW).into());
    let checkbox = if switching {
        Rect::default()
    } else {
        Rectangle::new((x, note_y).into(), (CHECKBOX_WIDTH, CHECKBOX_ROW).into())
    };
    let labels = buttons(switching);
    // Right-aligned, in order.
    let total =
        labels.iter().map(|(_, w)| w).sum::<i32>() + (labels.len() as i32 - 1) * QUIT_BUTTON_GAP;
    let left = panel.loc.x + QUIT_WIDTH - QUIT_PADDING - total;
    let button_y = panel.loc.y + QUIT_HEIGHT - QUIT_PADDING - QUIT_BUTTON;
    let buttons = std::array::from_fn(|i| {
        let before = labels[..i]
            .iter()
            .map(|(_, w)| w + QUIT_BUTTON_GAP)
            .sum::<i32>();
        Rectangle::new(
            (left + before, button_y).into(),
            (labels[i].1, QUIT_BUTTON).into(),
        )
    });
    QuitLayout {
        panel,
        title,
        body,
        checkbox,
        note,
        buttons,
    }
}

impl Painter {
    /// The quit dialog for an output of `size`, inside a margin of `SHADOW`
    /// holding its drop shadow: the question, the "Don't ask again"
    /// checkbox, and Cancel, Stop project and Keep running; or, switching,
    /// what the switch stops, and Cancel, Switch and Open in new gerdr.
    pub(super) fn quit_dialog(
        &self,
        view: &QuitView,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let switching = view.switch_to.is_some();
        let layout = quit_layout(size, switching);
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
                "This stops \u{201c}{}\u{201d}: its apps, terminals and agents.",
                view.project
            )
        } else {
            format!(
                "Keep \u{201c}{}\u{201d} running in the background?",
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

        if let Some(to) = &view.switch_to {
            let note = shift(layout.note);
            let (note_x, note_end) = phys(note, scale);
            self.text(
                &mut c,
                &format!("To keep it running, open \u{201c}{to}\u{201d} in a new gerdr."),
                FONT_SIZE,
                note_x,
                note_end,
                band(note, scale),
                scale,
                p.muted,
            );
        }

        // The checkbox: an outlined square, filled with a check mark when checked.
        let checkbox = shift(layout.checkbox);
        if !switching {
            let hovered = view.hover == Some(QuitHit::Checkbox);
            let state = if view.dont_ask { Check::On } else { Check::Off };
            let box_end = self.checkbox(&mut c, checkbox, state, hovered, scale, p);
            self.text(
                &mut c,
                "Don't ask again",
                FONT_SIZE,
                box_end + (CHECKBOX_GAP as f32 * s).round(),
                phys(checkbox, scale).1,
                band(checkbox, scale),
                scale,
                p.text,
            );
        }

        // The buttons; the last, the default, in the accent.
        let b = px(1).max(1) as f32;
        for (i, (rect, (label, _))) in layout.buttons.iter().zip(buttons(switching)).enumerate() {
            let rect = shift(*rect);
            let (x, y, w, h) = rect_px(rect, scale);
            let r = BUTTON_RADIUS * s;
            let hovered = view.hover == Some(QuitHit::Button(i));
            if view.selected == i {
                focus_ring(&mut c, rect, BUTTON_RADIUS, p.elevated, scale, p.accent);
            }
            let color = if i == 2 {
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
                if i == 1 { p.warning } else { p.text }
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
    fn switching_has_no_checkbox() {
        let l = quit_layout((1920, 1080).into(), true);
        assert!(l.checkbox.is_empty());
        assert!(inside(l.panel, l.note));
        assert!(l.buttons.iter().all(|b| inside(l.panel, *b)));
    }

    #[test]
    fn quit_layout_is_centred_with_buttons_in_order() {
        let size: Size<i32, Logical> = (1920, 1080).into();
        let l = quit_layout(size, false);
        let p = l.panel;
        assert_eq!(p.loc.x, 1920 - (p.loc.x + p.size.w));
        assert_eq!(p.loc.y, 1080 - (p.loc.y + p.size.h));
        for part in [l.title, l.body, l.checkbox] {
            assert!(inside(p, part));
        }
        assert!(l.buttons.iter().all(|b| inside(p, *b)));
        assert!(
            l.buttons
                .windows(2)
                .all(|b| b[0].loc.x + b[0].size.w < b[1].loc.x)
        );
        assert!(
            l.buttons
                .iter()
                .all(|b| b.loc.y >= l.checkbox.loc.y + l.checkbox.size.h)
        );
    }
}

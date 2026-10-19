//! The quit dialog over everything.

use super::{
    BUTTON_RADIUS, CHECKBOX_GAP, CHECKBOX_ROW, CHECKBOX_WIDTH, Canvas, Check, FONT_SIZE,
    HELP_RADIUS, LABEL_FONT_SIZE, Painter, Palette, QUIT_BODY, QUIT_BUTTON, QUIT_BUTTON_GAP,
    QUIT_BUTTONS, QUIT_HEIGHT, QUIT_LINE_GAP, QUIT_PADDING, QUIT_TITLE, QUIT_WIDTH, Rect, SHADOW,
    band, focus_ring, mix, phys, physical, rect_px,
};
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// What the quit dialog shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuitView {
    /// The project's name.
    pub project: String,
    /// The selected button: 0 Cancel, 1 Stop project, 2 Keep running.
    pub selected: usize,
    pub dont_ask: bool,
    pub hover: Option<QuitHit>,
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
    pub checkbox: Rect,
    /// Cancel, Stop project, Keep running.
    pub buttons: [Rect; 3],
}

/// Where the quit dialog's parts go, centred on an output of `size`.
pub fn quit_layout(size: Size<i32, Logical>) -> QuitLayout {
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
    let checkbox = Rectangle::new(
        (x, body_y + QUIT_BODY + QUIT_LINE_GAP).into(),
        (CHECKBOX_WIDTH, CHECKBOX_ROW).into(),
    );
    // Right-aligned, in order.
    let total = QUIT_BUTTONS.iter().map(|(_, w)| w).sum::<i32>()
        + (QUIT_BUTTONS.len() as i32 - 1) * QUIT_BUTTON_GAP;
    let left = panel.loc.x + QUIT_WIDTH - QUIT_PADDING - total;
    let button_y = panel.loc.y + QUIT_HEIGHT - QUIT_PADDING - QUIT_BUTTON;
    let buttons = std::array::from_fn(|i| {
        let before = QUIT_BUTTONS[..i]
            .iter()
            .map(|(_, w)| w + QUIT_BUTTON_GAP)
            .sum::<i32>();
        Rectangle::new(
            (left + before, button_y).into(),
            (QUIT_BUTTONS[i].1, QUIT_BUTTON).into(),
        )
    });
    QuitLayout {
        panel,
        title,
        body,
        checkbox,
        buttons,
    }
}

impl Painter {
    /// The quit dialog for an output of `size`, inside a margin of `SHADOW`
    /// holding its drop shadow: the question, the "Don't ask again"
    /// checkbox, and Cancel, Stop project and Keep running.
    pub(super) fn quit_dialog(
        &self,
        view: &QuitView,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let layout = quit_layout(size);
        let panel = layout.panel;
        let outer = Size::from((panel.size.w + 2 * SHADOW, panel.size.h + 2 * SHADOW));
        let mut c = Canvas::new(physical(outer, scale));
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        c.popup(panel.size, HELP_RADIUS, scale, p);
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
            "Quit gerdr?",
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
            &format!(
                "Keep \u{201c}{}\u{201d} running in the background?",
                view.project
            ),
            FONT_SIZE,
            body_x,
            body_end,
            band(body, scale),
            scale,
            p.muted,
        );

        // The checkbox: an outlined square, filled with a check mark when checked.
        let checkbox = shift(layout.checkbox);
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

        // The buttons; the last, the default, in the accent.
        let b = px(1).max(1) as f32;
        for (i, (rect, (label, _))) in layout.buttons.iter().zip(QUIT_BUTTONS).enumerate() {
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
    fn quit_layout_is_centred_with_buttons_in_order() {
        let size: Size<i32, Logical> = (1920, 1080).into();
        let l = quit_layout(size);
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

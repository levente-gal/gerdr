//! The bar over each terminal pane: what it shows, its buttons, and how
//! it is drawn.

use super::{
    BAR_BUTTON, BAR_BUTTON_GAP, BAR_INSET, BAR_NOTE_GAP, Canvas, DETAIL_FONT_SIZE, PADDING,
    PANE_BAR_HEIGHT, Painter, Palette, Rect, STATUS_GAP, STATUS_WIDTH, STROKE, icon_sdf, mix,
    physical, rect_px, restart_arrow, status_mark, stop,
};
use crate::herdr::AgentStatus;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle};

/// A pane's place split in two: its bar on top, and the rest for its
/// window, which keeps at least a pixel.
pub fn pane_bar_rects(pane: Rect) -> (Rect, Rect) {
    let h = PANE_BAR_HEIGHT.min(pane.size.h - 1).max(0);
    let bar = Rectangle::new(pane.loc, (pane.size.w, h).into());
    let client = Rectangle::new(
        (pane.loc.x, pane.loc.y + h).into(),
        (pane.size.w, (pane.size.h - h).max(1)).into(),
    );
    (bar, client)
}

/// Where the restart and stop buttons sit in a pane's bar, left to right.
pub fn pane_bar_buttons(bar: Rect) -> [(BarButton, Rect); 2] {
    let right = bar.loc.x + bar.size.w - BAR_INSET;
    let at = |i: i32| {
        Rectangle::new(
            (right - (i + 1) * BAR_BUTTON - i * BAR_BUTTON_GAP, bar.loc.y).into(),
            (BAR_BUTTON, bar.size.h).into(),
        )
    };
    [(BarButton::Restart, at(1)), (BarButton::Stop, at(0))]
}

/// What of a pane's bar is at `pos`, if the bar is.
pub fn pane_bar_hit(bar: Rect, pos: Point<f64, Logical>) -> Option<BarHit> {
    if !bar.to_f64().contains(pos) {
        return None;
    }
    let button = pane_bar_buttons(bar)
        .into_iter()
        .find(|(_, rect)| rect.to_f64().contains(pos));
    Some(button.map_or(BarHit::Label, |(button, _)| BarHit::Button(button)))
}

/// Everything the chrome shows; a change in any of it triggers a redraw.
/// A button in a terminal pane's bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarButton {
    Restart,
    Stop,
}

/// What a click on a pane's bar lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarHit {
    Label,
    Button(BarButton),
}

/// What a pane's bar says of its command, by the mark before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarStatus {
    /// The shell waits at its prompt, and nothing ran in it to run again.
    Idle,
    Running,
    /// The command ran and is done: it can run again.
    Exited,
    /// Being stopped or restarted.
    Working,
}

/// The bar over a terminal pane: what runs in it, and its buttons.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneBarView {
    /// Where the bar goes on the output.
    pub rect: Rect,
    pub label: String,
    /// A word after the label, such as "exited" or "stopping…".
    pub note: Option<&'static str>,
    pub status: BarStatus,
    pub can_restart: bool,
    pub can_stop: bool,
    /// The pane has keyboard focus.
    pub focused: bool,
    /// The button under the pointer.
    pub hover: Option<BarButton>,
}

impl Painter {
    /// A terminal pane's bar: a mark saying how its command is, the command
    /// and a note after it, then the restart and stop buttons.
    pub(super) fn pane_bar(
        &self,
        view: &PaneBarView,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let size = view.rect.size;
        let mut c = Canvas::new(physical(size, scale));
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let line = px(1).max(1);
        let background = if view.focused {
            p.tab_active
        } else {
            p.tab_bar
        };
        c.fill(0, 0, c.w, c.h, background);
        c.fill(0, c.h - line, c.w, line, p.border_variant);

        let bar = Rectangle::from_size(size);
        let buttons = pane_bar_buttons(bar);
        let mark = Rectangle::new((PADDING as i32, 0).into(), (STATUS_WIDTH, size.h).into());
        match view.status {
            BarStatus::Idle => {}
            BarStatus::Running => {
                icon_sdf(&mut c, mark, scale, p.success, |x, y| x.hypot(y) - 3.4 * s)
            }
            BarStatus::Exited => icon_sdf(&mut c, mark, scale, p.placeholder, |x, y| {
                (x.hypot(y) - 3.4 * s).abs() - STROKE * s
            }),
            BarStatus::Working => icon_sdf(&mut c, mark, scale, p.warning, |x, y| {
                status_mark(AgentStatus::Working, x, y, s)
            }),
        }

        // The note keeps its room; the label is cut short before it.
        let band = (0, px(size.h));
        let x = px(mark.loc.x + STATUS_WIDTH + STATUS_GAP) as f32;
        let right = px(buttons[0].1.loc.x - BAR_NOTE_GAP) as f32;
        let note_w = view.note.map_or(0.0, |note| {
            self.text_width(note, DETAIL_FONT_SIZE, scale) + px(BAR_NOTE_GAP) as f32
        });
        let (label_color, note_color) = match view.status {
            BarStatus::Running => (p.text, p.muted),
            BarStatus::Working => (p.text, p.warning),
            BarStatus::Idle | BarStatus::Exited => (p.muted, p.placeholder),
        };
        let end = self.text(
            &mut c,
            &view.label,
            DETAIL_FONT_SIZE,
            x,
            (right - note_w).max(x),
            band,
            scale,
            label_color,
        );
        if let Some(note) = view.note {
            self.text(
                &mut c,
                note,
                DETAIL_FONT_SIZE,
                end + px(BAR_NOTE_GAP) as f32,
                right,
                band,
                scale,
                note_color,
            );
        }

        for (button, rect) in buttons {
            let enabled = match button {
                BarButton::Restart => view.can_restart,
                BarButton::Stop => view.can_stop,
            };
            let hovered = enabled && view.hover == Some(button);
            if hovered {
                let inset = Rectangle::new(
                    (rect.loc.x + 2, rect.loc.y + 2).into(),
                    (rect.size.w - 4, (rect.size.h - 4).max(0)).into(),
                );
                let (x, y, w, h) = rect_px(inset, scale);
                c.rounded(x, y, w, h, 4.0 * s, p.selected);
            }
            let color = if !enabled {
                mix(p.placeholder, background, 0.5)
            } else if hovered {
                p.text
            } else {
                p.muted
            };
            match button {
                BarButton::Restart => {
                    icon_sdf(&mut c, rect, scale, color, |x, y| restart_arrow(x, y, s))
                }
                BarButton::Stop => icon_sdf(&mut c, rect, scale, color, |x, y| stop(x, y, s)),
            }
        }
        c.into_buffer(true)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BarButton, BarHit, BarStatus, PaneBarView, pane_bar_buttons, pane_bar_hit, pane_bar_rects,
    };
    use crate::chrome::{BAR_INSET, PANE_BAR_HEIGHT, Painter, Rect, Theme};

    #[test]
    fn pane_bar_takes_the_top_and_leaves_the_rest_to_the_window() {
        let pane = Rect::new((200, 50).into(), (600, 400).into());
        let (bar, client) = pane_bar_rects(pane);
        assert_eq!(
            bar,
            Rect::new((200, 50).into(), (600, PANE_BAR_HEIGHT).into())
        );
        assert_eq!(
            client,
            Rect::new(
                (200, 50 + PANE_BAR_HEIGHT).into(),
                (600, 400 - PANE_BAR_HEIGHT).into()
            )
        );
    }

    #[test]
    fn tiny_pane_keeps_a_window() {
        for h in [0, 1, 10, PANE_BAR_HEIGHT] {
            let pane = Rect::new((0, 0).into(), (300, h).into());
            let (bar, client) = pane_bar_rects(pane);
            assert!(client.size.h >= 1, "{h}");
            assert!(bar.size.h >= 0 && bar.size.h < h.max(1), "{h}");
        }
    }

    #[test]
    fn pane_bar_buttons_sit_right_and_do_not_overlap() {
        let bar = Rect::new((200, 50).into(), (600, PANE_BAR_HEIGHT).into());
        let [(first, restart), (second, stop)] = pane_bar_buttons(bar);
        assert_eq!((first, second), (BarButton::Restart, BarButton::Stop));
        assert_eq!(stop.loc.x + stop.size.w, 200 + 600 - BAR_INSET);
        assert!(restart.loc.x + restart.size.w <= stop.loc.x);
        assert!(restart.loc.y == 50 && stop.size.h == PANE_BAR_HEIGHT);
    }

    #[test]
    fn pane_bar_hit_agrees_with_the_buttons() {
        let bar = Rect::new((200, 50).into(), (600, PANE_BAR_HEIGHT).into());
        for (button, rect) in pane_bar_buttons(bar) {
            let centre = rect.to_f64().loc + rect.to_f64().size.downscale(2.0).to_point();
            assert_eq!(pane_bar_hit(bar, centre), Some(BarHit::Button(button)));
        }
        assert_eq!(pane_bar_hit(bar, (210.0, 60.0).into()), Some(BarHit::Label));
        assert_eq!(pane_bar_hit(bar, (210.0, 80.0).into()), None);
        assert_eq!(pane_bar_hit(bar, (100.0, 60.0).into()), None);
    }

    #[test]
    fn pane_bars_paint() {
        let painter = Painter::new();
        let statuses = [
            BarStatus::Idle,
            BarStatus::Running,
            BarStatus::Exited,
            BarStatus::Working,
        ];
        for theme in [Theme::Dark, Theme::Light] {
            for scale in [1.0, 1.25, 2.0] {
                for w in [600, 60, 1] {
                    for (i, status) in statuses.into_iter().enumerate() {
                        let view = PaneBarView {
                            rect: Rect::new((0, 0).into(), (w, PANE_BAR_HEIGHT).into()),
                            label: "npm run dev -- --port 3000 --host 0.0.0.0".into(),
                            note: (i % 2 == 0).then_some("restarting…"),
                            status,
                            can_restart: i < 2,
                            can_stop: i % 2 == 1,
                            focused: i == 0,
                            hover: Some(BarButton::Restart),
                        };
                        painter.pane_bar(&view, scale, theme.palette());
                    }
                }
            }
        }
    }
}

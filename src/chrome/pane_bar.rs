//! The bar over each terminal pane: what it shows, its buttons, how it
//! hides and shows, and how it is drawn; and the strip marking the focused
//! pane, whatever it shows.

use super::{
    BAR_BUTTON, BAR_BUTTON_GAP, BAR_INSET, BAR_NOTE_GAP, Canvas, DETAIL_FONT_SIZE, PADDING,
    PANE_BAR_HEIGHT, Painter, Palette, Rect, STATUS_GAP, STATUS_WIDTH, icon_sdf, mix, physical,
    rect_px, restart_arrow, status_mark, stop,
};
use crate::herdr::AgentStatus;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// How far from a pane's top edge, above or below it, the pointer brings
/// its hidden bar out.
pub const BAR_REVEAL: i32 = 8;
/// How far a bar slides from under the top edge of its pane as it shows,
/// and back as it hides: its whole height.
pub const BAR_SLIDE: i32 = PANE_BAR_HEIGHT;
/// The strip along the top edge of the focused pane: this high.
pub const FOCUS_STRIP: i32 = 2;

/// A pane's place split in two: its bar on top, and the rest for its
/// window, which keeps at least a pixel. The bar as it is when shown; see
/// `pane_bar_rects_shown` for one that may be hidden.
pub fn pane_bar_rects(pane: Rect) -> (Rect, Rect) {
    pane_bar_rects_shown(pane, true)
}

/// A pane's place split in two: its bar on top, and the rest for its
/// window. A hidden bar (`shown` false) has no height, and the window takes
/// its room; a shown one leaves the window at least a pixel.
pub fn pane_bar_rects_shown(pane: Rect, shown: bool) -> (Rect, Rect) {
    let h = if shown {
        PANE_BAR_HEIGHT.min(pane.size.h - 1).max(0)
    } else {
        0
    };
    let bar = Rectangle::new(pane.loc, (pane.size.w, h).into());
    let client = Rectangle::new(
        (pane.loc.x, pane.loc.y + h).into(),
        (pane.size.w, (pane.size.h - h).max(1)).into(),
    );
    (bar, client)
}

/// Whether the pointer at `pos` asks for the bar of the pane at `pane`:
/// within `BAR_REVEAL` of the pane's top edge, or, while the bar is
/// `shown`, anywhere over it.
pub fn pointer_reveals(pane: Rect, shown: bool, pos: Point<f64, Logical>) -> bool {
    let (x, y) = (pos.x, pos.y);
    let (left, right) = (pane.loc.x as f64, (pane.loc.x + pane.size.w) as f64);
    if x < left || x >= right {
        return false;
    }
    let top = pane.loc.y as f64;
    let reach = if shown {
        pane_bar_rects(pane).0.size.h.max(BAR_REVEAL)
    } else {
        BAR_REVEAL
    };
    y >= top - BAR_REVEAL as f64 && y < top + reach as f64
}

/// Where the strip marking the focused pane goes: along the top edge of
/// `pane` (the pane's client rect), never taller than the pane.
pub fn focus_strip_rect(pane: Rect) -> Rect {
    Rectangle::new(
        pane.loc,
        (pane.size.w, FOCUS_STRIP.min(pane.size.h).max(0)).into(),
    )
}

/// The strip marking the focused pane, `size` big: a line in the accent.
pub fn focus_strip(size: Size<i32, Logical>, scale: f64, p: &Palette) -> MemoryRenderBuffer {
    let mut c = Canvas::new(physical(size, scale));
    c.fill(0, 0, c.w, c.h, p.accent);
    c.into_buffer(false)
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

/// The bar over a terminal pane: what runs in it, its buttons, and where
/// it stands between hidden and shown.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneBarView {
    /// Where the bar goes on the output when shown in full; no height while
    /// it is hidden and still.
    pub rect: Rect,
    pub label: String,
    /// A word after the label, such as "exited" or "stopping…".
    pub note: Option<&'static str>,
    pub status: BarStatus,
    pub can_restart: bool,
    pub can_stop: bool,
    /// It has the restart and stop buttons: a terminal's bar.
    pub buttons: bool,
    /// The pane has keyboard focus.
    pub focused: bool,
    /// The pointer is over the bar: its buttons show.
    pub lit: bool,
    /// The button under the pointer.
    pub hover: Option<BarButton>,
    /// The bar is out: with room of its own over its window (see
    /// `pane_bar_rects_shown`) or drawn over the window's top. While
    /// false with a height, it is still fading out.
    pub visible: bool,
    /// Its opacity, 0 to 1, from the fade showing or hiding it; 1 at rest.
    pub alpha: f32,
    /// How far it is still under its pane's top edge, in logical pixels,
    /// while it slides in or out; 0 at rest.
    pub slide: i32,
}

impl PaneBarView {
    /// Where the bar is drawn right now, how big, and how opaque: slid
    /// under the top edge by `slide`, so only the rest shows.
    pub fn placement(&self) -> (Point<i32, Logical>, Size<i32, Logical>, f32) {
        let h = (self.rect.size.h - self.slide).max(0);
        (self.rect.loc, (self.rect.size.w, h).into(), self.alpha)
    }

    /// The same bar at full opacity: what its buffer looks like, for a
    /// cache key (the alpha is applied when it is composed).
    pub fn still(&self) -> Self {
        Self {
            alpha: 1.0,
            ..self.clone()
        }
    }
}

impl Painter {
    /// A terminal pane's bar: a mark saying how its command is, the command
    /// and a note after it, then, with the pointer over it, the restart and
    /// stop buttons. Drawn `view.slide` pixels up, cut off at the top.
    pub(super) fn pane_bar(
        &self,
        view: &PaneBarView,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let (_, size, _) = view.placement();
        let full = view.rect.size;
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

        // Everything is laid out in the full bar, then moved up by the slide.
        let up = -view.slide;
        let bar = Rectangle::new((0, up).into(), full);
        let buttons = pane_bar_buttons(bar);
        let mark = Rectangle::new((PADDING as i32, up).into(), (STATUS_WIDTH, full.h).into());
        // The marks the sidebar's agent chips use: a dot for alive, a ring
        // for waiting, an open ring for busy.
        match view.status {
            BarStatus::Idle => {}
            BarStatus::Running => icon_sdf(&mut c, mark, scale, p.success, |x, y| {
                status_mark(AgentStatus::Done, x, y, s)
            }),
            BarStatus::Exited => icon_sdf(&mut c, mark, scale, p.placeholder, |x, y| {
                status_mark(AgentStatus::Idle, x, y, s)
            }),
            BarStatus::Working => icon_sdf(&mut c, mark, scale, p.warning, |x, y| {
                status_mark(AgentStatus::Working, x, y, s)
            }),
        }

        // The note keeps its room; the label is cut short before it. With
        // the buttons hidden, both reach the bar's end.
        let band = (px(up), px(up + full.h));
        let x = px(mark.loc.x + STATUS_WIDTH + STATUS_GAP) as f32;
        let end_x = if view.lit && view.buttons {
            buttons[0].1.loc.x
        } else {
            full.w - BAR_INSET
        };
        let right = px(end_x - BAR_NOTE_GAP) as f32;
        let note_w = view.note.map_or(0.0, |note| {
            self.text_width(note, DETAIL_FONT_SIZE, scale) + px(BAR_NOTE_GAP) as f32
        });
        // An unfocused bar is all muted; a focused one says more.
        let (label_color, note_color) = match view.status {
            _ if !view.focused => (p.muted, p.placeholder),
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

        if !view.lit || !view.buttons {
            return c.into_buffer(true);
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
        BAR_REVEAL, BarButton, BarHit, BarStatus, FOCUS_STRIP, PaneBarView, focus_strip,
        focus_strip_rect, pane_bar_buttons, pane_bar_hit, pane_bar_rects, pane_bar_rects_shown,
        pointer_reveals,
    };
    use crate::chrome::{BAR_INSET, PANE_BAR_HEIGHT, Painter, Rect, Theme};

    fn view(w: i32) -> PaneBarView {
        PaneBarView {
            rect: Rect::new((0, 0).into(), (w, PANE_BAR_HEIGHT).into()),
            label: "npm run dev -- --port 3000 --host 0.0.0.0".into(),
            note: None,
            status: BarStatus::Running,
            can_restart: true,
            can_stop: true,
            buttons: true,
            focused: true,
            lit: true,
            hover: None,
            visible: true,
            alpha: 1.0,
            slide: 0,
        }
    }

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
        assert_eq!(pane_bar_rects_shown(pane, true), (bar, client));
    }

    #[test]
    fn a_hidden_bar_has_no_height_and_the_window_takes_it_all() {
        let pane = Rect::new((200, 50).into(), (600, 400).into());
        let (bar, client) = pane_bar_rects_shown(pane, false);
        assert_eq!(bar, Rect::new((200, 50).into(), (600, 0).into()));
        assert_eq!(client, pane);
        // Even a pane too small for a bar.
        let tiny = Rect::new((0, 0).into(), (300, 1).into());
        assert_eq!(pane_bar_rects_shown(tiny, false).1, tiny);
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
    fn the_pointer_near_the_top_edge_reveals_the_bar() {
        let pane = Rect::new((200, 50).into(), (600, 400).into());
        let at = |x: f64, y: f64| (x, y).into();
        // Hidden: a band around the edge, inside the pane's width.
        assert!(pointer_reveals(pane, false, at(300.0, 50.0)));
        assert!(pointer_reveals(
            pane,
            false,
            at(300.0, 50.0 - BAR_REVEAL as f64)
        ));
        assert!(pointer_reveals(pane, false, at(300.0, 57.9)));
        assert!(!pointer_reveals(pane, false, at(300.0, 58.0)));
        assert!(!pointer_reveals(pane, false, at(300.0, 41.0)));
        assert!(!pointer_reveals(pane, false, at(199.0, 50.0)));
        assert!(!pointer_reveals(pane, false, at(800.0, 50.0)));
        // Shown: the whole bar keeps it, and the band above still counts.
        assert!(pointer_reveals(pane, true, at(300.0, 73.0)));
        assert!(pointer_reveals(pane, true, at(300.0, 45.0)));
        assert!(!pointer_reveals(pane, true, at(300.0, 74.0)));
        // A pane too short for a bar still has its band.
        let short = Rect::new((0, 100).into(), (300, 4).into());
        assert!(pointer_reveals(short, true, at(10.0, 105.0)));
        assert!(!pointer_reveals(short, true, at(10.0, 108.0)));
    }

    #[test]
    fn the_focus_strip_lies_along_the_top_edge() {
        let pane = Rect::new((200, 74).into(), (600, 376).into());
        assert_eq!(
            focus_strip_rect(pane),
            Rect::new((200, 74).into(), (600, FOCUS_STRIP).into())
        );
        let flat = Rect::new((0, 0).into(), (100, 1).into());
        assert_eq!(focus_strip_rect(flat).size.h, 1);
        assert_eq!(focus_strip_rect(Rect::default()).size.h, 0);
        for scale in [1.0, 1.5, 2.0] {
            for theme in [Theme::Dark, Theme::Light] {
                focus_strip((600, FOCUS_STRIP).into(), scale, theme.palette());
            }
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
        // A hidden bar has nothing to hit.
        let hidden = Rect::new((200, 50).into(), (600, 0).into());
        assert_eq!(pane_bar_hit(hidden, (210.0, 50.0).into()), None);
    }

    #[test]
    fn a_sliding_bar_shows_only_what_is_past_the_edge() {
        let mut v = view(600);
        v.alpha = 0.5;
        v.slide = 10;
        let (loc, size, alpha) = v.placement();
        assert_eq!(loc, (0, 0).into());
        assert_eq!(size, (600, PANE_BAR_HEIGHT - 10).into());
        assert_eq!(alpha, 0.5);
        v.slide = PANE_BAR_HEIGHT + 5;
        assert_eq!(v.placement().1.h, 0);
        // The cache key forgets the alpha, not the slide.
        let still = v.still();
        assert_eq!((still.alpha, still.slide), (1.0, PANE_BAR_HEIGHT + 5));
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
                            note: (i % 2 == 0).then_some("restarting…"),
                            status,
                            can_restart: i < 2,
                            can_stop: i % 2 == 1,
                            focused: i == 0,
                            buttons: i != 2,
                            lit: i != 3,
                            hover: Some(BarButton::Restart),
                            slide: [0, 7, PANE_BAR_HEIGHT - 1, PANE_BAR_HEIGHT][i],
                            ..view(w)
                        };
                        let buffer = painter.pane_bar(&view, scale, theme.palette());
                        drop(buffer);
                    }
                }
            }
        }
    }
}

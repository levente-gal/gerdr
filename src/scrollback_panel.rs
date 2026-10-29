//! The scrollback viewer's share of gerdr (see `scrollback`): opening it
//! over a terminal pane with all herdr keeps of the pane's output, placing
//! it, and taking the keys and the pointer while it is open.

use std::time::Instant;

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point, Rectangle},
};

use crate::{
    Gerdr,
    chrome::{self, Rect, ScrollbackHit, ScrollbackRow, ScrollbackView, SearchPart},
    input::{MULTI_CLICK, MULTI_CLICK_SLOP, Overlay},
    scroll::Motion,
    scrollback::{Fit, Outcome, Scrollback},
    toast::{Level, ToastHit},
    tooltip::Tooltip,
};

/// A pane smaller than this either way has the viewer over the whole app
/// area instead, to have room to read in.
const MIN_WIDTH: i32 = 420;
const MIN_HEIGHT: i32 = 240;
/// How far the viewer stands in from its pane's edges, so its own show.
const INSET: i32 = 4;

/// The viewer's key hints, each a key it takes and what it does: the
/// search's while it is open, and while its field has the keyboard only
/// those.
fn hints(viewer: &Scrollback) -> Vec<(String, String)> {
    let hints: &[(&str, &str)] = match viewer.find_bar() {
        Some(bar) if bar.focused => &[
            ("Enter", "next"),
            ("Shift+Enter", "previous"),
            ("Tab", "to the text"),
            ("Esc", "close search"),
        ],
        Some(_) => &[
            ("n", "next"),
            ("Shift+N", "previous"),
            ("Ctrl+C", "copy"),
            ("/", "find"),
            ("Esc", "close search"),
        ],
        None => &[
            ("Ctrl+C", "copy"),
            ("Enter", "copy and close"),
            ("Ctrl+A", "select all"),
            ("/", "find"),
            ("Esc", "close"),
        ],
    };
    hints
        .iter()
        .map(|(key, what)| ((*key).to_owned(), (*what).to_owned()))
        .collect()
}

/// `rect` less `by` on every side.
fn inset(rect: Rect, by: i32) -> Rect {
    Rectangle::new(
        (rect.loc.x + by, rect.loc.y + by).into(),
        ((rect.size.w - 2 * by).max(1), (rect.size.h - 2 * by).max(1)).into(),
    )
}

impl Gerdr {
    /// Open the viewer over the focused pane, when it is a terminal.
    pub fn open_focused_scrollback(&mut self) {
        let focused = self.tab().and_then(|tab| {
            let id = tab.focused_id()?;
            tab.herdr_pane(id).map(|_| id)
        });
        match focused {
            Some(id) => self.open_scrollback(id),
            None => self.show_notice("Not a terminal pane".into()),
        }
    }

    /// Open the viewer over terminal pane `id` of the active tab, with all
    /// its pane keeps of its output, standing where the pane's view does.
    pub fn open_scrollback(&mut self, id: u32) {
        let Some(pane_id) = self
            .tab()
            .and_then(|tab| tab.herdr_pane(id))
            .map(|p| p.pane_id.clone())
        else {
            return self.show_notice("Not a terminal pane".into());
        };
        self.focus_pane(|_| Some(id));
        let read = match self.herdr.as_mut() {
            Some(herdr) => herdr.pane_scrollback(&pane_id),
            None => Err("no herdr session".into()),
        };
        let (text, from_bottom) = match read {
            Ok(read) => read,
            Err(err) => {
                tracing::warn!("scrollback of {pane_id}: {err}");
                return self.show_notice(format!("Couldn't read the pane's output: {err}"));
            }
        };
        let label = self.pane_bar_content(&pane_id).label;
        self.switcher = None;
        self.project_menu = None;
        self.worktrees.close();
        self.repos.close();
        self.inbox.close();
        self.stop_key_repeat();
        self.close_menu();
        self.tooltip.hide();
        self.chrome.drop_tooltip();
        let mut viewer = Scrollback::new(id, pane_id, label, &text, from_bottom);
        if let Some(fit) = self.scrollback_fit_for(id) {
            viewer.fit(fit);
        }
        self.scrollback = Some(viewer);
        // It takes the pointer from the terminal under it.
        self.refresh_pointer_focus();
    }

    /// Close the viewer.
    pub fn close_scrollback(&mut self) {
        self.scrollback = None;
        // Gone, the terminal under the pointer gets it back.
        self.refresh_pointer_focus();
    }

    /// Where the viewer over pane `id` goes: over the pane in the active
    /// tab, its bar too, or over the app area when the pane is too small
    /// to read in; `None` while the pane doesn't show.
    fn scrollback_panel_for(&self, id: u32) -> Option<Rect> {
        let area = self.content_rect();
        let (_, rect) = self
            .tab()?
            .shown_rects(area)
            .into_iter()
            .find(|(pane, _)| *pane == id)?;
        let room = if rect.size.w < MIN_WIDTH || rect.size.h < MIN_HEIGHT {
            area
        } else {
            rect
        };
        Some(inset(room, INSET))
    }

    fn scrollback_fit_for(&self, id: u32) -> Option<Fit> {
        self.scrollback_panel_for(id).map(chrome::scrollback_fit)
    }

    /// Where the open viewer goes, and how its text fits there.
    fn scrollback_place(&self) -> Option<(Rect, Fit)> {
        let panel = self.scrollback_panel_for(self.scrollback.as_ref()?.pane)?;
        Some((panel, chrome::scrollback_fit(panel)))
    }

    /// What the viewer shows, while it is open over a pane that shows.
    pub fn scrollback_view(&self) -> Option<ScrollbackView> {
        let viewer = self.scrollback.as_ref()?;
        let (panel, _) = self.scrollback_place()?;
        let first = viewer.first();
        let end = (first + chrome::scrollback_shown(panel)).min(viewer.row_count());
        let rows = (first..end)
            .map(|i| ScrollbackRow {
                text: viewer.row_text(i).to_owned(),
                selected: viewer.row_selection(i),
                found: viewer.row_found(i),
            })
            .collect();
        let note = match viewer.selected_lines() {
            Some(1) => "1 line selected".to_owned(),
            Some(n) => format!("{n} lines selected"),
            None if viewer.line_count() == 1 => "1 line".to_owned(),
            None => format!("{} lines", viewer.line_count()),
        };
        Some(ScrollbackView {
            panel,
            title: format!("Scrollback · {}", viewer.label),
            note,
            rows,
            first,
            part: viewer.part(),
            overscroll: viewer.overscroll(),
            total: viewer.row_count(),
            hints: hints(viewer),
            hover: viewer.hover,
            search: viewer.find_bar(),
            ticks: viewer.find_ticks(),
        })
    }

    /// A key while the viewer is open, `typed` with Shift and the layout
    /// applied.
    pub fn scrollback_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<Outcome> {
        let (_, fit) = self.scrollback_place()?;
        self.scrollback
            .as_mut()?
            .key(mods, sym, typed, Instant::now(), fit)
    }

    /// Do what the viewer asked for.
    pub fn apply_scrollback(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Close => self.close_scrollback(),
            Outcome::Copy { text, lines, close } => {
                self.copy_text(text);
                let s = if lines == 1 { "" } else { "s" };
                self.toast(format!("Copied {lines} line{s}"), Level::Info);
                if close {
                    self.close_scrollback();
                }
            }
            Outcome::NothingSelected => {
                self.show_notice("Nothing selected: drag over the text first".into());
            }
        }
    }

    /// A left press while the viewer is open: on the text it starts a
    /// selection (a word on a double click, the line on a triple one, and
    /// with Shift the selection reaches there), the keyboard going to the
    /// text from the search; on the search, what its part does; outside
    /// the viewer it closes it, but on a toast presses that.
    pub fn scrollback_click(&mut self, pos: Point<f64, Logical>) {
        let Some(view) = self.scrollback_view() else {
            return;
        };
        let layout = chrome::scrollback_layout(view.panel, &view.hints);
        let hit = chrome::scrollback_hit(&view, &layout, pos);
        if !matches!(hit, Some(ScrollbackHit::Search(_)))
            && let Some(viewer) = self.scrollback.as_mut()
        {
            viewer.blur_find();
        }
        match hit {
            Some(ScrollbackHit::Search(part)) => {
                let Some((_, fit)) = self.scrollback_place() else {
                    return;
                };
                if let Some(viewer) = self.scrollback.as_mut() {
                    viewer.find_click(part, fit);
                }
            }
            Some(ScrollbackHit::Text) => {
                let (row, col) = chrome::scrollback_point(&view, &layout, pos);
                let extend = self.seat.get_keyboard().unwrap().modifier_state().shift;
                let Some(viewer) = self.scrollback.as_mut() else {
                    return;
                };
                let now = Instant::now();
                let count = match viewer.clicks {
                    Some((when, from, count))
                        if now.duration_since(when) < MULTI_CLICK
                            && (from.x - pos.x).abs() < MULTI_CLICK_SLOP
                            && (from.y - pos.y).abs() < MULTI_CLICK_SLOP =>
                    {
                        count % 3 + 1
                    }
                    _ => 1,
                };
                viewer.clicks = Some((now, pos, count));
                viewer.press(row, col, extend, count);
            }
            // A key hint: its key, pressed.
            Some(ScrollbackHit::Hint(i)) => {
                self.press_hint(Overlay::Scrollback, &view.hints[i].0, pos);
            }
            Some(ScrollbackHit::Panel) => {}
            None => match self.toast_press(pos) {
                Some(ToastHit::Action(action)) => self.menu_action(action),
                Some(_) => {}
                None => self.close_scrollback(),
            },
        }
    }

    /// The pointer moved to `pos` holding a press on the text: the
    /// selection reaches the character there. Past the text's top or
    /// bottom, it reaches the row at that edge, which the list scrolls
    /// on under (see `scrollback_scroll_tick`).
    pub fn scrollback_drag(&mut self, pos: Point<f64, Logical>) {
        let Some(view) = self.scrollback_view() else {
            return;
        };
        let layout = chrome::scrollback_layout(view.panel, &view.hints);
        let text = layout.text;
        let (top, bottom) = (text.loc.y as f64, (text.loc.y + text.size.h) as f64 - 1.0);
        let at = Point::from((pos.x, pos.y.clamp(top, bottom)));
        let (row, col) = chrome::scrollback_point(&view, &layout, at);
        if let Some(viewer) = self.scrollback.as_mut() {
            viewer.drag_to(row, col);
        }
    }

    /// Whether a press on the viewer's text is held.
    pub fn scrollback_dragging(&self) -> bool {
        self.scrollback.as_ref().is_some_and(Scrollback::dragging)
    }

    /// The left button went up.
    pub fn scrollback_release(&mut self) {
        if let Some(viewer) = self.scrollback.as_mut() {
            viewer.release();
        }
    }

    /// The pointer moved while the viewer is open: its key hints and the
    /// search's buttons light up, the buttons with a tooltip.
    pub fn scrollback_hover(&mut self, pos: Point<f64, Logical>) -> Option<Tooltip> {
        let view = self.scrollback_view()?;
        let layout = chrome::scrollback_layout(view.panel, &view.hints);
        let hit = chrome::scrollback_hit(&view, &layout, pos);
        self.scrollback.as_mut()?.hover =
            hit.filter(|hit| matches!(hit, ScrollbackHit::Hint(_) | ScrollbackHit::Search(_)));
        let Some(ScrollbackHit::Search(part)) = hit else {
            return None;
        };
        let (parts, _) = chrome::scrollback_search(&view, &layout);
        let (_, rect) = parts.iter().find(|(p, _)| *p == part)?;
        let (text, key) = match part {
            SearchPart::Open => ("Find in the scrollback", "/"),
            SearchPart::Prev => ("The place found before", "Shift+Enter"),
            SearchPart::Next => ("The next place found", "Enter"),
            SearchPart::Close => ("Close the search", "Esc"),
            SearchPart::Field => return None,
        };
        Some(Tooltip::new(text, *rect).key(key))
    }

    /// Whether `pos` is on the viewer's text, where the pointer is a text
    /// cursor.
    pub fn on_scrollback_text(&self, pos: Point<f64, Logical>) -> bool {
        self.scrollback_view().is_some_and(|view| {
            let layout = chrome::scrollback_layout(view.panel, &view.hints);
            chrome::scrollback_hit(&view, &layout, pos) == Some(ScrollbackHit::Text)
        })
    }

    /// The wheel or the touchpad moved the viewer's text.
    pub fn scrollback_scroll(&mut self, motion: Motion) {
        let Some((_, fit)) = self.scrollback_place() else {
            return;
        };
        if let Some(viewer) = self.scrollback.as_mut() {
            viewer.scroll(motion, Instant::now(), fit);
        }
    }

    /// Stand the viewer's text at row `pos`, as its scrollbar's thumb held
    /// there puts it.
    pub fn scrollback_scroll_to(&mut self, pos: f64) {
        let Some((_, fit)) = self.scrollback_place() else {
            return;
        };
        if let Some(viewer) = self.scrollback.as_mut() {
            viewer.scroll_to(pos, fit);
        }
    }

    /// Lay the viewer out again for where its pane is now and move its
    /// text on to `now`: on its own, and while a drag is held past its top
    /// or bottom, the selection going on with it. Its pane gone from view,
    /// it closes. Whether its text still moves.
    pub fn scrollback_scroll_tick(&mut self, now: Instant) -> bool {
        if self.scrollback.is_none() {
            return false;
        }
        let Some((panel, fit)) = self.scrollback_place() else {
            self.close_scrollback();
            return false;
        };
        let edge = if self.scrollback_dragging() {
            chrome::scrollback_edge(&chrome::scrollback_layout(panel, &[]), self.pointer)
        } else {
            0.0
        };
        let Some(viewer) = self.scrollback.as_mut() else {
            return false;
        };
        viewer.fit(fit);
        let before = (viewer.first(), viewer.part());
        let moving = viewer.tick(now, fit, edge);
        // Moved at all, the last step of a glide too.
        if viewer.dragging() && (viewer.first(), viewer.part()) != before {
            self.scrollback_drag(self.pointer);
        }
        moving
    }
}

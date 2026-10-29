//! Scrollbars held by their thumbs: a left press on a scrollbar holds
//! its thumb (off the thumb, the thumb comes under the pointer first),
//! and the list stands wherever the thumb is dragged along the track.

use smithay::utils::{Logical, Point};

use crate::{
    Gerdr,
    chrome::{self, Thumb},
};

/// A scrollbar the pointer can hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bar {
    Sidebar,
    ProjectMenu,
    Switcher,
    Inbox,
    Scrollback,
    ReposTree,
    ReposDetail,
}

/// A scrollbar's thumb held with the left button: which, and how far
/// below the thumb's top.
#[derive(Clone, Copy, Debug)]
pub struct ThumbDrag {
    pub bar: Bar,
    pub grab: f64,
}

impl Gerdr {
    /// The scrollbar under `pos` that nothing covers, and its thumb: the
    /// popup on top's, as a press finds it.
    fn bar_at(&self, pos: Point<f64, Logical>) -> Option<(Bar, Thumb)> {
        if self.quit.is_some() || self.restart.is_some() || self.welcome.is_some() {
            return None;
        }
        let bars: &[Bar] = if self.project_menu.is_some() {
            &[Bar::ProjectMenu]
        } else if self.worktrees.is_open() || self.context_menu.is_some() {
            &[]
        } else if self.repos.is_open() {
            &[Bar::ReposTree, Bar::ReposDetail]
        } else if self.inbox.is_open() {
            &[Bar::Inbox]
        } else if self.scrollback.is_some() {
            &[Bar::Scrollback]
        } else if self.switcher.is_some() {
            &[Bar::Switcher]
        } else if self.help_hit(pos).is_some() {
            &[]
        } else {
            &[Bar::Sidebar]
        };
        bars.iter().find_map(|&bar| {
            let thumb = self.thumb(bar)?;
            // The sidebar's ends at its edge, which is held to resize it.
            let inside = bar != Bar::Sidebar || pos.x < self.sidebar_width as f64;
            (inside && thumb.takes(pos)).then_some((bar, thumb))
        })
    }

    /// The scrollbar `bar` as it stands, while its list does not all show.
    fn thumb(&self, bar: Bar) -> Option<Thumb> {
        let size = self.output_size();
        match bar {
            Bar::Sidebar => chrome::sidebar_thumb(
                &self.sidebar_layout(),
                self.sidebar_width,
                size,
                self.sidebar_scroll.pos(),
            ),
            Bar::ProjectMenu => {
                let view = self.project_menu_view()?;
                let layout = chrome::project_menu_layout(&view, size);
                chrome::project_menu_thumb(&view, &layout)
            }
            Bar::Switcher => {
                let view = self.switcher_view()?;
                let layout = chrome::switcher_geometry(&view, self.content_rect());
                chrome::switcher_thumb(&view, &layout)
            }
            Bar::Inbox => {
                let view = self.inbox_view()?;
                let layout = chrome::inbox_layout(&view, size);
                chrome::inbox_thumb(&view, &layout, size)
            }
            Bar::Scrollback => {
                let view = self.scrollback_view()?;
                let layout = chrome::scrollback_layout(view.panel, &view.hints);
                chrome::scrollback_thumb(&view, &layout)
            }
            Bar::ReposTree | Bar::ReposDetail => {
                let view = self.repos_view()?;
                let layout = chrome::repos_layout(&view, size);
                // The sheet and the branch menu are over them.
                if layout.sheet.is_some() || layout.branch_menu.is_some() {
                    return None;
                }
                let [tree, detail] = chrome::repos_thumbs(&view, &layout);
                if bar == Bar::ReposTree { tree } else { detail }
            }
        }
    }

    /// Whether the sidebar's scrollbar takes `pos`, before its edge does.
    pub(crate) fn on_sidebar_bar(&self, pos: Point<f64, Logical>) -> bool {
        self.bar_at(pos).is_some_and(|(bar, _)| bar == Bar::Sidebar)
    }

    /// A left press at `pos`: on a scrollbar, its thumb is held, by where
    /// it was pressed or else by its middle; whether it was.
    pub(crate) fn press_thumb(&mut self, pos: Point<f64, Logical>) -> bool {
        let Some((bar, thumb)) = self.bar_at(pos) else {
            return false;
        };
        let grab = thumb.on_thumb(pos.y).unwrap_or(thumb.h / 2.0);
        self.thumb_drag = Some(ThumbDrag { bar, grab });
        self.drag_thumb(pos);
        true
    }

    /// The pointer moved to `pos` with a thumb held: its list stands
    /// where the thumb is then.
    pub(crate) fn drag_thumb(&mut self, pos: Point<f64, Logical>) {
        let Some(drag) = self.thumb_drag else {
            return;
        };
        let Some(thumb) = self.thumb(drag.bar) else {
            // The list all shows now, or something covers it.
            self.thumb_drag = None;
            return;
        };
        let at = thumb.pos_at(pos.y - drag.grab);
        match drag.bar {
            Bar::Sidebar => self.sidebar_scroll_to(at),
            Bar::ProjectMenu => self.project_menu_scroll_to(at),
            Bar::Switcher => self.switcher_scroll_to(at),
            Bar::Inbox => self.inbox_scroll_to(at),
            Bar::Scrollback => self.scrollback_scroll_to(at),
            Bar::ReposTree => self.repos_scroll_to(at, false),
            Bar::ReposDetail => self.repos_scroll_to(at, true),
        }
    }
}

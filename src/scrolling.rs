//! The scrolled lists' share of input and of each frame: the wheel's and
//! the touchpad's events made `Motion`s, and every list moving on its own
//! (a glide, a fling, a bounce) moved on once a frame.

use std::time::Instant;

use smithay::backend::input::{Axis, Event, InputBackend, PointerAxisEvent};

use crate::{Gerdr, scroll::Motion};

/// How many logical pixels a list moves for each a touchpad's fingers move.
const FINGER_SPEED: f64 = 1.5;

impl Gerdr {
    /// What a wheel or touchpad event does to a list. The wheel turns
    /// whole notches; a touchpad (winit says only "pixels") moves by its
    /// pixels, physical ones made logical and sped up by `FINGER_SPEED`,
    /// and an event moving neither way is its fingers lifting. Sideways
    /// alone moves nothing.
    pub(crate) fn scroll_motion<I: InputBackend>(
        &self,
        event: &I::PointerAxisEvent,
    ) -> Option<Motion> {
        if let Some(v120) = event.amount_v120(Axis::Vertical) {
            return (v120 != 0.0).then_some(Motion::Wheel(v120 / 120.0));
        }
        let time = event.time_msec();
        let dy = event.amount(Axis::Vertical)?;
        let dx = event.amount(Axis::Horizontal).unwrap_or(0.0);
        if dy == 0.0 && dx == 0.0 {
            return Some(Motion::Lift { time });
        }
        (dy != 0.0).then(|| Motion::Finger {
            px: dy / self.scale() * FINGER_SPEED,
            time,
        })
    }

    /// Stop every list flung, where it stands; whether one was.
    pub(crate) fn stop_flings(&mut self) -> bool {
        // Every one of them, not just up to the first flung.
        [
            self.sidebar_scroll.stop_fling(),
            (self.project_menu.as_mut()).is_some_and(|menu| menu.stop_fling()),
            (self.switcher.as_mut()).is_some_and(|switcher| switcher.stop_fling()),
            self.worktrees.stop_fling(),
            self.repos.stop_fling(),
            (self.inbox.panel.as_mut()).is_some_and(|panel| panel.stop_fling()),
            (self.scrollback.as_mut()).is_some_and(|viewer| viewer.stop_fling()),
        ]
        .contains(&true)
    }

    /// Move every list moving on its own on to `now`, before the frame
    /// shows them. Once the last stops, what is under the pointer is found
    /// again: the lists leave it as it was while they move.
    pub(crate) fn tick_scrolls(&mut self, now: Instant) {
        // Every one of them, not just up to the first still moving.
        let moving = [
            self.project_menu_scroll_tick(now),
            self.switcher_scroll_tick(now),
            self.worktrees_scroll_tick(now),
            self.repos_scroll_tick(now),
            self.inbox_scroll_tick(now),
            self.scrollback_scroll_tick(now),
            self.sidebar_scroll_tick(now),
        ]
        .contains(&true);
        if self.scrolling && !moving {
            let time = self.start_time.elapsed().as_millis() as u32;
            self.pointer_moved(self.pointer, time);
        }
        self.scrolling = moving;
    }
}

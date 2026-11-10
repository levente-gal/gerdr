//! The scrolled lists' share of input and of each frame: the wheel's and
//! the touchpad's events made `Motion`s, and every list moving on its own
//! (a glide, a fling, a bounce) moved on once a frame. A list that moves
//! by whole rows only, as a dialog's, takes a `Motion` as rows (see
//! `whole_rows`).

use std::{cell::Cell, time::Instant};

use smithay::backend::input::{Axis, Event, InputBackend, PointerAxisEvent};

use crate::{
    Gerdr,
    scroll::{Motion, NOTCH_PX},
};

/// How many logical pixels a list moves for each a touchpad's fingers move.
const FINGER_SPEED: f64 = 1.5;

/// The whole steps (rows, characters) a press held `edge` logical pixels
/// past a list's or a field's start (negative) or end goes on by at `now`
/// (see `scrollback::edge_speed`), counted from the frame `held` says it
/// last went on at, with the part of a step it carried; none while it is
/// within (`edge` 0), which forgets them.
pub(crate) fn edge_steps(held: &mut Option<(Instant, f64)>, now: Instant, edge: f64) -> i64 {
    if edge == 0.0 {
        *held = None;
        return 0;
    }
    let steps = match *held {
        Some((at, carried)) => {
            carried + crate::scrollback::edge_speed(edge) * now.duration_since(at).as_secs_f64()
        }
        None => 0.0,
    };
    let whole = steps.trunc();
    *held = Some((now, steps - whole));
    whole as i64
}

thread_local! {
    /// What the motions so far moved a list of whole rows short of
    /// another row, in rows (see `whole_rows`).
    static ROWS_LEFT: Cell<f64> = const { Cell::new(0.0) };
}

/// How many whole rows `motion` moves a list moving by whole rows, its
/// rows `pitch` logical pixels apart, after the motions before it moved
/// it `left` rows short of another (down positive): a notch of the wheel
/// as far as it moves a list of pixels, at least a row, and the fingers
/// as far as they move. What it moves short of another row comes back
/// with it, to carry on to the next motion; a lift, or a turn the other
/// way, drops it.
pub(crate) fn whole_rows(motion: Motion, pitch: f64, left: f64) -> (isize, f64) {
    let pitch = pitch.max(1.0);
    let by = match motion {
        Motion::Wheel(notches) | Motion::Notches(notches) => {
            notches * (NOTCH_PX / pitch).round().max(1.0)
        }
        Motion::Finger { px, .. } => px / pitch,
        Motion::Lift { .. } => return (0, 0.0),
    };
    let left = if left * by < 0.0 { 0.0 } else { left };
    let total = left + by;
    let rows = total.trunc();
    (rows as isize, total - rows)
}

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

    /// How many whole rows `motion` moves a list moving by whole rows
    /// `pitch` logical pixels apart, carrying what it moves short of a row
    /// on to the next motion (see `whole_rows`).
    pub(crate) fn motion_rows(&self, motion: Motion, pitch: f64) -> isize {
        let (rows, left) = whole_rows(motion, pitch, ROWS_LEFT.get());
        ROWS_LEFT.set(left);
        rows
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
            self.feature_tab_stop_fling(),
            (self.review_tab_mut()).is_some_and(|tab| tab.wd.diff.scroll.stop_fling()),
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
            self.repos_drag_edge_tick(now),
            self.field_edge_tick(now),
            self.page_text_edge_tick(now),
            self.inbox_scroll_tick(now),
            self.scrollback_scroll_tick(now),
            self.feature_tab_scroll_tick(now),
            self.sidebar_scroll_tick(now),
        ]
        .contains(&true);
        self.keep_field_scroll();
        if self.scrolling && !moving {
            let time = self.start_time.elapsed().as_millis() as u32;
            self.pointer_moved(self.pointer, time);
        }
        self.scrolling = moving;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notch_moves_whole_rows_as_far_as_a_list_of_pixels_goes() {
        // Rows 50 px apart: a notch's 100 px is two rows.
        assert_eq!(whole_rows(Motion::Wheel(1.0), 50.0, 0.0), (2, 0.0));
        assert_eq!(whole_rows(Motion::Wheel(-1.0), 50.0, 0.0), (-2, 0.0));
        // Taller rows than a notch: a row at least.
        assert_eq!(whole_rows(Motion::Wheel(1.0), 300.0, 0.0), (1, 0.0));
        // A fine wheel's fraction of a notch carries on.
        let (rows, left) = whole_rows(Motion::Wheel(0.25), 50.0, 0.0);
        assert_eq!((rows, left), (0, 0.5));
        assert_eq!(whole_rows(Motion::Wheel(0.25), 50.0, left), (1, 0.0));
    }

    #[test]
    fn fingers_move_whole_rows_and_carry_the_rest() {
        let finger = |px| Motion::Finger { px, time: 0 };
        let (rows, left) = whole_rows(finger(30.0), 40.0, 0.0);
        assert_eq!(rows, 0);
        let (rows, left) = whole_rows(finger(30.0), 40.0, left);
        assert_eq!(rows, 1);
        assert!((left - 0.5).abs() < 1e-9);
        // Turned the other way, what was carried is dropped.
        assert_eq!(whole_rows(finger(-20.0), 40.0, left), (0, -0.5));
        // So does a lift.
        assert_eq!(whole_rows(Motion::Lift { time: 0 }, 40.0, left), (0, 0.0));
    }
}

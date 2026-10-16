//! Tooltips: what a region of the chrome says of the thing under the
//! pointer, after the pointer rests on it a while. A region's motion
//! handler offers one (or none); `TooltipState` times it and hides it
//! on a press or when the pointer leaves. A timer on the event loop
//! shows it once its wait is over.

use std::time::{Duration, Instant};

use smithay::reexports::calloop::timer::{TimeoutAction, Timer};

use crate::{
    Gerdr,
    chrome::{Rect, TooltipRow, TooltipView},
};

/// How long the pointer rests on something before its tooltip shows.
pub const SHOW_DELAY: Duration = Duration::from_millis(500);

/// What a tooltip says, and what it is about (window coordinates).
#[derive(Clone, Debug, PartialEq)]
pub struct Tooltip {
    pub text: String,
    /// The key that does the same, shown as a keycap.
    pub key: Option<String>,
    pub anchor: Rect,
    /// A list under the text, a line each.
    pub rows: Vec<TooltipRow>,
}

impl Tooltip {
    pub fn new(text: &str, anchor: Rect) -> Self {
        Self {
            text: text.into(),
            key: None,
            anchor,
            rows: Vec::new(),
        }
    }

    pub fn key(mut self, key: &str) -> Self {
        self.key = Some(key.into());
        self
    }

    pub fn rows(mut self, rows: Vec<TooltipRow>) -> Self {
        self.rows = rows;
        self
    }
}

/// The tooltip offered last and since when, and the one shown.
#[derive(Debug, Default)]
pub struct TooltipState {
    pending: Option<(Tooltip, Instant)>,
    shown: Option<Tooltip>,
    /// A timer on the event loop waits for `pending`.
    timed: bool,
}

impl TooltipState {
    /// The pointer is over what `tip` is about (nothing with a tooltip
    /// when `None`), at `now`: the same tooltip keeps its time, another
    /// starts over, none hides what shows.
    pub fn offer(&mut self, tip: Option<Tooltip>, now: Instant) {
        match tip {
            None => {
                self.pending = None;
                self.shown = None;
            }
            Some(tip) => {
                if self.shown.as_ref() == Some(&tip) {
                    return;
                }
                if self.pending.as_ref().is_none_or(|(p, _)| *p != tip) {
                    self.pending = Some((tip, now));
                    self.shown = None;
                }
            }
        }
    }

    /// Hide the tooltip, on a press or the pointer leaving.
    pub fn hide(&mut self) {
        self.pending = None;
        self.shown = None;
    }

    /// Whether a tooltip shows or is about to.
    pub fn is_up(&self) -> bool {
        self.pending.is_some() || self.shown.is_some()
    }

    /// Show the pending tooltip once it has waited `SHOW_DELAY`; whether
    /// anything changed.
    pub fn tick(&mut self, now: Instant) -> bool {
        match &self.pending {
            Some((tip, since)) if now.duration_since(*since) >= SHOW_DELAY => {
                self.shown = Some(tip.clone());
                self.pending = None;
                true
            }
            _ => false,
        }
    }

    /// When the pending tooltip is due, if one is.
    pub fn due(&self) -> Option<Instant> {
        self.pending.as_ref().map(|(_, since)| *since + SHOW_DELAY)
    }

    /// What the chrome draws, if a tooltip shows.
    pub fn view(&self) -> Option<TooltipView> {
        self.shown.as_ref().map(|tip| TooltipView {
            text: tip.text.clone(),
            key: tip.key.clone(),
            anchor: tip.anchor,
            rows: tip.rows.clone(),
        })
    }
}

impl Gerdr {
    /// Offer `tip` for what is under the pointer at `now` (see
    /// `TooltipState::offer`), and have a timer show it when it is due.
    pub fn offer_tooltip(&mut self, tip: Option<Tooltip>, now: Instant) {
        self.tooltip.offer(tip, now);
        let Some(due) = self.tooltip.due() else {
            return;
        };
        if self.tooltip.timed {
            return;
        }
        let show = move |_, _: &mut (), state: &mut Gerdr| {
            let now = Instant::now();
            state.tooltip.tick(now);
            match state.tooltip.due() {
                // Another tooltip took the wait over: wait for it.
                Some(due) if due > now => TimeoutAction::ToInstant(due),
                _ => {
                    state.tooltip.timed = false;
                    TimeoutAction::Drop
                }
            }
        };
        match self
            .loop_handle
            .insert_source(Timer::from_deadline(due), show)
        {
            Ok(_) => self.tooltip.timed = true,
            Err(err) => tracing::warn!("timing the tooltip: {err}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SHOW_DELAY, Tooltip, TooltipState};
    use crate::chrome::Rect;
    use std::time::{Duration, Instant};

    #[test]
    fn a_tooltip_shows_after_the_delay_and_hides_on_a_press() {
        let anchor = Rect::new((0, 0).into(), (10, 10).into());
        let tip = Tooltip::new("New tab", anchor).key("Ctrl+B C");
        let mut state = TooltipState::default();
        let start = Instant::now();
        state.offer(Some(tip.clone()), start);
        assert!(state.view().is_none());
        assert!(state.is_up());
        assert!(!state.tick(start + SHOW_DELAY / 2));
        assert!(state.tick(start + SHOW_DELAY));
        assert_eq!(state.view().map(|v| v.text), Some("New tab".into()));
        // Offered again while shown, it stays.
        state.offer(Some(tip), start + 2 * SHOW_DELAY);
        assert!(state.view().is_some());
        state.hide();
        assert!(state.view().is_none() && state.due().is_none() && !state.is_up());
        // Another tooltip starts its own wait.
        state.offer(Some(Tooltip::new("Close", anchor)), start);
        assert_eq!(state.due(), Some(start + SHOW_DELAY));
        state.offer(None, start);
        assert!(state.due().is_none());
    }

    #[test]
    fn moving_to_another_thing_starts_the_wait_over() {
        let anchor = Rect::new((0, 0).into(), (10, 10).into());
        let mut state = TooltipState::default();
        let start = Instant::now();
        state.offer(Some(Tooltip::new("Restart", anchor)), start);
        let later = start + Duration::from_millis(300);
        state.offer(Some(Tooltip::new("Stop", anchor)), later);
        assert_eq!(state.due(), Some(later + SHOW_DELAY));
        // The first one's time does not count for the second.
        assert!(!state.tick(start + SHOW_DELAY));
        assert!(state.tick(later + SHOW_DELAY));
        assert_eq!(state.view().map(|v| v.text), Some("Stop".into()));
        // Moving on while shown hides it until the next one is due.
        state.offer(Some(Tooltip::new("Close", anchor)), later + SHOW_DELAY);
        assert!(state.view().is_none());
        assert_eq!(state.due(), Some(later + 2 * SHOW_DELAY));
    }
}

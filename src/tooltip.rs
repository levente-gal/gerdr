//! Tooltips: what a region of the chrome says of the thing under the
//! pointer, after the pointer rests on it a while. A region's motion
//! handler offers one (or none); `TooltipState` times it and hides it
//! on a press or when the pointer leaves.

use std::time::{Duration, Instant};

use crate::chrome::{Rect, TooltipView};

/// How long the pointer rests on something before its tooltip shows.
pub const SHOW_DELAY: Duration = Duration::from_millis(500);

/// What a tooltip says, and what it is about (window coordinates).
#[derive(Clone, Debug, PartialEq)]
pub struct Tooltip {
    pub text: String,
    /// The key that does the same, shown as a keycap.
    pub key: Option<String>,
    pub anchor: Rect,
}

impl Tooltip {
    pub fn new(text: &str, anchor: Rect) -> Self {
        Self {
            text: text.into(),
            key: None,
            anchor,
        }
    }

    pub fn key(mut self, key: &str) -> Self {
        self.key = Some(key.into());
        self
    }
}

/// The tooltip offered last and since when, and the one shown.
#[derive(Debug, Default)]
pub struct TooltipState {
    pending: Option<(Tooltip, Instant)>,
    shown: Option<Tooltip>,
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
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{SHOW_DELAY, Tooltip, TooltipState};
    use crate::chrome::Rect;
    use std::time::Instant;

    #[test]
    fn a_tooltip_shows_after_the_delay_and_hides_on_a_press() {
        let anchor = Rect::new((0, 0).into(), (10, 10).into());
        let tip = Tooltip::new("New tab", anchor).key("Ctrl+B C");
        let mut state = TooltipState::default();
        let start = Instant::now();
        state.offer(Some(tip.clone()), start);
        assert!(state.view().is_none());
        assert!(!state.tick(start + SHOW_DELAY / 2));
        assert!(state.tick(start + SHOW_DELAY));
        assert_eq!(state.view().map(|v| v.text), Some("New tab".into()));
        // Offered again while shown, it stays.
        state.offer(Some(tip), start + 2 * SHOW_DELAY);
        assert!(state.view().is_some());
        state.hide();
        assert!(state.view().is_none() && state.due().is_none());
        // Another tooltip starts its own wait.
        state.offer(Some(Tooltip::new("Close", anchor)), start);
        assert_eq!(state.due(), Some(start + SHOW_DELAY));
        state.offer(None, start);
        assert!(state.due().is_none());
    }
}

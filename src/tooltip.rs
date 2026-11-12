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
/// How long it rests before the next one shows while a tooltip just
/// showed (moving along a row of buttons, say): hardly at all.
pub const WARM_DELAY: Duration = Duration::from_millis(60);
/// How long after a tooltip went the next one still shows that soon.
pub const WARM_TIME: Duration = Duration::from_millis(700);

/// What a tooltip says, and what it is about (window coordinates).
#[derive(Clone, Debug, PartialEq)]
pub struct Tooltip {
    pub text: String,
    /// The key that does the same, shown as a keycap.
    pub key: Option<String>,
    pub anchor: Rect,
    /// A list under the text, a line each.
    pub rows: Vec<TooltipRow>,
    /// Labelled values under the text, a line each.
    pub fields: Vec<(String, String)>,
}

impl Tooltip {
    pub fn new(text: &str, anchor: Rect) -> Self {
        Self {
            text: text.into(),
            key: None,
            anchor,
            rows: Vec::new(),
            fields: Vec::new(),
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

    pub fn fields(mut self, fields: Vec<(String, String)>) -> Self {
        self.fields = fields;
        self
    }
}

/// The tooltip offered last and when it is due, and the one shown.
#[derive(Debug, Default)]
pub struct TooltipState {
    pending: Option<(Tooltip, Instant)>,
    shown: Option<Tooltip>,
    /// Until when another tooltip shows after `WARM_DELAY`: one showed
    /// just now. The pointer leaving keeps it; a press or a key ends it.
    warm_until: Option<Instant>,
    /// A timer on the event loop waits for `pending`.
    timed: bool,
}

impl TooltipState {
    /// The pointer is over what `tip` is about (nothing with a tooltip
    /// when `None`), at `now`: the same tooltip keeps its time, another
    /// starts over, none hides what shows. Right after one showed, the
    /// next waits only `WARM_DELAY`.
    pub fn offer(&mut self, tip: Option<Tooltip>, now: Instant) {
        if self.shown.is_some() && self.shown != tip {
            self.warm_until = Some(now + WARM_TIME);
        }
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
                    let warm = self.warm_until.is_some_and(|until| now < until);
                    let wait = if warm { WARM_DELAY } else { SHOW_DELAY };
                    self.pending = Some((tip, now + wait));
                    self.shown = None;
                }
            }
        }
    }

    /// Hide the tooltip, on a press or a key: the next waits the whole
    /// `SHOW_DELAY` again.
    pub fn hide(&mut self) {
        self.pending = None;
        self.shown = None;
        self.warm_until = None;
    }

    /// `tip` in place of the one waiting or shown at the same place, as
    /// what it says was read meanwhile: it keeps its time.
    pub fn refresh(&mut self, tip: Tooltip) {
        if let Some((pending, _)) = self.pending.as_mut()
            && pending.anchor == tip.anchor
        {
            *pending = tip;
        } else if let Some(shown) = self.shown.as_mut()
            && shown.anchor == tip.anchor
        {
            *shown = tip;
        }
    }

    /// Whether a tooltip shows or is about to.
    pub fn is_up(&self) -> bool {
        self.pending.is_some() || self.shown.is_some()
    }

    /// Show the pending tooltip once it is due; whether anything changed.
    pub fn tick(&mut self, now: Instant) -> bool {
        match &self.pending {
            Some((tip, due)) if now >= *due => {
                self.shown = Some(tip.clone());
                self.pending = None;
                true
            }
            _ => false,
        }
    }

    /// When the pending tooltip is due, if one is.
    pub fn due(&self) -> Option<Instant> {
        self.pending.as_ref().map(|(_, due)| *due)
    }

    /// What the chrome draws, if a tooltip shows.
    pub fn view(&self) -> Option<TooltipView> {
        self.shown.as_ref().map(|tip| TooltipView {
            text: tip.text.clone(),
            key: tip.key.clone(),
            anchor: tip.anchor,
            rows: tip.rows.clone(),
            fields: tip.fields.clone(),
        })
    }
}

impl Gerdr {
    /// Offer `tip` for what is under the pointer at `now` (see
    /// `TooltipState::offer`), and have a timer show it when it is due.
    pub fn offer_tooltip(&mut self, tip: Option<Tooltip>, now: Instant) {
        self.hover.tooltip.offer(tip, now);
        let Some(due) = self.hover.tooltip.due() else {
            return;
        };
        if self.hover.tooltip.timed {
            return;
        }
        let show = move |_, _: &mut (), state: &mut Gerdr| {
            let now = Instant::now();
            state.hover.tooltip.tick(now);
            match state.hover.tooltip.due() {
                // Another tooltip took the wait over: wait for it.
                Some(due) if due > now => TimeoutAction::ToInstant(due),
                _ => {
                    state.hover.tooltip.timed = false;
                    TimeoutAction::Drop
                }
            }
        };
        match self
            .loop_handle
            .insert_source(Timer::from_deadline(due), show)
        {
            Ok(_) => self.hover.tooltip.timed = true,
            Err(err) => tracing::warn!("timing the tooltip: {err}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SHOW_DELAY, Tooltip, TooltipState, WARM_DELAY, WARM_TIME};
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
        // Moving on while shown hides it until the next one is due: soon,
        // as one just showed.
        let shown = later + SHOW_DELAY;
        state.offer(Some(Tooltip::new("Close", anchor)), shown);
        assert!(state.view().is_none());
        assert_eq!(state.due(), Some(shown + WARM_DELAY));
    }

    #[test]
    fn right_after_a_tooltip_the_next_shows_at_once() {
        let anchor = Rect::new((0, 0).into(), (10, 10).into());
        let mut state = TooltipState::default();
        let start = Instant::now();
        state.offer(Some(Tooltip::new("Restart", anchor)), start);
        assert!(state.tick(start + SHOW_DELAY));
        // Off it onto the gap between buttons, then the next button.
        let left = start + SHOW_DELAY + Duration::from_millis(100);
        state.offer(None, left);
        let next = left + Duration::from_millis(50);
        state.offer(Some(Tooltip::new("Stop", anchor)), next);
        assert_eq!(state.due(), Some(next + WARM_DELAY));
        assert!(state.tick(next + WARM_DELAY));
        // Long after, the whole wait again.
        state.offer(None, next + WARM_DELAY);
        let much_later = next + WARM_DELAY + WARM_TIME;
        state.offer(Some(Tooltip::new("Close", anchor)), much_later);
        assert_eq!(state.due(), Some(much_later + SHOW_DELAY));
        // A press ends the warm time at once.
        assert!(state.tick(much_later + SHOW_DELAY));
        state.hide();
        let after = much_later + SHOW_DELAY;
        state.offer(Some(Tooltip::new("Stop", anchor)), after);
        assert_eq!(state.due(), Some(after + SHOW_DELAY));
    }

    #[test]
    fn a_tooltip_read_meanwhile_keeps_its_time() {
        let now = Instant::now();
        let anchor = Rect::new((0, 0).into(), (10, 10).into());
        let mut state = TooltipState::default();
        state.offer(Some(Tooltip::new("2 commits", anchor)), now);
        let due = state.due();
        let more = Tooltip::new("2 commits", anchor).fields(vec![("a1".into(), "x".into())]);
        state.refresh(more.clone());
        assert_eq!(state.due(), due);
        assert!(state.tick(now + SHOW_DELAY));
        assert_eq!(state.view().unwrap().fields, more.fields);
        // Elsewhere, it is let be.
        let other = Rect::new((20, 0).into(), (10, 10).into());
        state.refresh(Tooltip::new("other", other));
        assert_eq!(state.view().unwrap().text, "2 commits");
    }
}

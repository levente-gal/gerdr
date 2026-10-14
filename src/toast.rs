//! Toasts: short notes over the apps ("Switched to gerdr", "Zed didn't
//! start"), each with a level and, some, a button. At most `MAX_TOASTS`
//! queue; each goes after `TOAST_TIME`, or when clicked. `show_notice`
//! is the old name for an `Info` toast.

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use smithay::{
    reexports::calloop::timer::{TimeoutAction, Timer},
    utils::{Logical, Point},
};

use crate::{
    Gerdr,
    chrome::{self, ToastView},
    menu::MenuAction,
};

/// How long a toast stays.
pub const TOAST_TIME: Duration = Duration::from_secs(4);
/// The most toasts kept; an older one goes when more come.
pub const MAX_TOASTS: usize = 3;

/// Tells the toasts apart, so each fades on its own.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// What kind of news a toast brings, for its colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub id: u64,
    pub text: String,
    pub level: Level,
    /// A button on the toast: its label and what it does.
    pub action: Option<(String, MenuAction)>,
    /// When the toast goes.
    pub until: Instant,
}

impl Toast {
    pub fn new(
        text: String,
        level: Level,
        action: Option<(String, MenuAction)>,
        until: Instant,
    ) -> Self {
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            text,
            level,
            action,
            until,
        }
    }

    pub fn view(&self) -> ToastView {
        ToastView {
            id: self.id,
            text: self.text.clone(),
            level: self.level,
            action: self.action.as_ref().map(|(label, _)| label.clone()),
        }
    }
}

/// Put `toast` in the queue, letting the oldest go past `MAX_TOASTS`.
pub fn enqueue(toasts: &mut std::collections::VecDeque<Toast>, toast: Toast) {
    toasts.push_back(toast);
    while toasts.len() > MAX_TOASTS {
        toasts.pop_front();
    }
}

/// What a press on the toasts does.
#[derive(Clone, Debug, PartialEq)]
pub enum ToastHit {
    /// The toast's button: what it does.
    Action(MenuAction),
    /// The toast itself: it goes.
    Dismiss,
}

impl Gerdr {
    /// Show `text` for `TOAST_TIME`.
    pub fn toast(&mut self, text: String, level: Level) {
        self.toast_with(text, level, None);
    }

    /// Show `text` for `TOAST_TIME`, with a button doing `action`.
    pub fn toast_with(&mut self, text: String, level: Level, action: Option<(String, MenuAction)>) {
        let until = Instant::now() + TOAST_TIME;
        enqueue(&mut self.toasts, Toast::new(text, level, action, until));
        // Gone from the next frame, as every frame draws the chrome.
        let clear = move |_, _: &mut (), state: &mut Gerdr| {
            state.expire_toasts(Instant::now());
            TimeoutAction::Drop
        };
        if let Err(err) = self
            .loop_handle
            .insert_source(Timer::from_duration(TOAST_TIME), clear)
        {
            // `chrome_view` leaves it out once it is past anyway.
            tracing::warn!("timing the toast out: {err}");
        }
    }

    /// Show `text` over the apps for `TOAST_TIME`.
    pub fn show_notice(&mut self, text: String) {
        self.toast(text, Level::Info);
    }

    /// Drop the toasts whose time is past at `now`.
    pub fn expire_toasts(&mut self, now: Instant) {
        self.toasts.retain(|toast| now < toast.until);
    }

    /// The toasts still showing, oldest first.
    pub fn toast_views(&self) -> Vec<ToastView> {
        let now = Instant::now();
        self.toasts
            .iter()
            .filter(|toast| now < toast.until)
            .map(Toast::view)
            .collect()
    }

    /// What a press at `pos` lands on among the toasts, if any: that toast
    /// goes, and its button's action comes back to run.
    pub fn toast_press(&mut self, pos: Point<f64, Logical>) -> Option<ToastHit> {
        self.expire_toasts(Instant::now());
        let views: Vec<ToastView> = self.toasts.iter().map(Toast::view).collect();
        let layouts = chrome::toast_layout(&views, self.content_rect());
        let i = layouts
            .iter()
            .position(|layout| layout.panel.to_f64().contains(pos))?;
        let on_button = layouts[i]
            .button
            .is_some_and(|button| button.to_f64().contains(pos));
        let toast = self.toasts.remove(i)?;
        Some(match toast.action {
            Some((_, action)) if on_button => ToastHit::Action(action),
            _ => ToastHit::Dismiss,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Level, MAX_TOASTS, Toast, enqueue};
    use std::{collections::VecDeque, time::Instant};

    #[test]
    fn the_queue_lets_the_oldest_go_past_its_limit() {
        let mut toasts = VecDeque::new();
        let until = Instant::now();
        for i in 0..MAX_TOASTS + 2 {
            enqueue(
                &mut toasts,
                Toast::new(format!("toast {i}"), Level::Info, None, until),
            );
        }
        assert_eq!(toasts.len(), MAX_TOASTS);
        assert_eq!(toasts.front().map(|t| t.text.as_str()), Some("toast 2"));
        assert_eq!(toasts.back().map(|t| t.text.as_str()), Some("toast 4"));
        // Each toast is its own.
        let ids: Vec<u64> = toasts.iter().map(|t| t.id).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]));
    }
}

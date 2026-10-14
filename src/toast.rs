//! Toasts: short notes over the apps ("Switched to gerdr", "Zed didn't
//! start"), each with a level and, some, a button. At most `MAX_TOASTS`
//! queue; each goes after `TOAST_TIME`. `show_notice` is the old name for
//! an `Info` toast.

use std::time::{Duration, Instant};

use smithay::reexports::calloop::timer::{TimeoutAction, Timer};

use crate::{Gerdr, chrome::ToastView, menu::MenuAction};

/// How long a toast stays.
pub const TOAST_TIME: Duration = Duration::from_secs(3);
/// The most toasts kept; an older one goes when more come.
pub const MAX_TOASTS: usize = 3;

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
    pub text: String,
    pub level: Level,
    /// A button on the toast: its label and what it does.
    pub action: Option<(String, MenuAction)>,
    /// When the toast goes.
    pub until: Instant,
}

impl Toast {
    pub fn view(&self) -> ToastView {
        ToastView {
            text: self.text.clone(),
            level: self.level,
            action: self.action.as_ref().map(|(label, _)| label.clone()),
        }
    }
}

impl Gerdr {
    /// Show `text` for `TOAST_TIME`.
    pub fn toast(&mut self, text: String, level: Level) {
        self.toast_with(text, level, None);
    }

    /// Show `text` for `TOAST_TIME`, with a button doing `action`.
    pub fn toast_with(&mut self, text: String, level: Level, action: Option<(String, MenuAction)>) {
        let until = Instant::now() + TOAST_TIME;
        self.toasts.push_back(Toast {
            text,
            level,
            action,
            until,
        });
        while self.toasts.len() > MAX_TOASTS {
            self.toasts.pop_front();
        }
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

    /// Show `text` at the top of the app area for `TOAST_TIME`, in place
    /// of the notice shown, if any.
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
}

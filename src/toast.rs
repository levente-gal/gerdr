//! Toasts: short notes over the apps ("Switched to gerdr", "Zed didn't
//! start"), each with a level and, some, a button. At most `MAX_TOASTS`
//! queue; each goes after `TOAST_TIME`, or when clicked. A toast that goes
//! keeps its place in the stack while it fades out (`LEAVE_TIME`), so the
//! others don't slide over it. `show_notice` is the old name for an `Info`
//! toast.

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
    anim::FAST,
    chrome::{self, ToastView},
    menu::MenuAction,
};

/// How long a toast stays.
pub const TOAST_TIME: Duration = Duration::from_secs(4);
/// How long a toast that goes keeps its place: as long as its fade out
/// (`FAST`), with room for the frame that fade starts on.
const LEAVE_TIME: Duration = FAST.saturating_mul(2);
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
    /// How wide its button is, as the font sets the label (see
    /// `Chrome::toast_button_width`); 0 without one.
    pub button_w: i32,
    /// When the toast starts to go: it fades out from then, and is let go
    /// of `LEAVE_TIME` later.
    pub until: Instant,
}

impl Toast {
    pub fn new(
        text: String,
        level: Level,
        action: Option<(String, MenuAction)>,
        button_w: i32,
        until: Instant,
    ) -> Self {
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            text,
            level,
            action,
            button_w,
            until,
        }
    }

    /// Whether the toast is on its way out at `now`.
    fn leaving(&self, now: Instant) -> bool {
        now >= self.until
    }

    pub fn view(&self, now: Instant) -> ToastView {
        ToastView {
            id: self.id,
            text: self.text.clone(),
            level: self.level,
            action: self.action.as_ref().map(|(label, _)| label.clone()),
            button_w: self.button_w,
            leaving: self.leaving(now),
        }
    }
}

/// Put `toast` in the queue at `now`, sending the oldest on its way past
/// `MAX_TOASTS` (the ones already going don't count).
pub fn enqueue(toasts: &mut std::collections::VecDeque<Toast>, toast: Toast, now: Instant) {
    toasts.push_back(toast);
    let mut staying: Vec<&mut Toast> = toasts.iter_mut().filter(|t| !t.leaving(now)).collect();
    let extra = staying.len().saturating_sub(MAX_TOASTS);
    for toast in staying.iter_mut().take(extra) {
        toast.until = now;
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
        let now = Instant::now();
        let button_w = action
            .as_ref()
            .map_or(0, |(label, _)| self.chrome.toast_button_width(label));
        let toast = Toast::new(text, level, action, button_w, now + TOAST_TIME);
        enqueue(&mut self.toasts, toast, now);
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

    /// Let go of the toasts that went and had `LEAVE_TIME` to fade by `now`.
    pub fn expire_toasts(&mut self, now: Instant) {
        self.toasts.retain(|toast| now < toast.until + LEAVE_TIME);
    }

    /// The toasts in the stack, oldest first, the ones going too.
    pub fn toast_views(&self) -> Vec<ToastView> {
        let now = Instant::now();
        self.toasts.iter().map(|toast| toast.view(now)).collect()
    }

    /// What a press at `pos` lands on among the toasts staying, if any:
    /// that toast goes, and its button's action comes back to run.
    pub fn toast_press(&mut self, pos: Point<f64, Logical>) -> Option<ToastHit> {
        let now = Instant::now();
        self.expire_toasts(now);
        let views: Vec<ToastView> = self.toasts.iter().map(|toast| toast.view(now)).collect();
        let layouts = chrome::toast_layout(&views, self.content_rect());
        let i = layouts
            .iter()
            .zip(&views)
            .position(|(layout, view)| !view.leaving && layout.panel.to_f64().contains(pos))?;
        let on_button = layouts[i]
            .button
            .is_some_and(|button| button.to_f64().contains(pos));
        let toast = &mut self.toasts[i];
        toast.until = now;
        Some(match &toast.action {
            Some((_, action)) if on_button => ToastHit::Action(action.clone()),
            _ => ToastHit::Dismiss,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Level, MAX_TOASTS, TOAST_TIME, Toast, enqueue};
    use std::{collections::VecDeque, time::Instant};

    #[test]
    fn the_queue_sends_the_oldest_on_its_way_past_its_limit() {
        let mut toasts = VecDeque::new();
        let now = Instant::now();
        for i in 0..MAX_TOASTS + 2 {
            let toast = Toast::new(format!("toast {i}"), Level::Info, None, 0, now + TOAST_TIME);
            enqueue(&mut toasts, toast, now);
        }
        // The two oldest go, but keep their place while they fade.
        assert_eq!(toasts.len(), MAX_TOASTS + 2);
        let leaving: Vec<bool> = toasts.iter().map(|t| t.view(now).leaving).collect();
        assert_eq!(leaving, [true, true, false, false, false]);
        assert_eq!(toasts.back().map(|t| t.text.as_str()), Some("toast 4"));
        // Each toast is its own.
        let ids: Vec<u64> = toasts.iter().map(|t| t.id).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]));
    }
}

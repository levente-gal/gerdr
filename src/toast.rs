//! Toasts: short notes over the apps ("Switched to gerdr", "Zed didn't
//! start"), each with a level and, some, a button. At most `MAX_TOASTS`
//! queue, the least pressing going first when more come; each goes after
//! as long as its level says (`toast_time`), when clicked or its "×" is,
//! and waits while the pointer is over it (`Gerdr::toast_hover`). A toast
//! that goes keeps its place in the stack while it fades out
//! (`LEAVE_TIME`), so the others don't slide over it. `show_notice` is the
//! old name for an `Info` toast.

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
    chrome::{self, Rect, ToastView},
    menu::MenuAction,
};

/// How long a toast stays: news for a moment, what needs a look longer,
/// what failed longest; one with a button at least as long as an error,
/// to reach it.
pub const TOAST_TIME: Duration = Duration::from_secs(4);
const WARNING_TIME: Duration = Duration::from_secs(7);
const ERROR_TIME: Duration = Duration::from_secs(10);
const ACTION_TOAST_TIME: Duration = Duration::from_secs(10);
/// The least a toast stays once the pointer leaves it.
const RESUME_TIME: Duration = Duration::from_millis(1500);
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

/// How long a toast of `level` stays, with a button if `action`.
pub fn toast_time(level: Level, action: bool) -> Duration {
    let time = match level {
        Level::Info | Level::Success => TOAST_TIME,
        Level::Warning => WARNING_TIME,
        Level::Error => ERROR_TIME,
    };
    if action {
        time.max(ACTION_TOAST_TIME)
    } else {
        time
    }
}

/// How pressing a toast of `level` is, to choose which goes first when
/// too many come: what failed stays over what happened.
fn weight(level: Level) -> u8 {
    match level {
        Level::Info | Level::Success => 0,
        Level::Warning => 1,
        Level::Error => 2,
    }
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
    /// While the pointer is over it: how long it had left. It doesn't go
    /// meanwhile.
    pub paused: Option<Duration>,
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
            paused: None,
        }
    }

    /// Whether the toast is on its way out at `now`.
    pub(crate) fn leaving(&self, now: Instant) -> bool {
        self.paused.is_none() && now >= self.until
    }

    /// Hold the toast while the pointer is over it, if it isn't going.
    fn pause(&mut self, now: Instant) {
        if self.paused.is_none() && !self.leaving(now) {
            self.paused = Some(self.until - now);
        }
    }

    /// Let a held toast go on: what it had left, at least `RESUME_TIME`.
    /// How long it stays now, if it was held.
    fn resume(&mut self, now: Instant) -> Option<Duration> {
        let left = self.paused.take()?.max(RESUME_TIME);
        self.until = now + left;
        Some(left)
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

/// Put `toast` in the queue at `now`, sending others on their way past
/// `MAX_TOASTS` (the ones already going don't count): the least pressing
/// first, the oldest of those first, so news doesn't push out what
/// failed. The new one always stays.
pub fn enqueue(toasts: &mut std::collections::VecDeque<Toast>, toast: Toast, now: Instant) {
    toasts.push_back(toast);
    let newest = toasts.len() - 1;
    let mut staying: Vec<usize> = (0..newest).filter(|&i| !toasts[i].leaving(now)).collect();
    let extra = (staying.len() + 1).saturating_sub(MAX_TOASTS);
    // Stable: by weight, then by age.
    staying.sort_by_key(|&i| weight(toasts[i].level));
    for &i in staying.iter().take(extra) {
        toasts[i].paused = None;
        toasts[i].until = now;
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

    /// Show `text` for as long as `level` says (see `toast_time`), with a
    /// button doing `action`. It goes to the log too, so what was said
    /// can be read again.
    pub fn toast_with(&mut self, text: String, level: Level, action: Option<(String, MenuAction)>) {
        match level {
            Level::Error => tracing::warn!("toast: {text}"),
            _ => tracing::info!("toast: {text}"),
        }
        let now = Instant::now();
        let time = toast_time(level, action.is_some());
        let button_w = action
            .as_ref()
            .map_or(0, |(label, _)| self.chrome.toast_button_width(label));
        let toast = Toast::new(text, level, action, button_w, now + time);
        enqueue(&mut self.toasts, toast, now);
        self.time_toasts(time);
    }

    /// Let go of the toasts past their time `time` from now: gone from the
    /// next frame, as every frame draws the chrome.
    fn time_toasts(&mut self, time: Duration) {
        let clear = move |_, _: &mut (), state: &mut Gerdr| {
            state.expire_toasts(Instant::now());
            TimeoutAction::Drop
        };
        if let Err(err) = self
            .loop_handle
            .insert_source(Timer::from_duration(time + LEAVE_TIME), clear)
        {
            // `chrome_view` leaves it out once it is past anyway.
            tracing::warn!("timing the toast out: {err}");
        }
    }

    /// The pointer is at `pos`, or off the toasts (`None`): the toast under
    /// it waits there, the others go on with what they had left.
    pub fn toast_hover(&mut self, pos: Option<Point<f64, Logical>>) {
        let now = Instant::now();
        let views: Vec<ToastView> = self.toasts.iter().map(|toast| toast.view(now)).collect();
        let layouts = chrome::toast_layout(&views, self.toast_rect());
        let over = pos.and_then(|pos| {
            layouts
                .iter()
                .zip(&views)
                .position(|(layout, view)| !view.leaving && layout.panel.to_f64().contains(pos))
        });
        let mut longest = None;
        for (i, toast) in self.toasts.iter_mut().enumerate() {
            if Some(i) == over {
                toast.pause(now);
            } else if let Some(left) = toast.resume(now) {
                longest = longest.max(Some(left));
            }
        }
        if let Some(left) = longest {
            self.time_toasts(left);
        }
    }

    /// Show `text` over the apps for `TOAST_TIME`.
    pub fn show_notice(&mut self, text: String) {
        self.toast(text, Level::Info);
    }

    /// Show what went well.
    pub fn show_success(&mut self, text: String) {
        self.toast(text, Level::Success);
    }

    /// Show what needs a look but didn't fail.
    pub fn show_warning(&mut self, text: String) {
        self.toast(text, Level::Warning);
    }

    /// Show what failed.
    pub fn show_error(&mut self, text: String) {
        self.toast(text, Level::Error);
    }

    /// Let go of the toasts that went and had `LEAVE_TIME` to fade by `now`.
    pub fn expire_toasts(&mut self, now: Instant) {
        self.toasts
            .retain(|toast| toast.paused.is_some() || now < toast.until + LEAVE_TIME);
    }

    /// Where the toasts stack up: over the app area, above the button
    /// unzooming a zoomed pane.
    pub fn toast_rect(&self) -> Rect {
        chrome::toast_area(
            self.content_rect(),
            self.unzoom_button().map(|(_, rect)| rect),
        )
    }

    /// The toasts in the stack, oldest first, the ones going too.
    pub fn toast_views(&self) -> Vec<ToastView> {
        let now = Instant::now();
        self.toasts.iter().map(|toast| toast.view(now)).collect()
    }

    /// What a press at `pos` lands on among the toasts staying, if any:
    /// that toast goes (its "×" or anywhere on it), and its button's
    /// action comes back to run.
    pub fn toast_press(&mut self, pos: Point<f64, Logical>) -> Option<ToastHit> {
        let now = Instant::now();
        self.expire_toasts(now);
        let views: Vec<ToastView> = self.toasts.iter().map(|toast| toast.view(now)).collect();
        let layouts = chrome::toast_layout(&views, self.toast_rect());
        let i = layouts
            .iter()
            .zip(&views)
            .position(|(layout, view)| !view.leaving && layout.panel.to_f64().contains(pos))?;
        let on_button = layouts[i]
            .button
            .is_some_and(|button| button.to_f64().contains(pos));
        let toast = &mut self.toasts[i];
        toast.paused = None;
        toast.until = now;
        Some(match &toast.action {
            Some((_, action)) if on_button => ToastHit::Action(action.clone()),
            _ => ToastHit::Dismiss,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ERROR_TIME, Level, MAX_TOASTS, RESUME_TIME, TOAST_TIME, Toast, WARNING_TIME, enqueue,
        toast_time,
    };
    use std::{
        collections::VecDeque,
        time::{Duration, Instant},
    };

    #[test]
    fn errors_stay_longest_and_a_button_gives_time_to_reach_it() {
        assert_eq!(toast_time(Level::Info, false), TOAST_TIME);
        assert_eq!(toast_time(Level::Success, false), TOAST_TIME);
        assert_eq!(toast_time(Level::Warning, false), WARNING_TIME);
        assert_eq!(toast_time(Level::Error, false), ERROR_TIME);
        assert!(toast_time(Level::Info, true) >= ERROR_TIME);
        assert!(TOAST_TIME < WARNING_TIME && WARNING_TIME < ERROR_TIME);
    }

    #[test]
    fn news_goes_before_what_failed() {
        let mut toasts = VecDeque::new();
        let now = Instant::now();
        let mut push = |text: &str, level| {
            let toast = Toast::new(text.into(), level, None, 0, now + TOAST_TIME);
            enqueue(&mut toasts, toast, now);
        };
        push("failed", Level::Error);
        push("switched", Level::Info);
        push("started", Level::Info);
        push("created", Level::Success);
        let staying: Vec<&str> = toasts
            .iter()
            .filter(|t| !t.view(now).leaving)
            .map(|t| t.text.as_str())
            .collect();
        assert_eq!(staying, ["failed", "started", "created"]);
    }

    #[test]
    fn a_held_toast_waits_and_then_goes_on() {
        let now = Instant::now();
        let mut toast = Toast::new("x".into(), Level::Error, None, 0, now + ERROR_TIME);
        let later = now + Duration::from_secs(4);
        toast.pause(later);
        assert_eq!(toast.paused, Some(ERROR_TIME - Duration::from_secs(4)));
        // Long past its time, it is still there.
        let much_later = now + Duration::from_secs(60);
        assert!(!toast.view(much_later).leaving);
        assert_eq!(toast.resume(much_later), Some(Duration::from_secs(6)));
        assert!(!toast.view(much_later).leaving);
        assert!(toast.view(much_later + Duration::from_secs(6)).leaving);
        // One near its end gets a moment after the pointer leaves.
        let mut short = Toast::new("y".into(), Level::Info, None, 0, now + TOAST_TIME);
        short.pause(now + TOAST_TIME - Duration::from_millis(10));
        assert_eq!(short.resume(now), Some(RESUME_TIME));
        assert_eq!(short.resume(now), None);
    }

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

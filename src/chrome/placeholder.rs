//! The placeholder over a pane whose window has not shown up yet: a
//! panel with a spinner and "Starting Zed…", or once it took too long
//! (or the window went away), "Zed didn't start" ("Zed closed") with
//! Retry and Close buttons.

use std::time::{Duration, Instant};

use super::{
    Canvas, FONT_SIZE, Painter, Palette, Rect, SMALL_FONT_SIZE, icon_sdf, physical, spinner,
};
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// The spinner: its side, the gap under it, and how long one turn takes.
pub const SPINNER_SIZE: i32 = 24;
const SPINNER_GAP: i32 = 12;
pub const SPINNER_PERIOD: Duration = Duration::from_millis(900);
/// How many drawings one turn of the spinner is made of.
pub const SPINNER_STEPS: u32 = 24;
/// The label line, and the Close and Retry buttons side by side under it.
const LABEL_HEIGHT: i32 = 24;
const RETRY_GAP: i32 = 14;
const RETRY_WIDTH: i32 = 84;
const RETRY_HEIGHT: i32 = 30;
const BUTTON_GAP: i32 = 8;

/// What a placeholder shows: where the window goes, what is starting
/// there ("Zed"), since when, whether it took too long, and whether a
/// window it had went away.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaceholderView {
    pub rect: Rect,
    pub label: String,
    pub since: Instant,
    pub timed_out: bool,
    pub closed: bool,
    /// It waits for the keyring to unlock (see `keyring`).
    pub held: bool,
}

impl PlaceholderView {
    /// What the placeholder says.
    pub fn text(&self) -> String {
        if self.held {
            format!("{} waits for the keyring to unlock", self.label)
        } else if self.closed {
            format!("{} closed", self.label)
        } else if self.timed_out {
            format!("{} didn't start", self.label)
        } else {
            format!("Starting {}…", self.label)
        }
    }
}

/// A button of a timed out placeholder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceholderButton {
    /// Start the window again.
    Retry,
    /// Give up on the window, and its pane with it.
    Close,
}

/// Where the parts of a placeholder go in its pane's `rect` (window
/// coordinates): the spinner (while waiting), the label line and, once
/// `timed_out`, the Retry and Close buttons.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaceholderLayout {
    pub spinner: Option<Rect>,
    pub label: Rect,
    pub retry: Option<Rect>,
    pub close: Option<Rect>,
}

impl PlaceholderLayout {
    /// The button at `pos`, if one is.
    pub fn button_at(&self, pos: Point<f64, Logical>) -> Option<PlaceholderButton> {
        let at = |rect: Option<Rect>| rect.is_some_and(|r| r.to_f64().contains(pos));
        if at(self.retry) {
            Some(PlaceholderButton::Retry)
        } else if at(self.close) {
            Some(PlaceholderButton::Close)
        } else {
            None
        }
    }
}

pub fn placeholder_layout(rect: Rect, timed_out: bool) -> PlaceholderLayout {
    let cx = rect.loc.x + rect.size.w / 2;
    let cy = rect.loc.y + rect.size.h / 2;
    let label = |y: i32| Rectangle::new((rect.loc.x, y).into(), (rect.size.w, LABEL_HEIGHT).into());
    if timed_out {
        let h = LABEL_HEIGHT + RETRY_GAP + RETRY_HEIGHT;
        let top = cy - h / 2;
        let button = |x: i32| {
            Some(Rectangle::new(
                (x, top + LABEL_HEIGHT + RETRY_GAP).into(),
                (RETRY_WIDTH, RETRY_HEIGHT).into(),
            ))
        };
        PlaceholderLayout {
            spinner: None,
            label: label(top),
            // Close on the left, Retry, what it is for, on the right.
            close: button(cx - BUTTON_GAP / 2 - RETRY_WIDTH),
            retry: button(cx + BUTTON_GAP / 2),
        }
    } else {
        let h = SPINNER_SIZE + SPINNER_GAP + LABEL_HEIGHT;
        let top = cy - h / 2;
        PlaceholderLayout {
            spinner: Some(Rectangle::new(
                (cx - SPINNER_SIZE / 2, top).into(),
                (SPINNER_SIZE, SPINNER_SIZE).into(),
            )),
            label: label(top + SPINNER_SIZE + SPINNER_GAP),
            retry: None,
            close: None,
        }
    }
}

impl Painter {
    /// The placeholder's panel, the size of its pane: its text and, once
    /// it timed out, the Retry and Close buttons (the `hovered` one lit).
    /// The spinner is drawn apart, as it moves.
    pub(super) fn placeholder(
        &self,
        view: &PlaceholderView,
        hovered: Option<PlaceholderButton>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let size = view.rect.size;
        let mut c = Canvas::new(physical(size, scale));
        c.fill(0, 0, c.w, c.h, p.panel);
        // Parts relative to the panel.
        let local = Rectangle::new((0, 0).into(), size);
        let layout = placeholder_layout(local, view.timed_out);
        let color = if view.timed_out { p.text } else { p.muted };
        self.centered(&mut c, &view.text(), FONT_SIZE, layout.label, scale, color);
        if let Some(close) = layout.close {
            let lit = hovered == Some(PlaceholderButton::Close);
            self.button(&mut c, "Close", close, (lit, false), false, scale, p);
        }
        if let Some(retry) = layout.retry {
            let lit = hovered == Some(PlaceholderButton::Retry);
            self.button(&mut c, "Retry", retry, (lit, false), true, scale, p);
            let hint = Rectangle::new(
                (0, retry.loc.y + retry.size.h + 6).into(),
                (size.w, LABEL_HEIGHT).into(),
            );
            self.centered(
                &mut c,
                "Its output is in the launch log",
                SMALL_FONT_SIZE,
                hint,
                scale,
                p.muted,
            );
        }
        c.into_buffer(true)
    }

    /// The spinner, `SPINNER_SIZE` square, turned `step` of
    /// `SPINNER_STEPS` of a full circle.
    pub(super) fn spinner(&self, step: u32, scale: f64, p: &Palette) -> MemoryRenderBuffer {
        let size = Size::from((SPINNER_SIZE, SPINNER_SIZE));
        let mut c = Canvas::new(physical(size, scale));
        c.fill(0, 0, c.w, c.h, p.panel);
        let turn = step as f32 / SPINNER_STEPS as f32;
        let s = scale as f32 * 1.4;
        icon_sdf(
            &mut c,
            Rectangle::new((0, 0).into(), size),
            scale,
            p.accent,
            |x, y| spinner(turn, x, y, s),
        );
        c.into_buffer(true)
    }
}

#[cfg(test)]
mod tests {
    use super::{PlaceholderButton, PlaceholderView, SPINNER_SIZE, placeholder_layout};
    use crate::chrome::{Rect, inside};
    use smithay::utils::{Logical, Point};
    use std::time::Instant;

    #[test]
    fn the_placeholder_says_what_starts_and_when_it_did_not() {
        let rect = Rect::new((200, 48).into(), (600, 400).into());
        let waiting = PlaceholderView {
            rect,
            label: "Zed".into(),
            since: Instant::now(),
            timed_out: false,
            closed: false,
            held: false,
        };
        assert_eq!(waiting.text(), "Starting Zed…");
        let held = PlaceholderView {
            label: "chromium-browser".into(),
            held: true,
            ..waiting.clone()
        };
        assert_eq!(
            held.text(),
            "chromium-browser waits for the keyring to unlock"
        );
        let layout = placeholder_layout(rect, false);
        let spinner = layout.spinner.expect("a spinner while waiting");
        assert_eq!(spinner.size, (SPINNER_SIZE, SPINNER_SIZE).into());
        assert_eq!(spinner.loc.x + SPINNER_SIZE / 2, 500);
        assert!(inside(rect, spinner) && inside(rect, layout.label));
        assert!(layout.retry.is_none() && layout.close.is_none());

        let late = PlaceholderView {
            timed_out: true,
            ..waiting.clone()
        };
        assert_eq!(late.text(), "Zed didn't start");
        let gone = PlaceholderView {
            closed: true,
            ..late
        };
        assert_eq!(gone.text(), "Zed closed");
        let layout = placeholder_layout(rect, true);
        let retry = layout.retry.expect("a Retry button once timed out");
        let close = layout.close.expect("a Close button once timed out");
        assert!(layout.spinner.is_none());
        assert!(inside(rect, retry) && inside(rect, close));
        assert!(retry.loc.y >= layout.label.loc.y + layout.label.size.h);
        // Side by side, Close first, centered together.
        assert_eq!(retry.loc.y, close.loc.y);
        assert!(close.loc.x + close.size.w < retry.loc.x);
        assert_eq!(
            close.loc.x + (retry.loc.x + retry.size.w - close.loc.x) / 2,
            500
        );
        let middle = |r: Rect| {
            Point::<i32, Logical>::from((r.loc.x + r.size.w / 2, r.loc.y + r.size.h / 2)).to_f64()
        };
        assert_eq!(
            layout.button_at(middle(retry)),
            Some(PlaceholderButton::Retry)
        );
        assert_eq!(
            layout.button_at(middle(close)),
            Some(PlaceholderButton::Close)
        );
        assert_eq!(layout.button_at(middle(layout.label)), None);
    }
}

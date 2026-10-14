//! The placeholder over a pane whose window has not shown up yet: a
//! panel with a spinner and "Starting Zed…", or once it took too long,
//! "Zed didn't start" and a Retry button.

use std::time::{Duration, Instant};

use super::{
    Canvas, FONT_SIZE, Painter, Palette, Rect, SMALL_FONT_SIZE, icon_sdf, physical, spinner,
};
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Rectangle, Size};

/// The spinner: its side, the gap under it, and how long one turn takes.
pub const SPINNER_SIZE: i32 = 24;
const SPINNER_GAP: i32 = 12;
pub const SPINNER_PERIOD: Duration = Duration::from_millis(900);
/// How many drawings one turn of the spinner is made of.
pub const SPINNER_STEPS: u32 = 24;
/// The label line, and the Retry button under it.
const LABEL_HEIGHT: i32 = 24;
const RETRY_GAP: i32 = 14;
const RETRY_WIDTH: i32 = 84;
const RETRY_HEIGHT: i32 = 30;

/// What a placeholder shows: where the window goes, what is starting
/// there ("Zed"), since when, and whether it took too long.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaceholderView {
    pub rect: Rect,
    pub label: String,
    pub since: Instant,
    pub timed_out: bool,
}

impl PlaceholderView {
    /// What the placeholder says.
    pub fn text(&self) -> String {
        if self.timed_out {
            format!("{} didn't start", self.label)
        } else {
            format!("Starting {}…", self.label)
        }
    }
}

/// Where the parts of a placeholder go in its pane's `rect` (window
/// coordinates): the spinner (while waiting), the label line and, once
/// `timed_out`, the Retry button.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaceholderLayout {
    pub spinner: Option<Rect>,
    pub label: Rect,
    pub retry: Option<Rect>,
}

pub fn placeholder_layout(rect: Rect, timed_out: bool) -> PlaceholderLayout {
    let cx = rect.loc.x + rect.size.w / 2;
    let cy = rect.loc.y + rect.size.h / 2;
    let label = |y: i32| Rectangle::new((rect.loc.x, y).into(), (rect.size.w, LABEL_HEIGHT).into());
    if timed_out {
        let h = LABEL_HEIGHT + RETRY_GAP + RETRY_HEIGHT;
        let top = cy - h / 2;
        PlaceholderLayout {
            spinner: None,
            label: label(top),
            retry: Some(Rectangle::new(
                (cx - RETRY_WIDTH / 2, top + LABEL_HEIGHT + RETRY_GAP).into(),
                (RETRY_WIDTH, RETRY_HEIGHT).into(),
            )),
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
        }
    }
}

impl Painter {
    /// The placeholder's panel, the size of its pane: its text and, once
    /// it timed out, the Retry button (lit when `hovered`). The spinner is
    /// drawn apart, as it moves.
    pub(super) fn placeholder(
        &self,
        view: &PlaceholderView,
        hovered: bool,
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
        if let Some(retry) = layout.retry {
            self.button(&mut c, "Retry", retry, (hovered, false), true, scale, p);
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
    use super::{PlaceholderView, SPINNER_SIZE, placeholder_layout};
    use crate::chrome::{Rect, inside};
    use std::time::Instant;

    #[test]
    fn the_placeholder_says_what_starts_and_when_it_did_not() {
        let rect = Rect::new((200, 48).into(), (600, 400).into());
        let waiting = PlaceholderView {
            rect,
            label: "Zed".into(),
            since: Instant::now(),
            timed_out: false,
        };
        assert_eq!(waiting.text(), "Starting Zed…");
        let layout = placeholder_layout(rect, false);
        let spinner = layout.spinner.expect("a spinner while waiting");
        assert_eq!(spinner.size, (SPINNER_SIZE, SPINNER_SIZE).into());
        assert_eq!(spinner.loc.x + SPINNER_SIZE / 2, 500);
        assert!(inside(rect, spinner) && inside(rect, layout.label));
        assert!(layout.retry.is_none());

        let late = PlaceholderView {
            timed_out: true,
            ..waiting
        };
        assert_eq!(late.text(), "Zed didn't start");
        let layout = placeholder_layout(rect, true);
        let retry = layout.retry.expect("a Retry button once timed out");
        assert!(layout.spinner.is_none());
        assert!(inside(rect, retry));
        assert!(retry.loc.y >= layout.label.loc.y + layout.label.size.h);
        assert_eq!(retry.loc.x + retry.size.w / 2, 500);
    }
}

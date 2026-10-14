//! Timing for the chrome's motion: a fade in or out over a short while,
//! and the steps its value is quantized to so a cached buffer is drawn
//! again only when the step changes.

// Nothing fades yet: the overlays take these up next.
#![allow(dead_code)]

use std::time::{Duration, Instant};

/// How long a popup, tooltip or hover fades in or out.
pub const FAST: Duration = Duration::from_millis(120);
/// How long a larger surface (the welcome screen, a pane bar) fades.
pub const SLOW: Duration = Duration::from_millis(150);

/// How many steps a fade's value is quantized to.
const STEPS: f32 = 16.0;

/// A value going from 0 to 1 (`rising`) or from 1 to 0 over `dur` from
/// `start`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fade {
    pub start: Instant,
    pub dur: Duration,
    pub rising: bool,
}

impl Fade {
    pub fn new(start: Instant, dur: Duration, rising: bool) -> Self {
        Self { start, dur, rising }
    }

    /// Where the fade stands at `now`, from 0 to 1.
    pub fn value(&self, now: Instant) -> f32 {
        let t = if self.dur.is_zero() {
            1.0
        } else {
            (now.saturating_duration_since(self.start).as_secs_f32() / self.dur.as_secs_f32())
                .clamp(0.0, 1.0)
        };
        if self.rising { t } else { 1.0 - t }
    }

    /// Whether the fade has reached its end at `now`.
    pub fn done(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.start) >= self.dur
    }
}

/// `a` (0 to 1) as one of `STEPS` steps, for a cache key.
pub fn quantize(a: f32) -> u8 {
    (a.clamp(0.0, 1.0) * STEPS).round() as u8
}

#[cfg(test)]
mod tests {
    use super::{FAST, Fade, quantize};
    use std::time::{Duration, Instant};

    #[test]
    fn a_fade_runs_between_its_ends() {
        let start = Instant::now();
        let up = Fade::new(start, FAST, true);
        assert_eq!(up.value(start), 0.0);
        assert_eq!(up.value(start + FAST), 1.0);
        assert_eq!(up.value(start + 2 * FAST), 1.0);
        assert!(!up.done(start) && up.done(start + FAST));
        let mid = up.value(start + FAST / 2);
        assert!(mid > 0.4 && mid < 0.6);

        let down = Fade::new(start, FAST, false);
        assert_eq!(down.value(start), 1.0);
        assert_eq!(down.value(start + FAST), 0.0);
        // Before its start it has not moved.
        assert_eq!(down.value(start - Duration::from_secs(1)), 1.0);
        assert_eq!(Fade::new(start, Duration::ZERO, true).value(start), 1.0);
    }

    #[test]
    fn quantize_has_sixteen_steps() {
        assert_eq!(quantize(0.0), 0);
        assert_eq!(quantize(1.0), 16);
        assert_eq!(quantize(0.5), 8);
        assert_eq!(quantize(0.51), 8);
        assert_eq!(quantize(-1.0), 0);
        assert_eq!(quantize(2.0), 16);
    }
}

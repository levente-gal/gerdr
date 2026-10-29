//! Timing for the chrome's motion: a fade in or out over a short while,
//! and the steps its value is quantized to so a cached buffer is drawn
//! again only when the step changes.

use std::time::{Duration, Instant};

/// How long a popup, tooltip or hover fades in or out.
pub const FAST: Duration = Duration::from_millis(120);
/// How long a larger surface (the welcome screen, a pane bar) fades.
pub const SLOW: Duration = Duration::from_millis(150);
/// How long a blocked agent's pulse takes to go up and back down.
pub const PULSE: Duration = Duration::from_millis(1500);
/// How long a running dot's blink takes, dim to bright and back.
pub const BLINK: Duration = Duration::from_millis(1200);
/// How dim a blinking dot gets: never gone.
const BLINK_LOW: f32 = 0.35;

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

    /// A fade over `dur` that stands at `value` at `now` and goes on from
    /// there: what a fade turning around mid-way becomes, so a popup
    /// closed while still appearing fades out from where it got to.
    pub fn from_value(now: Instant, dur: Duration, rising: bool, value: f32) -> Self {
        let value = value.clamp(0.0, 1.0);
        let done = if rising { value } else { 1.0 - value };
        let start = now.checked_sub(dur.mul_f32(done)).unwrap_or(now);
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

/// What the pointer is on and since when, and what it just left, each
/// with its fade: a hover pill fades in under the pointer and out after
/// it, and one left half-way comes back from where it stands.
#[derive(Clone, Copy, Debug)]
pub struct Hover<T> {
    over: Option<(T, Fade)>,
    leaving: Option<(T, Fade)>,
}

impl<T> Default for Hover<T> {
    fn default() -> Self {
        Self {
            over: None,
            leaving: None,
        }
    }
}

impl<T: Copy + PartialEq> Hover<T> {
    /// The pointer is on `hit` (nothing with a pill when `None`) at `now`.
    pub fn track(&mut self, hit: Option<T>, now: Instant) {
        if self.over.map(|(h, _)| h) == hit {
            return;
        }
        let turn = |fade: Fade| Fade::from_value(now, fade.dur, !fade.rising, fade.value(now));
        if let Some((h, fade)) = self.over.take() {
            self.leaving = Some((h, turn(fade)));
        }
        if let Some(hit) = hit {
            // Back on what was being left: carry on from where its fade is.
            let fade = match self.leaving {
                Some((left, fade)) if left == hit => turn(fade),
                _ => Fade::new(now, FAST, true),
            };
            self.over = Some((hit, fade));
        }
    }

    /// What the pointer is on, if it is on something with a pill.
    pub fn over(&self) -> Option<T> {
        self.over.map(|(h, _)| h)
    }

    /// How lit `hit`'s pill is at `now`, from 0 to 1.
    pub fn alpha(&self, hit: T, now: Instant) -> f32 {
        [self.over, self.leaving]
            .into_iter()
            .flatten()
            .find(|(h, _)| *h == hit)
            .map_or(0.0, |(_, fade)| fade.value(now))
    }

    /// How lit the pill under the pointer is at `now`, in `quantize`
    /// steps; 0 with nothing under it.
    pub fn alpha_over(&self, now: Instant) -> u8 {
        self.over
            .map_or(0, |(hit, _)| quantize(self.alpha(hit, now)))
    }
}

/// `a` (0 to 1) as one of `STEPS` steps, for a cache key.
pub fn quantize(a: f32) -> u8 {
    (a.clamp(0.0, 1.0) * STEPS).round() as u8
}

/// Where a loop of `period` stands at `now`, from 0 to 1 and over again:
/// for a spinner or a pulse.
pub fn phase(start: Instant, period: Duration, now: Instant) -> f32 {
    if period.is_zero() {
        return 0.0;
    }
    let t = now.saturating_duration_since(start).as_secs_f32() / period.as_secs_f32();
    t - t.floor()
}

/// How opaque a blinking dot is at `now`, counted from `epoch`: bright at
/// the start of every `BLINK`, `BLINK_LOW` half-way, easing in between.
pub fn blink(epoch: Instant, now: Instant) -> f32 {
    let wave = 0.5 + 0.5 * (std::f32::consts::TAU * phase(epoch, BLINK, now)).cos();
    BLINK_LOW + (1.0 - BLINK_LOW) * wave
}

/// Where a blocked agent's pulse stands at `now`: from 0 up to 1 and back
/// down every `PULSE`, counted from `epoch`.
pub fn pulse(epoch: Instant, now: Instant) -> f32 {
    1.0 - (2.0 * phase(epoch, PULSE, now) - 1.0).abs()
}

#[cfg(test)]
mod tests {
    use super::{BLINK, BLINK_LOW, FAST, Fade, Hover, PULSE, blink, phase, pulse, quantize};
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
    fn a_fade_turned_around_goes_on_from_where_it_stands() {
        let start = Instant::now();
        let up = Fade::new(start, FAST, true);
        let mid = start + FAST / 4;
        let value = up.value(mid);
        let down = Fade::from_value(mid, FAST, false, value);
        assert!((down.value(mid) - value).abs() < 0.01);
        assert_eq!(down.value(mid + FAST), 0.0);
        // Already at the end, it is done at once.
        assert!(Fade::from_value(mid, FAST, true, 1.0).done(mid));
        assert!(Fade::from_value(mid, FAST, false, 0.0).done(mid));
    }

    #[test]
    fn hover_fades_in_out_and_back() {
        let start = Instant::now();
        let mut hover = Hover::default();
        hover.track(Some(0), start);
        assert_eq!(hover.over(), Some(0));
        assert_eq!(hover.alpha(0, start), 0.0);
        assert_eq!(hover.alpha(0, start + FAST), 1.0);
        assert_eq!(hover.alpha(1, start + FAST), 0.0);
        assert_eq!(hover.alpha_over(start + FAST), 16);
        // Leaving: the old pill fades out while the new fades in.
        let later = start + 2 * FAST;
        hover.track(Some(1), later);
        assert_eq!(hover.alpha(0, later), 1.0);
        assert_eq!(hover.alpha(0, later + FAST), 0.0);
        assert_eq!(hover.alpha(1, later + FAST), 1.0);
        // Back onto the one half faded out: it comes back from there.
        let mid = later + 2 * FAST;
        hover.track(Some(0), mid);
        let half = mid + FAST / 2;
        hover.track(None, half);
        assert_eq!(hover.over(), None);
        assert_eq!(hover.alpha_over(half), 0);
        let v = hover.alpha(0, half);
        assert!(v > 0.4 && v < 0.6, "{v}");
        hover.track(Some(0), half);
        assert!((hover.alpha(0, half) - v).abs() < 0.01);
        assert_eq!(hover.alpha(0, half + FAST), 1.0);
    }

    #[test]
    fn a_blink_dims_and_comes_back_but_never_goes() {
        let epoch = Instant::now();
        assert!((blink(epoch, epoch) - 1.0).abs() < 1e-4);
        assert!((blink(epoch, epoch + BLINK / 2) - BLINK_LOW).abs() < 1e-3);
        assert!((blink(epoch, epoch + BLINK * 7) - 1.0).abs() < 1e-3);
        for ms in (0..2400).step_by(37) {
            let a = blink(epoch, epoch + Duration::from_millis(ms));
            assert!((BLINK_LOW - 1e-4..=1.0 + 1e-4).contains(&a));
        }
    }

    #[test]
    fn the_pulse_loops() {
        let epoch = Instant::now();
        assert_eq!(pulse(epoch, epoch), 0.0);
        assert_eq!(pulse(epoch, epoch + PULSE / 2), 1.0);
        assert!(pulse(epoch, epoch + PULSE) < 0.01);
        let quarter = pulse(epoch, epoch + PULSE / 4);
        assert!(quarter > 0.45 && quarter < 0.55, "{quarter}");
        let three = pulse(epoch, epoch + PULSE * 3 / 4);
        assert!(three > 0.45 && three < 0.55, "{three}");
        assert!(pulse(epoch, epoch + PULSE * 10 + Duration::from_millis(750)) > 0.99);
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

    #[test]
    fn a_phase_wraps_around() {
        let start = Instant::now();
        let period = Duration::from_secs(1);
        assert_eq!(phase(start, period, start), 0.0);
        assert!((phase(start, period, start + Duration::from_millis(250)) - 0.25).abs() < 0.001);
        assert!((phase(start, period, start + Duration::from_millis(1250)) - 0.25).abs() < 0.001);
        assert_eq!(phase(start, Duration::ZERO, start + period), 0.0);
    }
}

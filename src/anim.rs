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

/// Where a blocked agent's pulse stands at `now`: from 0 up to 1 and back
/// down every `PULSE`, counted from `epoch`.
pub fn pulse(epoch: Instant, now: Instant) -> f32 {
    1.0 - (2.0 * phase(epoch, PULSE, now) - 1.0).abs()
}

#[cfg(test)]
mod tests {
    use super::{FAST, Fade, PULSE, phase, pulse, quantize};
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

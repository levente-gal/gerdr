//! Where a scrolled list stands and how it moves there: the wheel's
//! notches glide (each eases out over a few frames, one turned while
//! another glides lengthening the glide), a touchpad moves it with the
//! fingers (one to one, stretching past either end against a rubber band),
//! and fingers lifted while moving fling it on, slowing as it goes, until
//! it stops or bounces off an end. `tick` moves it on each frame.
//!
//! A position is in the list's own units: rows for a list scrolled by rows
//! (its whole part the first row shown, the rest how far that row is
//! scrolled past), logical pixels for one scrolled by pixels. `Bounds` says
//! how long a unit is where the list stands.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

/// What moved a list.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Motion {
    /// The mouse wheel turned by notches, down positive: each goes
    /// `NOTCH_PX` at least, and further turned fast (see `BURST`).
    Wheel(f64),
    /// The list scrolled by as many of its notches (`Bounds::notch`).
    Notches(f64),
    /// A touchpad's fingers moved the list by logical pixels (down
    /// positive) at `time`, the event's clock in milliseconds.
    Finger { px: f64, time: u32 },
    /// The fingers left the touchpad at `time`.
    Lift { time: u32 },
}

/// The list a motion moves, as it stands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    /// The furthest position, in units (the nearest is 0).
    pub max: f64,
    /// Units a notch of the wheel scrolls.
    pub notch: f64,
    /// Logical pixels in the unit where the list stands: the first row's
    /// height for a list of rows, 1 for one of pixels.
    pub unit: f64,
    /// Logical pixels in the units after and before it (the rows under and
    /// over the first): fingers moving the list into one go by its own
    /// height.
    pub next: f64,
    pub prev: f64,
    /// The list's height in logical pixels, which a stretch past an end
    /// pulls against.
    pub view: f64,
}

impl Bounds {
    /// How many units `px` logical pixels move a list standing at `pos`
    /// (down positive): by the height of each unit they cross, into the
    /// next or the one before (further ones taken as high as those).
    fn units(&self, pos: f64, px: f64) -> f64 {
        let (h, part) = (self.unit.max(1.0), pos - pos.floor());
        let room = if px >= 0.0 { 1.0 - part } else { -part };
        if (px / h).abs() <= room.abs() {
            return px / h;
        }
        let beyond = if px >= 0.0 { self.next } else { self.prev };
        room + (px - room * h) / beyond.max(1.0)
    }

    /// The logical pixels moving a list from `from` to `to` takes: the
    /// inverse of `units`.
    fn px(&self, from: f64, to: f64) -> f64 {
        let (h, part) = (self.unit.max(1.0), from - from.floor());
        let (by, room) = (to - from, if to >= from { 1.0 - part } else { -part });
        if by.abs() <= room.abs() {
            return by * h;
        }
        let beyond = if to >= from { self.next } else { self.prev };
        room * h + (by - room) * beyond.max(1.0)
    }
}

/// How far a notch of the wheel scrolls, logical pixels, at least.
const NOTCH_PX: f64 = 100.0;
/// Notches turned within this long of the last go further each time, up
/// to `BURST_MAX` times as far: a wheel spun fast covers more ground.
const BURST: Duration = Duration::from_millis(120);
const BURST_STEP: f64 = 1.3;
const BURST_MAX: f64 = 3.0;
/// A glide's time constant: a notch's way is 95% gone after three.
const GLIDE: f64 = 0.045;
/// A fling's time constant: its speed falls to a third in as long.
const FLING: f64 = 0.4;
/// The slowest a lift flings the list, and where a fling stops (logical
/// pixels a second).
const FLING_MIN: f64 = 120.0;
const FLING_STOP: f64 = 15.0;
/// The fastest a fling hits an end with, as the bounce sees it.
const BOUNCE_MAX: f64 = 2400.0;
/// How stiff the band pulling a list back from past an end is (its
/// natural frequency, a second): back within a quarter second.
const SPRING: f64 = 18.0;
/// How far back the fingers' speed is measured from their last move, and
/// how long they may rest before a lift flings nothing.
const SAMPLES: u32 = 100;
const REST: u32 = 50;
/// Without a lift, how long after the last move the fingers count as gone
/// (a device that never says it lifted, such as a pointing stick).
const LET_GO: Duration = Duration::from_millis(150);
/// The rubber band's give: how much of a pull past an end shows.
const GIVE: f64 = 0.55;

/// A scrolled list's position, and how it is moving.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scroll {
    /// In units, within the bounds.
    pos: f64,
    /// How far the fingers pulled past an end (logical pixels, positive
    /// past the far end, negative before the start), before the band.
    pull: f64,
    /// How far past an end the list shows (as `pull`, after the band).
    over: f64,
    moving: Option<Moving>,
    /// The fingers' recent moves: event time and logical pixels.
    samples: VecDeque<(u32, f64)>,
    /// When the fingers last moved, while they are on the touchpad.
    touched: Option<Instant>,
    /// When the wheel last turned, and how many times further than a
    /// notch's way its notches go now (see `BURST`).
    turned: Option<(Instant, f64)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Moving {
    /// Easing to `target` (units).
    Glide { target: f64, at: Instant },
    /// Going on at `vel` units a second, slowing.
    Fling { vel: f64, at: Instant },
    /// Coming back from past an end at `vel` logical pixels a second (as
    /// `over`).
    Spring { vel: f64, at: Instant },
}

impl Scroll {
    /// A list standing at `pos`.
    pub fn at(pos: f64) -> Self {
        Self {
            pos,
            ..Self::default()
        }
    }

    /// Where the list stands, in units.
    pub fn pos(&self) -> f64 {
        self.pos
    }

    /// The first row shown of a list of rows.
    pub fn first(&self) -> usize {
        self.pos.max(0.0).floor() as usize
    }

    /// How far its first row is scrolled past, from 0 up to 1.
    pub fn part(&self) -> f64 {
        self.pos.max(0.0).fract()
    }

    /// How far the list's rows are moved down past where `pos` puts them,
    /// in logical pixels: down past the start (positive) or up past the
    /// end (negative), while it stretches or bounces.
    pub fn overscroll(&self) -> f64 {
        -self.over
    }

    /// Whether the list moves on its own or under the fingers at `now`:
    /// what is under the pointer is left as it was meanwhile.
    pub fn moving(&self, now: Instant) -> bool {
        self.moving.is_some() || self.touched.is_some_and(|t| now - t < LET_GO)
    }

    /// Stand at `pos` and stop: the list was moved otherwise (by keys, to
    /// show the selection, say).
    pub fn jump(&mut self, pos: f64) {
        *self = Self::at(pos);
    }

    /// As `jump`, unless the list stands at row `first` already: the
    /// part of a row it stands past, and its motion, are kept then.
    pub fn jump_row(&mut self, first: usize) {
        if self.first() != first || self.pos < 0.0 {
            self.jump(first as f64);
        }
    }

    /// Stop a fling where the list stands; whether one was going.
    pub fn stop_fling(&mut self) -> bool {
        let flung = matches!(self.moving, Some(Moving::Fling { .. }));
        if flung {
            self.moving = None;
        }
        flung
    }

    /// Glide to `pos` from `now`, as a notch would: the list was asked to
    /// show something (the active workspace, say) without a jump.
    pub fn glide_to(&mut self, pos: f64, now: Instant, b: Bounds) {
        self.advance(now, b);
        self.unpull();
        self.samples.clear();
        self.touched = None;
        let target = pos.clamp(0.0, b.max.max(0.0));
        self.moving = (target != self.pos).then_some(Moving::Glide { target, at: now });
    }

    /// Take `motion` at `now`.
    pub fn apply(&mut self, motion: Motion, now: Instant, b: Bounds) {
        self.advance(now, b);
        match motion {
            Motion::Wheel(n) => {
                let burst = match self.turned {
                    Some((at, burst)) if now.saturating_duration_since(at) < BURST => {
                        (burst * BURST_STEP).min(BURST_MAX)
                    }
                    _ => 1.0,
                };
                self.turned = Some((now, burst));
                let step = b.notch.max(NOTCH_PX / b.unit.max(1.0)) * burst;
                self.glide_by(n * step, now, b);
            }
            Motion::Notches(n) => self.glide_by(n * b.notch, now, b),
            Motion::Finger { px, time } => {
                // Pixels while the wheel's notches glide are the wheel's
                // own, turned part of a notch (a fine-grained wheel's, which
                // come as pixels): they go with the glide.
                if let Some(Moving::Glide { target, at }) = self.moving {
                    let target = (target + px / b.unit.max(1.0)).clamp(0.0, b.max.max(0.0));
                    self.moving = Some(Moving::Glide { target, at });
                    return;
                }
                // The fingers catch the list, also back from past an end.
                if let Some(Moving::Spring { .. }) = self.moving {
                    self.pull = unband(self.over, b.view);
                }
                self.moving = None;
                self.touched = Some(now);
                self.samples.push_back((time, px));
                while self
                    .samples
                    .front()
                    .is_some_and(|(t, _)| time.wrapping_sub(*t) > SAMPLES)
                {
                    self.samples.pop_front();
                }
                self.pull_by(px, b);
            }
            // Only fingers that moved the list let go of it.
            Motion::Lift { .. } if self.touched.is_none() => {}
            Motion::Lift { time } => {
                let vel = self.speed(time);
                self.samples.clear();
                self.touched = None;
                self.let_go(vel, now, b);
            }
        }
    }

    /// Glide `by` units on from where the list stands, or where it glides
    /// to.
    fn glide_by(&mut self, by: f64, now: Instant, b: Bounds) {
        self.unpull();
        let from = match self.moving {
            Some(Moving::Glide { target, .. }) => target,
            _ => self.pos,
        };
        let target = (from + by).clamp(0.0, b.max.max(0.0));
        self.moving = (target != self.pos).then_some(Moving::Glide { target, at: now });
    }

    /// Move on to `now`; whether the list still moves then (see `moving`).
    pub fn tick(&mut self, now: Instant, b: Bounds) -> bool {
        self.advance(now, b);
        self.moving(now)
    }

    /// The fingers' speed when they left at `time`, logical pixels a second.
    fn speed(&self, time: u32) -> f64 {
        let (Some(&(first, _)), Some(&(last, _))) = (self.samples.front(), self.samples.back())
        else {
            return 0.0;
        };
        if time.wrapping_sub(last) > REST || last == first {
            return 0.0;
        }
        // The first sample's move happened before its time; the rest after.
        let moved: f64 = self.samples.iter().skip(1).map(|(_, px)| px).sum();
        moved * 1000.0 / last.wrapping_sub(first) as f64
    }

    /// The fingers let go moving at `vel` logical pixels a second: back
    /// from past an end, else flung on if fast enough.
    fn let_go(&mut self, vel: f64, now: Instant, b: Bounds) {
        self.pull = 0.0;
        self.moving = if self.over != 0.0 {
            Some(Moving::Spring { vel: 0.0, at: now })
        } else if vel.abs() >= FLING_MIN && b.max > 0.0 {
            Some(Moving::Fling {
                vel: vel / b.unit.max(1.0),
                at: now,
            })
        } else {
            None
        };
    }

    /// Move by `px` logical pixels under the fingers: first out of any
    /// pull past an end, then within the bounds, then past an end.
    fn pull_by(&mut self, mut px: f64, b: Bounds) {
        if self.pull != 0.0 {
            let pull = self.pull + px;
            if pull.signum() == self.pull.signum() {
                self.pull = pull;
                self.over = band(pull, b.view);
                return;
            }
            px = pull;
            self.pull = 0.0;
            self.over = 0.0;
        }
        let max = b.max.max(0.0);
        let (from, pos) = (self.pos, self.pos + b.units(self.pos, px));
        self.pos = pos.clamp(0.0, max);
        // A list that fits doesn't stretch.
        if max > 0.0 && pos != self.pos {
            // What is left of the move once at the end.
            self.pull = px - b.px(from, self.pos);
            self.over = band(self.pull, b.view);
        }
    }

    /// Back within the bounds at once, from past an end.
    fn unpull(&mut self) {
        self.pull = 0.0;
        self.over = 0.0;
    }

    /// Move on with what moves the list, to `now`.
    fn advance(&mut self, now: Instant, b: Bounds) {
        let max = b.max.max(0.0);
        if self.pos > max {
            self.pos = max;
        }
        // Fingers that went without saying so let go of the list.
        if self.touched.is_some_and(|t| now - t >= LET_GO) {
            self.touched = None;
            self.samples.clear();
            if self.pull != 0.0 {
                self.let_go(0.0, now, b);
            }
        }
        let unit = b.unit.max(1.0);
        self.moving = match self.moving {
            None => None,
            Some(Moving::Glide { target, at }) => {
                let target = target.min(max);
                let left = (target - self.pos) * (-secs(at, now) / GLIDE).exp();
                if left.abs() * unit < 0.5 {
                    self.pos = target;
                    None
                } else {
                    self.pos = target - left;
                    Some(Moving::Glide { target, at: now })
                }
            }
            Some(Moving::Fling { vel, at }) => {
                let decay = (-secs(at, now) / FLING).exp();
                let pos = self.pos + vel * FLING * (1.0 - decay);
                let vel = vel * decay;
                self.pos = pos.clamp(0.0, max);
                if pos != self.pos {
                    // Off an end: the list bounces there.
                    let hit = (vel * unit).clamp(-BOUNCE_MAX, BOUNCE_MAX);
                    Some(Moving::Spring { vel: hit, at: now })
                } else if (vel * unit).abs() < FLING_STOP {
                    None
                } else {
                    Some(Moving::Fling { vel, at: now })
                }
            }
            Some(Moving::Spring { vel, at }) => {
                let (over, vel) = spring(self.over, vel, secs(at, now));
                if over.abs() < 0.5 && vel.abs() < FLING_STOP {
                    self.over = 0.0;
                    None
                } else {
                    self.over = over;
                    Some(Moving::Spring { vel, at: now })
                }
            }
        };
    }
}

/// Seconds from `at` to `now`.
fn secs(at: Instant, now: Instant) -> f64 {
    now.saturating_duration_since(at).as_secs_f64()
}

/// How much of a pull of `px` past an end of a list `view` high shows:
/// all of a small one, less and less of more, never the whole view.
fn band(px: f64, view: f64) -> f64 {
    let view = view.max(1.0);
    px.signum() * (1.0 - 1.0 / (px.abs() * GIVE / view + 1.0)) * view
}

/// The pull that shows as `over` (see `band`).
fn unband(over: f64, view: f64) -> f64 {
    let view = view.max(1.0);
    let shown = (over.abs() / view).min(0.99);
    over.signum() * (1.0 / (1.0 - shown) - 1.0) * view / GIVE
}

/// A critically damped spring at `x` moving at `v`, `t` seconds on: where
/// it is and how fast it moves.
fn spring(x: f64, v: f64, t: f64) -> (f64, f64) {
    let b = v + SPRING * x;
    let e = (-SPRING * t).exp();
    ((x + b * t) * e, (b - SPRING * (x + b * t)) * e)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(max: f64) -> Bounds {
        Bounds {
            max,
            notch: 1.0,
            unit: 40.0,
            next: 40.0,
            prev: 40.0,
            view: 400.0,
        }
    }

    /// Ticks at 120 Hz from `start` for `ms`; the time reached.
    fn run(s: &mut Scroll, start: Instant, ms: u64, b: Bounds) -> Instant {
        let mut now = start;
        for _ in 0..ms * 120 / 1000 {
            now += Duration::from_micros(8333);
            s.tick(now, b);
        }
        now
    }

    /// Fingers moving `px` every 8 ms, `n` times from event time `t0`.
    fn swipe(s: &mut Scroll, start: Instant, t0: u32, px: f64, n: u32, b: Bounds) -> Instant {
        let mut now = start;
        for k in 0..n {
            now = start + Duration::from_millis(8 * k as u64);
            s.apply(
                Motion::Finger {
                    px,
                    time: t0 + 8 * k,
                },
                now,
                b,
            );
        }
        now
    }

    #[test]
    fn a_notch_glides_a_hundred_pixels_and_settles_there() {
        let (mut s, t) = (Scroll::default(), Instant::now());
        s.apply(Motion::Wheel(1.0), t, rows(10.0));
        s.tick(t + Duration::from_millis(16), rows(10.0));
        assert!(s.pos() > 0.5 && s.pos() < 2.5, "{}", s.pos());
        assert!(s.moving(t + Duration::from_millis(16)));
        let end = run(&mut s, t, 300, rows(10.0));
        // Rows 40 high: two and a half of them.
        assert_eq!(s.pos(), 2.5);
        assert!(!s.moving(end));
        // A list scrolled by more a notch goes as far as it says.
        let mut s = Scroll::default();
        let b = Bounds {
            notch: 4.0,
            ..rows(10.0)
        };
        s.apply(Motion::Wheel(1.0), t, b);
        run(&mut s, t, 400, b);
        assert_eq!(s.pos(), 4.0);
    }

    #[test]
    fn notches_turned_fast_add_up_and_go_further() {
        let (mut s, t) = (Scroll::default(), Instant::now());
        let b = rows(100.0);
        let ms = |n| t + Duration::from_millis(n);
        s.apply(Motion::Wheel(1.0), t, b);
        s.apply(Motion::Wheel(1.0), ms(60), b);
        s.apply(Motion::Wheel(1.0), ms(120), b);
        run(&mut s, ms(120), 400, b);
        // 2.5 rows a notch, then 1.3 and 1.69 times as far.
        assert!(
            (s.pos() - 2.5 * (1.0 + 1.3 + 1.69)).abs() < 1e-9,
            "{}",
            s.pos()
        );
        // Turned slowly, each goes a notch's way.
        let mut s = Scroll::default();
        s.apply(Motion::Wheel(1.0), t, b);
        s.apply(Motion::Wheel(1.0), ms(300), b);
        run(&mut s, ms(300), 400, b);
        assert_eq!(s.pos(), 5.0);
        // Not past the end, nor before the start.
        s.apply(Motion::Wheel(20.0), ms(1000), rows(10.0));
        run(&mut s, ms(1000), 400, rows(10.0));
        assert_eq!(s.pos(), 10.0);
        s.apply(Motion::Wheel(-20.0), ms(2000), rows(10.0));
        run(&mut s, ms(2000), 400, rows(10.0));
        assert_eq!(s.pos(), 0.0);
    }

    #[test]
    fn pixels_while_a_notch_glides_go_with_the_glide() {
        let (mut s, t) = (Scroll::default(), Instant::now());
        let b = rows(100.0);
        s.apply(Motion::Wheel(1.0), t, b);
        // A fine-grained wheel's part of a notch, and an event with
        // no move that is no lift.
        s.apply(Motion::Finger { px: 20.0, time: 8 }, t, b);
        s.apply(Motion::Lift { time: 16 }, t, b);
        assert!(s.moving(t));
        run(&mut s, t, 400, b);
        assert_eq!(s.pos(), 3.0);
    }

    #[test]
    fn fingers_move_the_list_one_to_one() {
        let (mut s, t) = (Scroll::default(), Instant::now());
        s.apply(Motion::Finger { px: 10.0, time: 0 }, t, rows(10.0));
        assert_eq!(s.pos(), 0.25);
        assert_eq!((s.first(), s.part()), (0, 0.25));
        s.apply(Motion::Finger { px: 40.0, time: 8 }, t, rows(10.0));
        assert_eq!((s.first(), s.part()), (1, 0.25));
        assert!(s.moving(t));
    }

    #[test]
    fn fingers_crossing_into_a_row_of_another_height_go_by_its_height() {
        let t = Instant::now();
        // A 40 high row between a 20 high label above and an 80 high
        // row under.
        let b = Bounds {
            next: 80.0,
            prev: 20.0,
            ..rows(10.0)
        };
        let mut s = Scroll::at(3.5);
        // 20 pixels to the next row, 40 more half of it.
        s.apply(Motion::Finger { px: 60.0, time: 0 }, t, b);
        assert_eq!(s.pos(), 4.5);
        let mut s = Scroll::at(3.5);
        // 20 pixels to the row's top, 10 more half the label.
        s.apply(Motion::Finger { px: -30.0, time: 0 }, t, b);
        assert_eq!(s.pos(), 2.5);
        // Within the row, by its own height.
        let mut s = Scroll::at(3.5);
        s.apply(Motion::Finger { px: 10.0, time: 0 }, t, b);
        assert_eq!(s.pos(), 3.75);
        // Past an end, the pull is what the move had left.
        let mut s = Scroll::at(9.5);
        s.apply(Motion::Finger { px: 50.0, time: 0 }, t, b);
        assert_eq!(s.pos(), 10.0);
        assert!((b.px(9.5, 10.0) - 20.0).abs() < 1e-9);
        assert!((s.overscroll() + band(30.0, b.view)).abs() < 1e-9);
    }

    #[test]
    fn a_lift_while_moving_flings_the_list_on_and_it_slows_to_a_stop() {
        let (mut s, t) = (Scroll::default(), Instant::now());
        let b = rows(100.0);
        let now = swipe(&mut s, t, 0, 16.0, 10, b);
        let at_lift = s.pos();
        s.apply(Motion::Lift { time: 80 }, now, b);
        assert!(s.moving(now));
        let later = run(&mut s, now, 3000, b);
        // 2000 px/s for a 0.4 s time constant: about 800 px further.
        let flung = (s.pos() - at_lift) * 40.0;
        assert!((700.0..900.0).contains(&flung), "{flung}");
        assert!(!s.moving(later));
    }

    #[test]
    fn fingers_resting_before_the_lift_fling_nothing() {
        let (mut s, t) = (Scroll::default(), Instant::now());
        let now = swipe(&mut s, t, 0, 16.0, 10, rows(100.0));
        let pos = s.pos();
        s.apply(Motion::Lift { time: 72 + 200 }, now, rows(100.0));
        run(&mut s, now, 500, rows(100.0));
        assert_eq!(s.pos(), pos);
    }

    #[test]
    fn a_pull_past_an_end_stretches_less_and_comes_back() {
        let (mut s, t) = (Scroll::default(), Instant::now());
        s.apply(
            Motion::Finger {
                px: -100.0,
                time: 0,
            },
            t,
            rows(10.0),
        );
        assert_eq!(s.pos(), 0.0);
        let over = s.overscroll();
        assert!(over > 0.0 && over < 100.0, "{over}");
        // Pulled back the other way, the list comes out of the stretch first.
        s.apply(Motion::Finger { px: 140.0, time: 8 }, t, rows(10.0));
        assert_eq!((s.overscroll(), s.pos()), (0.0, 1.0));
        s.apply(
            Motion::Finger {
                px: -200.0,
                time: 16,
            },
            t,
            rows(10.0),
        );
        assert!(s.overscroll() > 0.0);
        s.apply(Motion::Lift { time: 24 }, t, rows(10.0));
        let end = run(&mut s, t, 600, rows(10.0));
        assert_eq!((s.overscroll(), s.pos()), (0.0, 0.0));
        assert!(!s.moving(end));
    }

    #[test]
    fn a_fling_into_an_end_bounces_off_it() {
        let (mut s, t) = (Scroll::at(4.0), Instant::now());
        let b = rows(10.0);
        let now = swipe(&mut s, t, 0, -16.0, 6, b);
        s.apply(Motion::Lift { time: 48 }, now, b);
        let mut deepest: f64 = 0.0;
        let mut at = now;
        for _ in 0..120 {
            at += Duration::from_micros(8333);
            s.tick(at, b);
            deepest = deepest.max(s.overscroll());
        }
        assert_eq!(s.pos(), 0.0);
        assert!(deepest > 5.0, "{deepest}");
        assert_eq!(s.overscroll(), 0.0);
        assert!(!s.moving(at));
    }

    #[test]
    fn a_list_that_fits_neither_moves_nor_stretches() {
        let (mut s, t) = (Scroll::default(), Instant::now());
        s.apply(Motion::Finger { px: 50.0, time: 0 }, t, rows(0.0));
        s.apply(Motion::Notches(2.0), t, rows(0.0));
        assert_eq!((s.pos(), s.overscroll()), (0.0, 0.0));
    }

    #[test]
    fn fingers_that_never_lift_let_go_of_a_stretch() {
        let (mut s, t) = (Scroll::default(), Instant::now());
        s.apply(Motion::Finger { px: -60.0, time: 0 }, t, rows(10.0));
        assert!(s.overscroll() > 0.0);
        let end = run(&mut s, t, 800, rows(10.0));
        assert_eq!(s.overscroll(), 0.0);
        assert!(!s.moving(end));
    }

    #[test]
    fn a_jump_stops_the_list_and_a_jump_to_its_row_keeps_it() {
        let (mut s, t) = (Scroll::default(), Instant::now());
        s.apply(Motion::Finger { px: 50.0, time: 0 }, t, rows(10.0));
        s.jump_row(1);
        assert_eq!(s.pos(), 1.25);
        s.jump_row(3);
        assert_eq!(s.pos(), 3.0);
        assert!(!s.moving(t));
    }

    #[test]
    fn a_glide_to_a_place_gets_there() {
        let (mut s, t) = (Scroll::default(), Instant::now());
        s.glide_to(4.0, t, rows(10.0));
        assert!(s.moving(t));
        run(&mut s, t, 400, rows(10.0));
        assert_eq!(s.pos(), 4.0);
    }

    #[test]
    fn a_fling_stops_where_the_list_stands() {
        let (mut s, t) = (Scroll::default(), Instant::now());
        let b = rows(100.0);
        let now = swipe(&mut s, t, 0, 16.0, 10, b);
        s.apply(Motion::Lift { time: 80 }, now, b);
        let at = run(&mut s, now, 100, b);
        let pos = s.pos();
        assert!(s.stop_fling());
        assert!(!s.moving(at));
        run(&mut s, at, 500, b);
        assert_eq!(s.pos(), pos);
        // Nothing flung, nothing stopped: a glide goes on.
        assert!(!s.stop_fling());
        s.apply(Motion::Notches(1.0), at, b);
        assert!(!s.stop_fling());
        assert!(s.moving(at));
    }

    #[test]
    fn a_shrunk_list_holds_the_position_within_it() {
        let (mut s, t) = (Scroll::at(8.0), Instant::now());
        s.tick(t, rows(5.0));
        assert_eq!(s.pos(), 5.0);
    }

    #[test]
    fn the_band_gives_and_takes_back_alike() {
        for px in [-300.0, -20.0, 1.0, 80.0, 500.0] {
            let over = band(px, 400.0);
            assert!(over.abs() < px.abs() && over.abs() < 400.0);
            assert!((unband(over, 400.0) - px).abs() < 1e-6, "{px}");
        }
    }
}

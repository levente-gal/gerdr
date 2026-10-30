//! The icons, as signed distance fields painted at any scale.

use super::{Canvas, Rect, Rgb};
use crate::ci::CiState;
use crate::herdr::AgentStatus;

/// Signed distance from (x, y) to a rounded rectangle: negative inside.
pub(super) fn rounded_rect(x: f32, y: f32, left: f32, top: f32, w: f32, h: f32, r: f32) -> f32 {
    let (hw, hh) = (w / 2.0, h / 2.0);
    let qx = (x - left - hw).abs() - (hw - r);
    let qy = (y - top - hh).abs() - (hh - r);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

/// Signed distance to a sun icon centred on the origin, `s` pixels per logical pixel.
pub(super) fn sun(x: f32, y: f32, s: f32) -> f32 {
    let disc = x.hypot(y) - 3.5 * s;
    let rays = (0..8)
        .map(|i| {
            let (sin, cos) = (i as f32 * std::f32::consts::FRAC_PI_4).sin_cos();
            segment(
                x,
                y,
                cos * 5.8 * s,
                sin * 5.8 * s,
                cos * 7.6 * s,
                sin * 7.6 * s,
            ) - 0.8 * s
        })
        .fold(f32::MAX, f32::min);
    disc.min(rays)
}

/// Signed distance to a crescent moon icon centred on the origin.
pub(super) fn moon(x: f32, y: f32, s: f32) -> f32 {
    let disc = x.hypot(y) - 6.5 * s;
    let bite = (x - 3.6 * s).hypot(y + 3.0 * s) - 5.2 * s;
    disc.max(-bite)
}

/// Half the stroke width of the line icons below, in logical pixels.
pub(super) const STROKE: f32 = 0.8;

/// Distance to a "+" of arm `arm` centred on (cx, cy), stroked.
pub(super) fn plus(x: f32, y: f32, cx: f32, cy: f32, arm: f32, s: f32) -> f32 {
    let h = segment(x, y, cx - arm, cy, cx + arm, cy);
    let v = segment(x, y, cx, cy - arm, cx, cy + arm);
    h.min(v) - STROKE * s
}

/// The tab strip's "+": larger and a little bolder than the badge pluses.
pub(super) fn new_tab(x: f32, y: f32, s: f32) -> f32 {
    plus(x, y, 0.0, 0.0, 6.0 * s, s) - 0.2 * s
}

/// Signed distance to a folder outline centred on the origin: a body with a
/// tab on its top left.
pub(super) fn folder(x: f32, y: f32, s: f32) -> f32 {
    let body = rounded_rect(x, y, -8.0 * s, -5.0 * s, 16.0 * s, 12.0 * s, 1.5 * s);
    let tab = rounded_rect(x, y, -8.0 * s, -7.0 * s, 7.0 * s, 4.0 * s, 1.5 * s);
    body.min(tab).abs() - STROKE * s
}

/// Signed distance to a window outline with a bar across its top: a tab.
pub(super) fn tab(x: f32, y: f32, s: f32) -> f32 {
    let frame =
        rounded_rect(x, y, -8.0 * s, -6.5 * s, 16.0 * s, 13.0 * s, 2.0 * s).abs() - STROKE * s;
    frame.min(segment(x, y, -8.0 * s, -2.5 * s, 8.0 * s, -2.5 * s) - STROKE * s)
}

/// A folder with a "+" inside: add a project.
pub(super) fn folder_plus(x: f32, y: f32, s: f32) -> f32 {
    folder(x, y, s).min(plus(x, y, 0.0, 1.0 * s, 2.8 * s, s))
}

/// Signed distance to a window outline with a "play" triangle inside:
/// launch an app.
pub(super) fn app(x: f32, y: f32, s: f32) -> f32 {
    let frame =
        rounded_rect(x, y, -8.0 * s, -6.5 * s, 16.0 * s, 13.0 * s, 2.0 * s).abs() - STROKE * s;
    let (a, b, c) = ((-2.0 * s, -3.5 * s), (-2.0 * s, 3.5 * s), (3.5 * s, 0.0));
    let play = segment(x, y, a.0, a.1, b.0, b.1)
        .min(segment(x, y, b.0, b.1, c.0, c.1))
        .min(segment(x, y, c.0, c.1, a.0, a.1))
        - STROKE * s;
    frame.min(play)
}

/// The ">" prompt of the terminal icons.
pub(super) fn prompt(x: f32, y: f32, s: f32) -> f32 {
    segment(x, y, -4.5 * s, -2.5 * s, -1.5 * s, 0.0).min(segment(
        x,
        y,
        -1.5 * s,
        0.0,
        -4.5 * s,
        2.5 * s,
    )) - STROKE * s
}

/// Signed distance to a terminal window outline with a prompt and cursor,
/// centred on the origin.
pub(super) fn terminal(x: f32, y: f32, s: f32) -> f32 {
    let frame =
        rounded_rect(x, y, -8.0 * s, -7.0 * s, 16.0 * s, 14.0 * s, 2.0 * s).abs() - STROKE * s;
    let cursor = segment(x, y, 0.0, 2.5 * s, 3.0 * s, 2.5 * s) - STROKE * s;
    frame.min(prompt(x, y, s)).min(cursor)
}

/// Signed distance to the mark for an agent's status, centred on the origin:
/// an open ring spinning while it works, a dot when it wants attention
/// (blocked, done), a ring while it waits.
pub(super) fn status_mark(status: AgentStatus, x: f32, y: f32, s: f32) -> f32 {
    let ring = (x.hypot(y) - 3.6 * s).abs() - 1.1 * s;
    match status {
        // The ring without its top right quarter.
        AgentStatus::Working => ring.max(x.min(-y)),
        AgentStatus::Blocked | AgentStatus::Done => x.hypot(y) - 4.2 * s,
        AgentStatus::Idle => ring,
        AgentStatus::Unknown => (x.hypot(y) - 2.6 * s).abs() - 0.9 * s,
    }
}

/// Signed distance to a robot's head centred on the origin, about 12
/// logical pixels across: a rounded head with two eyes and an antenna. An
/// agent without a logo of its own.
pub(super) fn robot(x: f32, y: f32, s: f32) -> f32 {
    let head =
        rounded_rect(x, y, -5.5 * s, -3.0 * s, 11.0 * s, 8.5 * s, 2.5 * s).abs() - STROKE * s;
    let eyes = (x.abs() - 2.3 * s).hypot(y - 1.2 * s) - 1.1 * s;
    let antenna = (segment(x, y, 0.0, -3.0 * s, 0.0, -5.0 * s) - STROKE * s)
        .min(x.hypot(y + 5.6 * s) - 1.1 * s);
    head.min(eyes).min(antenna)
}

/// Signed distance to a flask centred on the origin, about 12 logical
/// pixels across: a neck under a rim, widening to a flat bottom, filled
/// with liquid up to its shoulders. A test run.
pub(super) fn flask(x: f32, y: f32, s: f32) -> f32 {
    let line = |ax: f32, ay: f32, bx: f32, by: f32| segment(x, y, ax * s, ay * s, bx * s, by * s);
    let outline = line(-3.2, -6.0, 3.2, -6.0)
        .min(line(-2.0, -6.0, -2.0, -1.5))
        .min(line(2.0, -6.0, 2.0, -1.5))
        .min(line(-2.0, -1.5, -5.6, 5.0))
        .min(line(2.0, -1.5, 5.6, 5.0))
        .min(line(-5.6, 5.0, 5.6, 5.0))
        - STROKE * s;
    // The body's sides, as far from the middle as y is down the slope.
    let side = |y: f32| (2.0 + (y / s + 1.5) * 3.6 / 6.5) * s;
    let liquid = (x.abs() - side(y)).max(1.5 * s - y).max(y - 5.0 * s);
    outline.min(liquid)
}

/// Signed distance to a Git branch icon centred on the origin: a trunk
/// between two rings, and a ring at the top right forking off it.
pub(super) fn git_branch(x: f32, y: f32, s: f32) -> f32 {
    let ring = |cx: f32, cy: f32| ((x - cx * s).hypot(y - cy * s) - 1.8 * s).abs() - STROKE * s;
    let trunk = segment(x, y, -3.0 * s, -2.2 * s, -3.0 * s, 2.2 * s) - STROKE * s;
    let fork = segment(x, y, 3.0 * s, -2.2 * s, 3.0 * s, -0.5 * s).min(segment(
        x,
        y,
        3.0 * s,
        -0.5 * s,
        -3.0 * s,
        2.0 * s,
    )) - STROKE * s;
    ring(-3.0, -4.0)
        .min(ring(-3.0, 4.0))
        .min(ring(3.0, -4.0))
        .min(trunk)
        .min(fork)
}

/// Signed distance to a small arrow centred on the origin, pointing up or down.
pub(super) fn arrow(up: bool, x: f32, y: f32, s: f32) -> f32 {
    let y = if up { y } else { -y };
    let shaft = segment(x, y, 0.0, -3.5 * s, 0.0, 3.5 * s);
    let head =
        segment(x, y, -2.5 * s, -s, 0.0, -3.5 * s).min(segment(x, y, 2.5 * s, -s, 0.0, -3.5 * s));
    shaft.min(head) - STROKE * s
}

/// Signed distance to a check mark centred on the origin.
pub(super) fn check(x: f32, y: f32, s: f32) -> f32 {
    let short = segment(x, y, -3.5 * s, 0.0, -s, 2.5 * s);
    let long = segment(x, y, -s, 2.5 * s, 3.8 * s, -3.0 * s);
    short.min(long) - 1.1 * s
}

/// Signed distance to a ring struck through, centred on the origin: what
/// runs in a tab won't run again after a restart.
pub(super) fn no_restart(x: f32, y: f32, s: f32) -> f32 {
    let ring = (x.hypot(y) - 3.6 * s).abs() - STROKE * s;
    let slash = segment(x, y, -2.6 * s, 2.6 * s, 2.6 * s, -2.6 * s) - STROKE * s;
    ring.min(slash)
}

/// Signed distance to a small down-pointing chevron centred on the origin.
pub(super) fn chevron_down(x: f32, y: f32, s: f32) -> f32 {
    let left = segment(x, y, -3.0 * s, -1.5 * s, 0.0, 1.5 * s);
    let right = segment(x, y, 0.0, 1.5 * s, 3.0 * s, -1.5 * s);
    left.min(right) - 0.75 * s
}

/// Signed distance to a chevron centred on the origin, pointing left for
/// back, else right for forward.
pub(super) fn chevron_side(left: bool, x: f32, y: f32, s: f32) -> f32 {
    let x = if left { x } else { -x };
    let top = segment(x, y, 1.8 * s, -3.8 * s, -1.8 * s, 0.0);
    let bottom = segment(x, y, -1.8 * s, 0.0, 1.8 * s, 3.8 * s);
    top.min(bottom) - STROKE * s
}

/// Signed distance to lines of text centred on the origin, the middle one
/// cut short by a text cursor: the scrollback, to select in.
pub(super) fn text_lines(x: f32, y: f32, s: f32) -> f32 {
    let line = |y0: f32, x1: f32| segment(x, y, -5.5 * s, y0 * s, x1 * s, y0 * s) - STROKE * s;
    let cursor = segment(x, y, 3.5 * s, -2.0 * s, 3.5 * s, 2.0 * s) - STROKE * s;
    line(-4.5, 5.5)
        .min(line(0.0, 1.0))
        .min(line(4.5, 3.0))
        .min(cursor)
}

/// Signed distance to two pages, one over the other's top left, centred
/// on the origin: copy.
pub(super) fn copy(x: f32, y: f32, s: f32) -> f32 {
    let front = rounded_rect(x, y, -1.6 * s, -1.6 * s, 6.0 * s, 6.0 * s, 1.2 * s);
    let back = rounded_rect(x, y, -4.4 * s, -4.4 * s, 6.0 * s, 6.0 * s, 1.2 * s);
    // The back page shows only where the front one doesn't cover it.
    let back = (back.abs() - STROKE * s).max(-(front - STROKE * s));
    (front.abs() - STROKE * s).min(back)
}

/// Signed distance to a magnifying glass centred on the origin: a ring and
/// its handle to the bottom right.
pub(super) fn magnifier(x: f32, y: f32, s: f32) -> f32 {
    let ring = ((x + 1.0 * s).hypot(y + 1.0 * s) - 4.5 * s).abs() - STROKE * s;
    let handle = segment(x, y, 2.4 * s, 2.4 * s, 5.5 * s, 5.5 * s) - 1.1 * s;
    ring.min(handle)
}

/// Signed distance to a pencil centred on the origin, pointing to the
/// bottom left: an outlined body and its pointed tip.
pub(super) fn pencil(x: f32, y: f32, s: f32) -> f32 {
    // Along the pencil (towards the top right) and across it.
    let (along, across) = (
        (x - y) * std::f32::consts::FRAC_1_SQRT_2,
        (x + y) * std::f32::consts::FRAC_1_SQRT_2,
    );
    let body = rounded_rect(along, across, -2.5 * s, -2.2 * s, 8.5 * s, 4.4 * s, 0.6 * s).abs()
        - STROKE * s;
    let tip = segment(along, across, -2.5 * s, -2.2 * s, -6.2 * s, 0.0).min(segment(
        along,
        across,
        -2.5 * s,
        2.2 * s,
        -6.2 * s,
        0.0,
    )) - STROKE * s;
    body.min(tip)
}

/// Signed distance to a prompt, ">_", centred on the origin: a project's
/// commands.
pub(super) fn commands_prompt(x: f32, y: f32, s: f32) -> f32 {
    let chevron = segment(x, y, -4.2 * s, -3.6 * s, -0.6 * s, 0.0).min(segment(
        x,
        y,
        -0.6 * s,
        0.0,
        -4.2 * s,
        3.6 * s,
    ));
    let line = segment(x, y, 1.0 * s, 3.6 * s, 4.6 * s, 3.6 * s);
    chevron.min(line) - STROKE * s
}

/// Signed distance to a page outline centred on the origin, its top right
/// corner folded over: a file.
pub(super) fn file(x: f32, y: f32, s: f32) -> f32 {
    let (left, right, top, bottom) = (-3.4 * s, 3.4 * s, -4.6 * s, 4.6 * s);
    let (fold_x, fold_y) = (0.8 * s, -2.0 * s);
    let edges = [
        (left, top, fold_x, top),
        (fold_x, top, right, fold_y),
        (right, fold_y, right, bottom),
        (right, bottom, left, bottom),
        (left, bottom, left, top),
        // The fold.
        (fold_x, top, fold_x, fold_y),
        (fold_x, fold_y, right, fold_y),
    ];
    edges
        .into_iter()
        .map(|(ax, ay, bx, by)| segment(x, y, ax, ay, bx, by))
        .fold(f32::MAX, f32::min)
        - STROKE * s
}

/// Signed distance to four corner brackets centred on the origin: at the
/// corners of a square pointing out for zoom, or near its middle pointing
/// in for unzoom.
pub(super) fn zoom_corners(unzoom: bool, x: f32, y: f32, s: f32) -> f32 {
    let (corner, arm) = if unzoom { (2.2, 2.4) } else { (4.4, -2.8) };
    let mut d = f32::MAX;
    for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        let (cx, cy) = (sx * corner * s, sy * corner * s);
        let across = segment(x, y, cx, cy, cx + sx * arm * s, cy);
        let down = segment(x, y, cx, cy, cx, cy + sy * arm * s);
        d = d.min(across).min(down);
    }
    d - STROKE * s
}

/// Signed distance to a filled square centred on the origin: stop.
pub(super) fn stop(x: f32, y: f32, s: f32) -> f32 {
    rounded_rect(x, y, -3.8 * s, -3.8 * s, 7.6 * s, 7.6 * s, 1.2 * s)
}

/// Signed distance to a circling arrow centred on the origin: run again. A
/// ring open at its top right, its top end an arrow head pointing into the
/// gap.
pub(super) fn restart_arrow(x: f32, y: f32, s: f32) -> f32 {
    let r = 4.2 * s;
    let ring = ((x.hypot(y) - r).abs() - STROKE * s).max(x.min(-y));
    let (tip_x, tip_y) = (1.4 * s, -r);
    let head = segment(x, y, tip_x, tip_y, tip_x - 2.4 * s, tip_y - 2.4 * s).min(segment(
        x,
        y,
        tip_x,
        tip_y,
        tip_x - 2.4 * s,
        tip_y + 2.4 * s,
    )) - STROKE * s;
    ring.min(head)
}

/// Signed distance to a sparkle centred on the origin, about 12 logical
/// pixels across: a four-pointed star with a small one at its upper
/// right. The AI agents.
pub(super) fn sparkle(x: f32, y: f32, s: f32) -> f32 {
    // A star of four points: two rhombi crossed, each arm `long` from its
    // centre and `wide` across at it.
    let star = |cx: f32, cy: f32, long: f32, wide: f32| {
        let rhombus = |x: f32, y: f32, a: f32, b: f32| {
            let (qx, qy) = (x.abs(), y.abs());
            let h =
                ((-2.0 * (qx * a - qy * b) + (a * a - b * b)) / (a * a + b * b)).clamp(-1.0, 1.0);
            let d = (qx - 0.5 * a * (1.0 - h)).hypot(qy - 0.5 * b * (1.0 + h));
            d * (qx * b + qy * a - a * b).signum()
        };
        let (x, y) = (x - cx, y - cy);
        rhombus(x, y, wide, long).min(rhombus(x, y, long, wide))
    };
    star(-s, s, 4.8 * s, 1.5 * s).min(star(3.6 * s, -3.6 * s, 2.2 * s, 0.75 * s))
}

/// Signed distance to an ellipsis centred on the origin: three dots in a
/// row. More of them, elsewhere.
pub(super) fn ellipsis(x: f32, y: f32, s: f32) -> f32 {
    let dot = |cx: f32| (x - cx * s).hypot(y) - 1.2 * s;
    dot(-3.6).min(dot(0.0)).min(dot(3.6))
}

/// Signed distance to a "×" centred on the origin: forget.
pub(super) fn cross(x: f32, y: f32, s: f32) -> f32 {
    let arm = 3.8 * s;
    segment(x, y, -arm, -arm, arm, arm).min(segment(x, y, -arm, arm, arm, -arm)) - STROKE * s
}

/// Signed distance to a trash can centred on the origin: delete. A lid
/// with its handle over a body with two ribs.
pub(super) fn trash(x: f32, y: f32, s: f32) -> f32 {
    let lid = segment(x, y, -4.6 * s, -3.6 * s, 4.6 * s, -3.6 * s);
    let handle = segment(x, y, -1.6 * s, -5.4 * s, 1.6 * s, -5.4 * s)
        .min(segment(x, y, -1.6 * s, -5.4 * s, -1.6 * s, -3.6 * s))
        .min(segment(x, y, 1.6 * s, -5.4 * s, 1.6 * s, -3.6 * s));
    let body = rounded_rect(x, y, -3.6 * s, -3.6 * s, 7.2 * s, 9.2 * s, 1.2 * s).abs();
    let rib = |rx: f32| segment(x, y, rx, -1.2 * s, rx, 3.2 * s);
    let ribs = rib(-1.2 * s).min(rib(1.2 * s));
    lid.min(handle).min(body).min(ribs) - STROKE * 0.75 * s
}

/// Signed distance to the mark for where a CI run stands, centred on the
/// origin: a check when it passed, a "×" when it failed, the working
/// agent's open ring while it runs, a small ring when nothing is known.
pub(super) fn ci_mark(state: CiState, x: f32, y: f32, s: f32) -> f32 {
    match state {
        CiState::Passed => check(x, y, 0.8 * s),
        CiState::Failed => cross(x, y, 0.75 * s) - 0.3 * s,
        CiState::Running => status_mark(AgentStatus::Working, x, y, 0.85 * s),
        CiState::Unknown => status_mark(AgentStatus::Unknown, x, y, s),
    }
}

/// Signed distance to an inbox tray centred on the origin: a box whose
/// floor dips into a slot in the middle.
pub(super) fn tray(x: f32, y: f32, s: f32) -> f32 {
    let outline = rounded_rect(x, y, -7.0 * s, -6.0 * s, 14.0 * s, 12.0 * s, 2.0 * s).abs();
    let floor = [
        (-7.0, 1.0, -3.0, 1.0),
        (-3.0, 1.0, -2.0, 3.0),
        (-2.0, 3.0, 2.0, 3.0),
        (2.0, 3.0, 3.0, 1.0),
        (3.0, 1.0, 7.0, 1.0),
    ]
    .into_iter()
    .map(|(ax, ay, bx, by)| segment(x, y, ax * s, ay * s, bx * s, by * s))
    .fold(f32::MAX, f32::min);
    outline.min(floor) - STROKE * s
}

/// Signed distance to an eye centred on the origin: an almond outline,
/// where two circles overlap, around a round pupil. Watch.
pub(super) fn eye(x: f32, y: f32, s: f32) -> f32 {
    // Circles through the almond's corners at (±7, 0) and its top and
    // bottom at (0, ±4.2).
    let (half_w, half_h) = (7.0 * s, 4.2 * s);
    let c = (half_w * half_w - half_h * half_h) / (2.0 * half_h);
    let r = c + half_h;
    let almond = (x.hypot(y - c) - r).max(x.hypot(y + c) - r);
    let pupil = x.hypot(y) - 2.2 * s;
    (almond.abs() - STROKE * s).min(pupil)
}

/// Signed distance to a spinner centred on the origin: the working
/// agent's open ring, turned `turn` of a full circle clockwise.
pub(super) fn spinner(turn: f32, x: f32, y: f32, s: f32) -> f32 {
    let (sin, cos) = (turn * std::f32::consts::TAU).sin_cos();
    let (rx, ry) = (x * cos + y * sin, y * cos - x * sin);
    status_mark(AgentStatus::Working, rx, ry, s)
}

/// Paint the signed-distance `shape`, centred on a logical rectangle.
pub(super) fn icon_sdf(
    c: &mut Canvas,
    rect: Rect,
    scale: f64,
    color: Rgb,
    shape: impl Fn(f32, f32) -> f32,
) {
    let px = |v: i32| (v as f64 * scale).round() as i32;
    let (x, y, w, h) = (
        px(rect.loc.x),
        px(rect.loc.y),
        px(rect.size.w),
        px(rect.size.h),
    );
    let (cx, cy) = (x as f32 + w as f32 / 2.0, y as f32 + h as f32 / 2.0);
    c.paint_in(x, y, w, h, color, |px, py| {
        coverage(shape(px - cx, py - cy))
    });
}

/// Distance from (x, y) to the segment from (ax, ay) to (bx, by).
pub(super) fn segment(x: f32, y: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let (px, py, dx, dy) = (x - ax, y - ay, bx - ax, by - ay);
    let t = ((px * dx + py * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    (px - dx * t).hypot(py - dy * t)
}

/// Anti-aliased pixel coverage for a signed distance in pixels.
pub(super) fn coverage(d: f32) -> f32 {
    (0.5 - d).clamp(0.0, 1.0)
}

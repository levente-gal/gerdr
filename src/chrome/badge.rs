//! What an agent's mark is, wherever it shows: its icon (see
//! `agent_icons`) at the start of its row and, in a column at the row's
//! right end, a dot for where it stands. The dot of something at work
//! blinks: it is left out of the cached buffers and composed over them on
//! its own (`blink_dot`, see `Chrome::elements`).

use super::{Canvas, Palette, Rect, Rgb, Standing, flask, icon_sdf, physical, robot};
use crate::agent_icons::AgentIcon;
use crate::ci::CiState;
use crate::herdr::AgentStatus;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Rectangle, Size};

/// The dot's diameter, logical pixels.
pub const DOT: i32 = 7;
/// The column the dot takes at the right end of a row, logical pixels.
pub const DOT_SLOT: i32 = 14;

/// Where an agent, a test run or a CI run stands, as its dot shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dot {
    /// At work, running: blinks.
    Working,
    Blocked,
    /// Done with unseen work, passed.
    Done,
    /// Waiting for a prompt: a ring.
    Idle,
    Unknown,
    Failed,
}

impl Dot {
    pub fn of_status(status: AgentStatus) -> Self {
        match status {
            AgentStatus::Working => Self::Working,
            AgentStatus::Blocked => Self::Blocked,
            AgentStatus::Done => Self::Done,
            AgentStatus::Idle => Self::Idle,
            AgentStatus::Unknown => Self::Unknown,
        }
    }

    pub fn of_ci(state: CiState) -> Self {
        match state {
            CiState::Running => Self::Working,
            CiState::Passed => Self::Done,
            CiState::Failed => Self::Failed,
            CiState::Unknown => Self::Unknown,
        }
    }

    /// Whether it blinks, and so is composed rather than painted.
    pub fn blinks(self) -> bool {
        self == Self::Working
    }

    pub(super) fn color(self, p: &Palette) -> Rgb {
        match self {
            Self::Working => Standing::Working.color(p),
            Self::Blocked => Standing::Yours.color(p),
            Self::Done => Standing::Done.color(p),
            Self::Idle => Standing::Idle.color(p),
            Self::Unknown => p.placeholder,
            Self::Failed => Standing::Failed.color(p),
        }
    }
}

/// An agent's mark: its icon and its dot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Badge {
    pub icon: AgentIcon,
    pub dot: Dot,
}

/// The square an icon `size` big takes, centred on `rect`.
fn icon_box(rect: Rect, size: i32) -> Rect {
    Rectangle::new(
        (
            rect.loc.x + (rect.size.w - size) / 2,
            rect.loc.y + (rect.size.h - size) / 2,
        )
            .into(),
        (size, size).into(),
    )
}

/// The dot's column at the right end of `line`, as high as it.
pub fn dot_slot(line: Rect) -> Rect {
    Rectangle::new(
        (line.loc.x + line.size.w - DOT_SLOT, line.loc.y).into(),
        (DOT_SLOT, line.size.h).into(),
    )
}

/// Where the dot of a row whose first line is `line` goes: centred in its
/// column at the line's right end. Whole logical pixels, so that the
/// painted row and the composed dot agree.
pub fn row_dot(line: Rect) -> Rect {
    centred_dot(dot_slot(line))
}

/// Where the blinking dot of a running mark centred on `mark` goes, as
/// a CI chip's: a `DOT` square at its middle.
pub fn centred_dot(mark: Rect) -> Rect {
    Rectangle::new(
        (
            mark.loc.x + (mark.size.w - DOT) / 2,
            mark.loc.y + (mark.size.h - DOT) / 2,
        )
            .into(),
        (DOT, DOT).into(),
    )
}

/// Signed distance to the dot `d` physical pixels across, centred on the
/// origin: a disc, or a ring for `Dot::Idle`.
pub(super) fn dot_shape(dot: Dot, x: f32, y: f32, d: f32) -> f32 {
    let r = d / 2.0;
    match dot {
        Dot::Idle => (x.hypot(y) - 0.75 * r).abs() - 0.25 * r,
        _ => x.hypot(y) - r,
    }
}

/// `icon` `size` big, centred on `rect`: a logo or a drawn icon, in
/// `tint`.
pub(super) fn agent_icon(
    c: &mut Canvas,
    rect: Rect,
    size: i32,
    icon: AgentIcon,
    tint: Rgb,
    scale: f64,
) {
    let s = scale as f32;
    let at = icon_box(rect, size);
    match (icon, icon.picture()) {
        (_, Some(picture)) => c.picture_sized(picture, at, scale, tint, size),
        (AgentIcon::Test, None) => {
            let k = size as f32 / 14.0 * s;
            icon_sdf(c, at, scale, tint, |x, y| flask(x, y, k));
        }
        _ => {
            let k = size as f32 / 14.0 * s;
            icon_sdf(c, at, scale, tint, |x, y| robot(x, y, k));
        }
    }
}

/// `dot` in `spot` (`DOT` square, see `row_dot`), unless it blinks.
pub(super) fn status_dot(c: &mut Canvas, spot: Rect, dot: Dot, scale: f64, p: &Palette) {
    if dot.blinks() {
        return;
    }
    let d = DOT as f32 * scale as f32;
    icon_sdf(c, spot, scale, dot.color(p), |x, y| dot_shape(dot, x, y, d));
}

/// A blinking dot on its own, `DOT` across on a clear buffer: composed
/// over a badge painted without it.
pub(super) fn blink_dot(dot: Dot, scale: f64, p: &Palette) -> MemoryRenderBuffer {
    dot_canvas(dot, scale, p).into_buffer(false)
}

fn dot_canvas(dot: Dot, scale: f64, p: &Palette) -> Canvas {
    let size = Size::from((DOT, DOT));
    let mut c = Canvas::new(physical(size, scale));
    let d = DOT as f32 * scale as f32;
    icon_sdf(
        &mut c,
        Rectangle::from_size(size),
        scale,
        dot.color(p),
        |x, y| dot_shape(dot, x, y, d),
    );
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chrome::Theme;

    const GROUND: Rgb = [40, 44, 52];

    /// `dot` drawn at scale 1 on `GROUND`, and its rect.
    fn drawn(dot: Dot) -> (Canvas, Rect) {
        let p = Theme::Dark.palette();
        let mut c = Canvas::new((24, 24).into());
        c.fill(0, 0, 24, 24, GROUND);
        let spot = row_dot(Rectangle::new((0, 0).into(), (24, 24).into()));
        status_dot(&mut c, spot, dot, 1.0, p);
        (c, spot)
    }

    fn rgb(c: &Canvas, x: i32, y: i32) -> Rgb {
        let i = ((y * c.w + x) * 4) as usize;
        [c.px[i + 2], c.px[i + 1], c.px[i]]
    }

    fn centre(rect: Rect) -> (i32, i32) {
        (rect.loc.x + rect.size.w / 2, rect.loc.y + rect.size.h / 2)
    }

    #[test]
    fn the_dot_sits_in_the_column_at_the_lines_end() {
        let line = Rectangle::new((10, 20).into(), (100, 24).into());
        let dot = row_dot(line);
        // The column spans 96..110 across.
        assert_eq!(
            dot.loc,
            (96 + (DOT_SLOT - DOT) / 2, 20 + (24 - DOT) / 2).into()
        );
        assert_eq!(dot.size, (DOT, DOT).into());
    }

    #[test]
    fn a_dot_in_the_states_colour_unless_it_blinks() {
        let p = Theme::Dark.palette();
        for (dot, color) in [
            (Dot::Done, p.success),
            (Dot::Blocked, p.warning),
            (Dot::Failed, p.error),
        ] {
            let (c, spot) = drawn(dot);
            let (x, y) = centre(spot);
            assert_eq!(rgb(&c, x, y), color, "{dot:?}");
        }
        // A blinking one leaves its place empty, for the composed dot.
        let (c, spot) = drawn(Dot::Working);
        let (x, y) = centre(spot);
        assert_eq!(rgb(&c, x, y), GROUND);
    }

    #[test]
    fn an_idle_dot_is_a_ring() {
        let p = Theme::Dark.palette();
        let (c, spot) = drawn(Dot::Idle);
        let (x, y) = centre(spot);
        let hole = rgb(&c, x, y);
        assert!(
            hole.iter().zip(GROUND).all(|(a, b)| a.abs_diff(b) < 8),
            "{hole:?}"
        );
        let edge = rgb(&c, spot.loc.x, y);
        assert_ne!(edge, GROUND);
        assert!(edge.iter().zip(p.muted).all(|(a, b)| a.abs_diff(b) < 40));
    }

    #[test]
    fn a_test_run_is_a_flask_and_an_agent_without_a_logo_a_robot() {
        for icon in [AgentIcon::Test, AgentIcon::Generic] {
            let p = Theme::Dark.palette();
            let mut c = Canvas::new((24, 24).into());
            c.fill(0, 0, 24, 24, GROUND);
            let rect = Rectangle::new((4, 4).into(), (16, 16).into());
            agent_icon(&mut c, rect, 16, icon, p.text, 1.0);
            // Something of it shows in the icon's middle column.
            let lit = (4..20).filter(|y| rgb(&c, 12, *y) != GROUND).count();
            assert!(lit >= 3, "{icon:?}");
        }
    }

    #[test]
    fn the_blinking_dot_is_a_disc_on_nothing() {
        let p = Theme::Dark.palette();
        let c = dot_canvas(Dot::Working, 2.0, p);
        assert_eq!((c.w, c.h), (2 * DOT, 2 * DOT));
        let alpha = |x: i32, y: i32| c.px[((y * c.w + x) * 4 + 3) as usize];
        assert_eq!(alpha(DOT, DOT), 255);
        assert_eq!(alpha(0, 0), 0);
        assert_eq!(rgb(&c, DOT, DOT), p.accent);
    }
}

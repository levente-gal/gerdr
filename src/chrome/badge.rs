//! What an agent's mark is, wherever it shows: its icon (see
//! `agent_icons`) and, on the icon's bottom right corner, a dot for where
//! it stands. The dot of something at work blinks: it is left out of the
//! cached buffers and composed over them on its own (`blink_dot`, see
//! `Chrome::elements`).

use super::{Canvas, Palette, Rect, Rgb, flask, icon_sdf, physical, robot};
use crate::agent_icons::AgentIcon;
use crate::ci::CiState;
use crate::herdr::AgentStatus;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Rectangle, Size};

/// The dot's diameter, logical pixels.
pub const DOT: i32 = 6;
/// How far round the dot the icon is cut away, logical pixels.
const DOT_RING: f32 = 1.5;
/// How far the dot sticks out past the icon's corner, logical pixels.
const DOT_OUT: i32 = 1;

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
            Self::Working => p.accent,
            Self::Blocked => p.warning,
            Self::Done => p.success,
            Self::Idle => p.muted,
            Self::Unknown => p.placeholder,
            Self::Failed => p.error,
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

/// Where the dot of an icon `size` big centred on `rect` goes: on its
/// bottom right corner, a little past it. Whole logical pixels, so that
/// the painted icon and the composed dot agree.
pub fn dot_rect(rect: Rect, size: i32) -> Rect {
    let icon = icon_box(rect, size);
    Rectangle::new(
        (
            icon.loc.x + size - DOT + DOT_OUT,
            icon.loc.y + size - DOT + DOT_OUT,
        )
            .into(),
        (DOT, DOT).into(),
    )
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
fn dot_shape(dot: Dot, x: f32, y: f32, d: f32) -> f32 {
    let r = d / 2.0;
    match dot {
        Dot::Idle => (x.hypot(y) - 0.75 * r).abs() - 0.25 * r,
        _ => x.hypot(y) - r,
    }
}

/// `badge` `size` big, centred on `rect`: its icon (a logo in its own
/// colours, its text-coloured part and a drawn icon in `tint`), the
/// corner cut away round the dot in `ground` (what it is drawn on), then
/// the dot, unless it blinks.
#[allow(clippy::too_many_arguments)]
pub(super) fn agent_badge(
    c: &mut Canvas,
    rect: Rect,
    size: i32,
    badge: Badge,
    tint: Rgb,
    ground: Rgb,
    scale: f64,
    p: &Palette,
) {
    let s = scale as f32;
    let icon = icon_box(rect, size);
    match (badge.icon, badge.icon.picture()) {
        (_, Some(picture)) => {
            for layer in [&picture.mono, &picture.color].into_iter().flatten() {
                c.picture_sized(layer, icon, scale, tint, size);
            }
        }
        (AgentIcon::Test, None) => {
            let k = size as f32 / 14.0 * s;
            icon_sdf(c, icon, scale, tint, |x, y| flask(x, y, k));
        }
        _ => {
            let k = size as f32 / 14.0 * s;
            icon_sdf(c, icon, scale, tint, |x, y| robot(x, y, k));
        }
    }
    let spot = dot_rect(rect, size);
    let d = DOT as f32 * s;
    icon_sdf(c, spot, scale, ground, |x, y| {
        x.hypot(y) - (d / 2.0 + DOT_RING * s)
    });
    if !badge.dot.blinks() {
        let color = badge.dot.color(p);
        icon_sdf(c, spot, scale, color, |x, y| dot_shape(badge.dot, x, y, d));
    }
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

    /// `badge` drawn 16 big at scale 1 on `GROUND`, and its dot's rect.
    fn drawn(badge: Badge) -> (Canvas, Rect) {
        let p = Theme::Dark.palette();
        let mut c = Canvas::new((24, 24).into());
        c.fill(0, 0, 24, 24, GROUND);
        let rect = Rectangle::new((4, 4).into(), (16, 16).into());
        agent_badge(&mut c, rect, 16, badge, p.text, GROUND, 1.0, p);
        (c, dot_rect(rect, 16))
    }

    fn rgb(c: &Canvas, x: i32, y: i32) -> Rgb {
        let i = ((y * c.w + x) * 4) as usize;
        [c.px[i + 2], c.px[i + 1], c.px[i]]
    }

    fn centre(rect: Rect) -> (i32, i32) {
        (rect.loc.x + rect.size.w / 2, rect.loc.y + rect.size.h / 2)
    }

    #[test]
    fn the_dot_sits_on_the_icons_corner() {
        let rect = Rectangle::new((10, 20).into(), (16, 24).into());
        let dot = dot_rect(rect, 16);
        // The icon spans 10..26 across and 24..40 down.
        assert_eq!(dot.loc, (21, 35).into());
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
            let (c, spot) = drawn(Badge {
                icon: AgentIcon::Claude,
                dot,
            });
            let (x, y) = centre(spot);
            assert_eq!(rgb(&c, x, y), color, "{dot:?}");
        }
        // A blinking one leaves its place cut out, for the composed dot.
        let (c, spot) = drawn(Badge {
            icon: AgentIcon::Claude,
            dot: Dot::Working,
        });
        let (x, y) = centre(spot);
        assert_eq!(rgb(&c, x, y), GROUND);
    }

    #[test]
    fn an_idle_dot_is_a_ring() {
        let p = Theme::Dark.palette();
        let (c, spot) = drawn(Badge {
            icon: AgentIcon::Generic,
            dot: Dot::Idle,
        });
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
            let (c, _) = drawn(Badge {
                icon,
                dot: Dot::Done,
            });
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

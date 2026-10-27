//! The toasts at the bottom right of the app area: a stack of short
//! notes, each with a colour bar for its level and, some, a button.

use super::{FONT_SIZE, Painter, Palette, Rect, Rgb, SHADOW, band, rect_px};
use crate::toast::Level;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Rectangle, Size};

/// A toast: this wide at most, this high, this far in from the app area's
/// bottom right corner, this far apart in the stack. The text sits in a
/// band as high as the notice pill the toasts replaced, with its inset.
pub const TOAST_WIDTH: i32 = 360;
pub const TOAST_HEIGHT: i32 = 44;
pub const TOAST_MARGIN: i32 = 16;
pub const TOAST_GAP: i32 = 8;
const TOAST_TEXT_BAND: i32 = 30;
const TOAST_PADDING: i32 = 16;
/// The level's colour bar at the left.
const TOAST_BAR: i32 = 4;
/// The button at the right: its height, the room at its ends and the gap
/// before it.
const TOAST_BUTTON: i32 = 26;
const TOAST_BUTTON_PADDING: i32 = 12;
const TOAST_BUTTON_MIN: i32 = 48;
const TOAST_BUTTON_GAP: i32 = 12;
const TOAST_RADIUS: f32 = 8.0;

/// What a toast shows: which one it is (to fade each in and out on its
/// own), its text, its level (for the colour bar), the label of its
/// button, if it has one, and how wide that is (see
/// `Painter::toast_button_width`). One `leaving` fades out, but keeps its
/// place in the stack, and takes no presses.
#[derive(Clone, Debug, PartialEq)]
pub struct ToastView {
    pub id: u64,
    pub text: String,
    pub level: Level,
    pub action: Option<String>,
    pub button_w: i32,
    pub leaving: bool,
}

/// Where a toast and its button go, in window coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToastLayout {
    pub panel: Rect,
    pub button: Option<Rect>,
}

/// Where `toasts` (oldest first) stack in `area`: the newest at the
/// bottom right, the older ones above it.
pub fn toast_layout(toasts: &[ToastView], area: Rect) -> Vec<ToastLayout> {
    let w = TOAST_WIDTH.min(area.size.w - 2 * TOAST_MARGIN).max(0);
    let right = area.loc.x + area.size.w - TOAST_MARGIN;
    let mut bottom = area.loc.y + area.size.h - TOAST_MARGIN;
    let mut layouts: Vec<ToastLayout> = toasts
        .iter()
        .rev()
        .map(|toast| {
            let panel = Rectangle::new(
                (right - w, bottom - TOAST_HEIGHT).into(),
                (w, TOAST_HEIGHT).into(),
            );
            bottom -= TOAST_HEIGHT + TOAST_GAP;
            ToastLayout {
                panel,
                button: toast
                    .action
                    .as_ref()
                    .map(|_| button_rect(panel, toast.button_w)),
            }
        })
        .collect();
    layouts.reverse();
    layouts
}

/// The button `w` wide of a toast in `panel`, at its right end.
fn button_rect(panel: Rect, w: i32) -> Rect {
    let w = w.max(TOAST_BUTTON_MIN);
    Rectangle::new(
        (
            panel.loc.x + panel.size.w - TOAST_PADDING - w,
            panel.loc.y + (panel.size.h - TOAST_BUTTON) / 2,
        )
            .into(),
        (w, TOAST_BUTTON).into(),
    )
}

/// The colour of a toast's bar for `level`.
fn level_color(level: Level, p: &Palette) -> Rgb {
    match level {
        Level::Info => p.accent,
        Level::Success => p.success,
        Level::Warning => p.warning,
        Level::Error => p.error,
    }
}

impl Painter {
    /// How wide a toast's button labelled `label` is: the label as the font
    /// sets it, with room at its ends. Logical pixels: the font's advances
    /// grow with the scale.
    pub(super) fn toast_button_width(&self, label: &str) -> i32 {
        let text = self.text_width(label, FONT_SIZE, 1.0).ceil() as i32;
        (text + 2 * TOAST_BUTTON_PADDING).max(TOAST_BUTTON_MIN)
    }

    /// A toast `w` wide, inside a margin of `SHADOW` holding its drop
    /// shadow: its level's bar at the left, its text, and its button, lit
    /// when `hovered`.
    pub(super) fn toast(
        &self,
        view: &ToastView,
        w: i32,
        hovered: bool,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let size = Size::from((w, TOAST_HEIGHT));
        let mut c = self.popup(size, TOAST_RADIUS, scale, p);
        let s = scale as f32;
        let panel = Rectangle::new((SHADOW, SHADOW).into(), size);
        // The bar, inset from the panel's edge and rounded like it.
        let (x, y, _, h) = rect_px(panel, scale);
        let inset = (6.0 * s).round();
        let bar_w = (TOAST_BAR as f32 * s).round();
        c.rounded(
            x + inset,
            y + inset,
            bar_w,
            h - 2.0 * inset,
            bar_w / 2.0,
            level_color(view.level, p),
        );
        let text_x = x + inset + bar_w + TOAST_PADDING as f32 * s;
        let mut text_end = x + (w as f32 - TOAST_PADDING as f32) * s;
        if let Some(label) = &view.action {
            let button = button_rect(panel, view.button_w);
            self.button(&mut c, label, button, (hovered, false), false, scale, p);
            text_end = button.loc.x as f32 * s - TOAST_BUTTON_GAP as f32 * s;
        }
        let text_band = Rectangle::new(
            (
                panel.loc.x,
                panel.loc.y + (TOAST_HEIGHT - TOAST_TEXT_BAND) / 2,
            )
                .into(),
            (w, TOAST_TEXT_BAND).into(),
        );
        self.text(
            &mut c,
            &view.text,
            FONT_SIZE,
            text_x,
            text_end,
            band(text_band, scale),
            scale,
            p.text,
        );
        c.into_buffer(false)
    }
}

#[cfg(test)]
mod tests {
    use super::{TOAST_GAP, TOAST_HEIGHT, TOAST_MARGIN, TOAST_WIDTH, ToastView, toast_layout};
    use crate::{chrome::Rect, toast::Level};

    fn toast(id: u64, action: Option<&str>) -> ToastView {
        ToastView {
            id,
            text: "Switched to gerdr".into(),
            level: Level::Info,
            action: action.map(str::to_owned),
            button_w: 64,
            leaving: false,
        }
    }

    #[test]
    fn toasts_stack_up_from_the_bottom_right() {
        let area = Rect::new((200, 48).into(), (1000, 700).into());
        let toasts = [toast(1, None), toast(2, Some("Retry")), toast(3, None)];
        let layouts = toast_layout(&toasts, area);
        assert_eq!(layouts.len(), 3);
        let newest = layouts[2].panel;
        assert_eq!(newest.loc.x + newest.size.w, 1200 - TOAST_MARGIN);
        assert_eq!(newest.loc.y + newest.size.h, 748 - TOAST_MARGIN);
        assert_eq!(newest.size, (TOAST_WIDTH, TOAST_HEIGHT).into());
        assert_eq!(
            layouts[1].panel.loc.y + TOAST_HEIGHT + TOAST_GAP,
            newest.loc.y
        );
        let button = layouts[1]
            .button
            .expect("the toast with an action has a button");
        assert!(button.loc.x > layouts[1].panel.loc.x);
        assert!(button.loc.x + button.size.w < layouts[1].panel.loc.x + layouts[1].panel.size.w);
        assert!(layouts[0].button.is_none() && layouts[2].button.is_none());
        // A narrow area narrows the toasts.
        let narrow = Rect::new((0, 0).into(), (300, 300).into());
        assert_eq!(
            toast_layout(&toasts, narrow)[0].panel.size.w,
            300 - 2 * TOAST_MARGIN
        );
    }
}

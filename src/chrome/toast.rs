//! The toasts at the bottom right of the app area: a stack of short
//! notes, each with a colour bar for its level, its text on up to two
//! lines, some a button, and a "×" that dismisses it.

use super::{
    FONT_SIZE, Painter, Palette, Rect, Rgb, SHADOW, band, cross, icon_sdf, measure, rect_px,
};
use crate::toast::Level;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// A toast: this wide at most, this high with one line of text (with two,
/// `TOAST_TALL`), this far in from the app area's bottom right corner,
/// this far apart in the stack. One line sits in a band as high as the
/// notice pill the toasts replaced, with its inset; two lines share the
/// toast's height but for `TOAST_LINES_INSET` above and below.
pub const TOAST_WIDTH: i32 = 360;
pub const TOAST_HEIGHT: i32 = 44;
pub const TOAST_TALL: i32 = 64;
pub const TOAST_MARGIN: i32 = 16;
pub const TOAST_GAP: i32 = 8;
const TOAST_TEXT_BAND: i32 = 30;
const TOAST_LINES_INSET: i32 = 8;
const TOAST_PADDING: i32 = 16;
/// The level's colour bar at the left, inset from the panel's edge.
const TOAST_BAR: i32 = 4;
const TOAST_BAR_INSET: i32 = 6;
/// The button at the right: its height, the room at its ends and the gap
/// before it.
const TOAST_BUTTON: i32 = 26;
const TOAST_BUTTON_PADDING: i32 = 12;
const TOAST_BUTTON_MIN: i32 = 48;
const TOAST_BUTTON_GAP: i32 = 12;
/// The "×" at the right end: its square, the room after it, and the gap
/// between it and the button or text before it.
const TOAST_CLOSE: i32 = 20;
const TOAST_CLOSE_INSET: i32 = 8;
const TOAST_CLOSE_GAP: i32 = 6;
/// The corners of the "×"'s highlight under the pointer.
const TOAST_CLOSE_RADIUS: f32 = 4.0;
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

/// Where a toast, its button and its "×" go, in window coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToastLayout {
    pub panel: Rect,
    pub button: Option<Rect>,
    pub close: Rect,
}

/// What of a toast lights up under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastSpot {
    /// Its button, which does what it says.
    Button,
    /// Its "×", which dismisses it.
    Close,
}

impl ToastLayout {
    /// What of the toast is at `pos` (window coordinates): its button, its
    /// "×", or neither.
    pub fn spot_at(&self, pos: Point<f64, Logical>) -> Option<ToastSpot> {
        let on = |rect: Rect| rect.to_f64().contains(pos);
        if on(self.close) {
            Some(ToastSpot::Close)
        } else if self.button.is_some_and(on) {
            Some(ToastSpot::Button)
        } else {
            None
        }
    }
}

/// Where `toasts` (oldest first) stack in `area`: the newest at the
/// bottom right, the older ones above it, each as tall as its text needs.
pub fn toast_layout(toasts: &[ToastView], area: Rect) -> Vec<ToastLayout> {
    let w = TOAST_WIDTH.min(area.size.w - 2 * TOAST_MARGIN).max(0);
    let right = area.loc.x + area.size.w - TOAST_MARGIN;
    let mut bottom = area.loc.y + area.size.h - TOAST_MARGIN;
    let mut layouts: Vec<ToastLayout> = toasts
        .iter()
        .rev()
        .map(|toast| {
            let h = toast_height(toast, w);
            let panel = Rectangle::new((right - w, bottom - h).into(), (w, h).into());
            bottom -= h + TOAST_GAP;
            let close = close_rect(panel);
            ToastLayout {
                panel,
                button: toast
                    .action
                    .as_ref()
                    .map(|_| button_rect(panel, close, toast.button_w)),
                close,
            }
        })
        .collect();
    layouts.reverse();
    layouts
}

/// How tall `toast` is when `w` wide: two lines' worth when its text
/// doesn't fit on one.
pub fn toast_height(toast: &ToastView, w: i32) -> i32 {
    let width = measure(&toast.text, FONT_SIZE)
        .unwrap_or(toast.text.chars().count() as f32 * FONT_SIZE * 0.6);
    if width > text_room(toast, w) as f32 {
        TOAST_TALL
    } else {
        TOAST_HEIGHT
    }
}

/// How much room the text of `toast`, `w` wide, has on a line: from after
/// the bar to before its button, else its "×".
fn text_room(toast: &ToastView, w: i32) -> i32 {
    let start = TOAST_BAR_INSET + TOAST_BAR + TOAST_PADDING;
    let mut end = w - TOAST_CLOSE_INSET - TOAST_CLOSE - TOAST_CLOSE_GAP;
    if toast.action.is_some() {
        end -= toast.button_w.max(TOAST_BUTTON_MIN) + TOAST_BUTTON_GAP;
    }
    (end - start).max(0)
}

/// The part of the app area `area` the toasts stack up from the bottom of:
/// all of it, or what is above `unzoom`, the button over a zoomed pane, so
/// that they leave it in sight.
pub fn toast_area(area: Rect, unzoom: Option<Rect>) -> Rect {
    match unzoom {
        Some(button) => Rectangle::new(
            area.loc,
            (area.size.w, (button.loc.y - area.loc.y).max(0)).into(),
        ),
        None => area,
    }
}

/// The "×" of a toast in `panel`, at its right end.
fn close_rect(panel: Rect) -> Rect {
    Rectangle::new(
        (
            panel.loc.x + panel.size.w - TOAST_CLOSE_INSET - TOAST_CLOSE,
            panel.loc.y + (panel.size.h - TOAST_CLOSE) / 2,
        )
            .into(),
        (TOAST_CLOSE, TOAST_CLOSE).into(),
    )
}

/// The button `w` wide of a toast in `panel`, before its "×" `close`.
fn button_rect(panel: Rect, close: Rect, w: i32) -> Rect {
    let w = w.max(TOAST_BUTTON_MIN);
    Rectangle::new(
        (
            close.loc.x - TOAST_CLOSE_GAP - w,
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

    /// `text` as a toast's two lines, the first at most `room` physical
    /// pixels at `scale`: cut at the last space that fits, else where it
    /// runs past.
    fn toast_lines<'a>(&self, text: &'a str, room: f32, scale: f64) -> (&'a str, &'a str) {
        let (mut space, mut fit) = (None, 0);
        for (at, ch) in text.char_indices() {
            let end = at + ch.len_utf8();
            if self.text_width(&text[..end], FONT_SIZE, scale) > room {
                break;
            }
            fit = end;
            if ch == ' ' && at > 0 {
                space = Some(at);
            }
        }
        if fit == text.len() {
            return (text, "");
        }
        let (first, rest) = text.split_at(space.unwrap_or(fit));
        (first.trim_end(), rest.trim_start())
    }

    /// `toast_lit`, with only its button lit, when `hovered`.
    #[cfg(test)]
    pub(super) fn toast(
        &self,
        view: &ToastView,
        w: i32,
        hovered: bool,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let lit = hovered.then_some(ToastSpot::Button);
        self.toast_lit(view, w, lit, scale, p)
    }

    /// A toast `w` wide, inside a margin of `SHADOW` holding its drop
    /// shadow: its level's bar at the left, its text on one line or two
    /// (the second cut with "…"), its button and the "×" dismissing it,
    /// the one of them `lit` lit.
    pub(super) fn toast_lit(
        &self,
        view: &ToastView,
        w: i32,
        lit: Option<ToastSpot>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let h = toast_height(view, w);
        let size = Size::from((w, h));
        let mut c = self.popup(size, TOAST_RADIUS, scale, p);
        let s = scale as f32;
        let panel = Rectangle::new((SHADOW, SHADOW).into(), size);
        // The bar, inset from the panel's edge and rounded like it.
        let (x, y, _, ph) = rect_px(panel, scale);
        let inset = (TOAST_BAR_INSET as f32 * s).round();
        let bar_w = (TOAST_BAR as f32 * s).round();
        c.rounded(
            x + inset,
            y + inset,
            bar_w,
            ph - 2.0 * inset,
            bar_w / 2.0,
            level_color(view.level, p),
        );
        let close = close_rect(panel);
        let close_lit = lit == Some(ToastSpot::Close);
        if close_lit {
            let (x, y, w, h) = rect_px(close, scale);
            c.rounded(x, y, w, h, TOAST_CLOSE_RADIUS * s, p.element);
        }
        let ink = if close_lit { p.text } else { p.muted };
        icon_sdf(&mut c, close, scale, ink, |x, y| cross(x, y, 0.75 * s));
        let text_x = x + inset + bar_w + TOAST_PADDING as f32 * s;
        let mut text_end = (close.loc.x - TOAST_CLOSE_GAP) as f32 * s;
        if let Some(label) = &view.action {
            let button = button_rect(panel, close, view.button_w);
            let hovered = lit == Some(ToastSpot::Button);
            self.button(&mut c, label, button, (hovered, false), false, scale, p);
            text_end = button.loc.x as f32 * s - TOAST_BUTTON_GAP as f32 * s;
        }
        let line = |top: i32, height: i32| {
            Rectangle::new((panel.loc.x, panel.loc.y + top).into(), (w, height).into())
        };
        let lines = if h == TOAST_HEIGHT {
            let one = line((TOAST_HEIGHT - TOAST_TEXT_BAND) / 2, TOAST_TEXT_BAND);
            vec![(view.text.as_str(), one)]
        } else {
            let (first, rest) = self.toast_lines(&view.text, text_end - text_x, scale);
            let half = (h - 2 * TOAST_LINES_INSET) / 2;
            vec![
                (first, line(TOAST_LINES_INSET, half)),
                (rest, line(TOAST_LINES_INSET + half, half)),
            ]
        };
        for (text, rect) in lines {
            self.text(
                &mut c,
                text,
                FONT_SIZE,
                text_x,
                text_end,
                band(rect, scale),
                scale,
                p.text,
            );
        }
        c.into_buffer(false)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TOAST_GAP, TOAST_HEIGHT, TOAST_MARGIN, TOAST_TALL, TOAST_WIDTH, ToastSpot, ToastView,
        toast_area, toast_height, toast_layout,
    };
    use crate::{
        chrome::{Painter, Rect, Theme, inside},
        toast::Level,
    };
    use smithay::backend::renderer::element::memory::MemoryRenderBuffer;

    /// The bytes `buffer` holds.
    fn pixels(buffer: &MemoryRenderBuffer) -> Vec<u8> {
        let mut bytes = Vec::new();
        let _ = buffer.clone().render().draw(|mem| {
            bytes.extend_from_slice(mem);
            Ok::<_, ()>(Vec::new())
        });
        bytes
    }

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
        assert!(button.loc.x + button.size.w < layouts[1].close.loc.x);
        assert!(layouts[0].button.is_none() && layouts[2].button.is_none());
        // Every toast has its "×", inside it.
        assert!(layouts.iter().all(|l| inside(l.panel, l.close)));
        // A narrow area narrows the toasts.
        let narrow = Rect::new((0, 0).into(), (300, 300).into());
        assert_eq!(
            toast_layout(&toasts, narrow)[0].panel.size.w,
            300 - 2 * TOAST_MARGIN
        );
    }

    #[test]
    fn a_long_toast_takes_two_lines() {
        let mut long = toast(1, Some("Open settings"));
        long.text = "Couldn't read the settings: unknown field `densty` in [ui]".repeat(2);
        assert_eq!(toast_height(&long, TOAST_WIDTH), TOAST_TALL);
        assert_eq!(toast_height(&toast(2, None), TOAST_WIDTH), TOAST_HEIGHT);
        let area = Rect::new((0, 0).into(), (1000, 700).into());
        let layouts = toast_layout(&[long.clone(), toast(2, None)], area);
        assert_eq!(layouts[0].panel.size.h, TOAST_TALL);
        assert_eq!(
            layouts[0].panel.loc.y + TOAST_TALL + TOAST_GAP,
            layouts[1].panel.loc.y
        );
        let painter = Painter::new();
        for theme in [Theme::Dark, Theme::Light] {
            for scale in [1.0, 2.0] {
                painter.toast(&long, TOAST_WIDTH, true, scale, theme.palette());
            }
        }
    }

    #[test]
    fn the_pointer_lights_a_toasts_button_or_its_close() {
        let area = Rect::new((0, 0).into(), (1000, 700).into());
        let toasts = [toast(1, Some("Retry")), toast(2, None)];
        let layouts = toast_layout(&toasts, area);
        let centre = |r: Rect| (r.loc.x as f64 + 2.0, r.loc.y as f64 + 2.0).into();
        let button = layouts[0].button.unwrap();
        assert_eq!(layouts[0].spot_at(centre(button)), Some(ToastSpot::Button));
        assert_eq!(
            layouts[0].spot_at(centre(layouts[0].close)),
            Some(ToastSpot::Close)
        );
        assert_eq!(
            layouts[1].spot_at(centre(layouts[1].close)),
            Some(ToastSpot::Close)
        );
        // Elsewhere on the toast, nothing lights up.
        assert_eq!(layouts[1].spot_at(centre(layouts[1].panel)), None);
        let painter = Painter::new();
        let p = Theme::Dark.palette();
        let plain = painter.toast_lit(&toasts[0], TOAST_WIDTH, None, 1.0, p);
        let lit = painter.toast_lit(&toasts[0], TOAST_WIDTH, Some(ToastSpot::Close), 1.0, p);
        assert_ne!(pixels(&plain), pixels(&lit));
    }

    #[test]
    fn toasts_stack_above_the_unzoom_button() {
        let area = Rect::new((200, 48).into(), (1000, 700).into());
        assert_eq!(toast_area(area, None), area);
        let button = Rect::new((1148, 696).into(), (36, 36).into());
        let above = toast_area(area, Some(button));
        let layouts = toast_layout(&[toast(1, None)], above);
        let panel = layouts[0].panel;
        assert!(panel.loc.y + panel.size.h < button.loc.y);
    }
}

//! Drawing text, fields, keycaps and buttons into a `Canvas` with the
//! system font.

use super::{
    BUTTON_RADIUS, CAP_GAP, CAP_PADDING, CAP_RADIUS, CARET_HEIGHT, CARET_WIDTH, CHECKBOX_SIZE,
    Canvas, Check, DETAIL_FONT_SIZE, FOCUS_GAP, FOCUS_RING, FONT_SIZE, GIT_MARK_GAP,
    GIT_MARK_WIDTH, KEY_FONT_SIZE, PADDING, Palette, ROW_HIGHLIGHT_RADIUS, Rect, Rgb, SHADOW,
    SMALL_FONT_SIZE, TINY_FONT_SIZE, arrow, check, coverage, cross, mix, segment,
};
use crate::git::{Change, GitStatus};
use crate::text_field::Line;
use ab_glyph::{Font, FontVec, PxScale, ScaleFont, VariableFont};
use smithay::utils::{Logical, Rectangle, Size};
use std::cell::RefCell;

/// A button's count badge: its height, and the least room on each side of
/// the number.
const BADGE_HEIGHT: i32 = 15;
const BADGE_PAD: i32 = 4;

/// How many bytes of popup backgrounds `Painter` keeps, besides the last
/// one drawn.
const POPUP_CACHE_BYTES: usize = 64 << 20;

pub(super) struct Painter {
    pub(super) font: Option<FontVec>,
    /// The monospace font, for diffs and logs; `None` when there is none,
    /// and the font above is used instead.
    pub(super) mono: Option<FontVec>,
    /// The UI font in bold, for names that head a group (a repository's);
    /// `None` when there is none, and the UI font is used instead.
    pub(super) bold: Option<FontVec>,
    /// Popup backgrounds drawn lately, the last used first: a popup drawn
    /// again, say as its list scrolls, starts from a copy.
    popups: RefCell<Vec<(PopupKey, Canvas)>>,
}

/// What a popup's background is drawn from.
#[derive(Clone, Copy, PartialEq)]
struct PopupKey {
    size: Size<i32, Logical>,
    radius: f32,
    scale: f64,
    border: Rgb,
    elevated: Rgb,
    shadow: f32,
}

/// A logical rectangle's left and right edges in physical pixels.
pub(super) fn phys(rect: Rect, scale: f64) -> (f32, f32) {
    let px = |v: i32| (v as f64 * scale).round() as f32;
    (px(rect.loc.x), px(rect.loc.x + rect.size.w))
}

/// A logical rectangle in physical pixels (x, y, width, height), its edges
/// on whole pixels.
pub(super) fn rect_px(rect: Rect, scale: f64) -> (f32, f32, f32, f32) {
    let px = |v: i32| (v as f64 * scale).round() as f32;
    let (x, y) = (px(rect.loc.x), px(rect.loc.y));
    (
        x,
        y,
        px(rect.loc.x + rect.size.w) - x,
        px(rect.loc.y + rect.size.h) - y,
    )
}

/// A logical rectangle's rows as the band `(top, height)` text is centred in.
pub(super) fn band(rect: Rect, scale: f64) -> (i32, i32) {
    let px = |v: i32| (v as f64 * scale).round() as i32;
    (
        px(rect.loc.y),
        px(rect.loc.y + rect.size.h) - px(rect.loc.y),
    )
}

/// The focused text field of an overlay: the area a click in it lands in,
/// where its text goes (physical pixels, from and up to), and what it shows.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldSpot {
    pub area: Rect,
    pub text: (f32, f32),
    /// The size of the font its text is in.
    pub size: f32,
    pub line: Line,
}

/// Where the text of a field drawn in `rect` (by `Painter::field`) goes.
pub(super) fn field_text(rect: Rect, scale: f64) -> (f32, f32) {
    let px = |v: i32| (v as f64 * scale).round() as f32;
    let (x, _, w, _) = rect_px(rect, scale);
    let pad = PADDING * scale as f32;
    (x + pad, x + w - pad - px(CARET_WIDTH + 1))
}

/// The text caret at `x`, vertically centred in the band `(top, height)`.
pub(super) fn caret(c: &mut Canvas, x: f32, (top, height): (i32, i32), scale: f64, color: Rgb) {
    let px = |v: i32| (v as f64 * scale).round() as i32;
    let h = px(CARET_HEIGHT).min(height);
    c.fill(
        x.round() as i32,
        top + (height - h) / 2,
        px(CARET_WIDTH).max(1),
        h,
        color,
    );
}

/// A ring in `color` around a button in `rect` with corners of `radius`
/// (logical pixels), held off it by a gap showing `behind`.
pub(super) fn focus_ring(
    c: &mut Canvas,
    rect: Rect,
    radius: f32,
    behind: Rgb,
    scale: f64,
    color: Rgb,
) {
    let s = scale as f32;
    let (x, y, w, h) = rect_px(rect, scale);
    let (gap, out) = (FOCUS_GAP * s, (FOCUS_GAP + FOCUS_RING) * s);
    let grown = |by: f32| (x - by, y - by, w + 2.0 * by, h + 2.0 * by, radius * s + by);
    let (rx, ry, rw, rh, rr) = grown(out);
    c.rounded(rx, ry, rw, rh, rr, color);
    let (gx, gy, gw, gh, gr) = grown(gap);
    c.rounded(gx, gy, gw, gh, gr, behind);
}

/// What a keycap for `key` says: the key's short name, "Del" for
/// "Delete" ("Shift+Del"), so every panel names a key the same way.
pub(super) fn keycap_text(key: &str) -> std::borrow::Cow<'_, str> {
    match key.strip_suffix("Delete") {
        Some(mods) if mods.is_empty() || mods.ends_with('+') => format!("{mods}Del").into(),
        _ => key.into(),
    }
}

/// Room either side of a button's label, and the narrowest a button is.
const BUTTON_PADDING: i32 = 20;
const BUTTON_MIN_WIDTH: i32 = 84;

/// How a button looks (see `Painter::button`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonStyle {
    /// Outlined, on the element colour.
    #[default]
    Plain,
    /// Filled with the accent: the action a dialog is for.
    Primary,
    /// Filled with the error colour: an action that loses something.
    /// Never the default of its dialog.
    Destructive,
}

impl From<bool> for ButtonStyle {
    /// `true` for the primary button, `false` for a plain one.
    fn from(primary: bool) -> Self {
        if primary {
            ButtonStyle::Primary
        } else {
            ButtonStyle::Plain
        }
    }
}

/// What a list row is lit with on a popup's ground: `selected` when it is
/// the selected row, else `element` faded in from the ground as far as
/// `hover` (0 to 1) says; `None` for a row neither selected nor hovered.
pub(super) fn row_fill(p: &Palette, selected: bool, hover: f32) -> Option<Rgb> {
    if selected {
        return Some(p.selected);
    }
    let hover = hover.clamp(0.0, 1.0);
    (hover > 0.0).then(|| mix(p.elevated, p.element, hover))
}

/// Pixel size of a buffer covering `size` logical pixels.
pub(super) fn physical(size: Size<i32, Logical>, scale: f64) -> Size<i32, Logical> {
    let px = |v: i32| ((v as f64 * scale).round() as i32).max(1);
    (px(size.w), px(size.h)).into()
}

/// The file of the system's sans-serif font, as fontconfig picks it.
pub(super) fn find_font() -> Option<String> {
    find_family("sans-serif")
}

/// The file of the system's monospace font, as fontconfig picks it.
fn find_mono_font() -> Option<String> {
    find_family("monospace")
}

/// The file fontconfig picks for `family`.
fn find_family(family: &str) -> Option<String> {
    let out = std::process::Command::new("fc-match")
        .args(["-f", "%{file}", family])
        .output()
        .ok()?;
    let path = String::from_utf8(out.stdout).ok()?;
    (!path.is_empty()).then_some(path)
}

/// The UI font in bold: the UI font's own file at a bold weight when it
/// has a weight axis, else the bold face fontconfig picks.
fn load_bold() -> Option<FontVec> {
    if let Some(mut font) = load_font(find_font())
        && font.set_variation(b"wght", BOLD_WEIGHT)
    {
        return Some(font);
    }
    load_font(find_family("sans-serif:bold"))
}

/// The weight bold text is drawn at, from a font with a weight axis.
const BOLD_WEIGHT: f32 = 650.0;

/// The font in the file at `path`, if it can be read.
fn load_font(path: Option<String>) -> Option<FontVec> {
    let data = std::fs::read(path?).ok()?;
    FontVec::try_from_vec(data).ok()
}

/// The fonts that draw what the monospace font lacks (a TUI's `❯`, `⏺`,
/// `✓`), by character: as fontconfig picks them, each font loaded once
/// and kept; `None` when no font has it.
type Fallbacks = std::collections::HashMap<char, Option<&'static FontVec>>;

/// The font to draw each of `chars` with, which the monospace font lacks:
/// looked up the first time (one `fc-match` for all those new, then one
/// for those its pick lacks, while picks help), then remembered.
fn fallbacks(chars: &[char]) -> Vec<Option<&'static FontVec>> {
    use std::sync::{Mutex, OnceLock};
    static FOUND: OnceLock<Mutex<Fallbacks>> = OnceLock::new();
    static FILES: OnceLock<Mutex<Vec<(String, &'static FontVec)>>> = OnceLock::new();
    let found = FOUND.get_or_init(Mutex::default);
    let mut found = found.lock().unwrap_or_else(|e| e.into_inner());
    let mut new: Vec<char> = chars
        .iter()
        .copied()
        .filter(|ch| !found.contains_key(ch))
        .collect();
    new.sort_unstable();
    new.dedup();
    let files = FILES.get_or_init(Mutex::default);
    let mut files = files.lock().unwrap_or_else(|e| e.into_inner());
    while !new.is_empty() {
        let charset: Vec<String> = new.iter().map(|ch| format!("{:x}", *ch as u32)).collect();
        let file = find_family(&format!("monospace:charset={}", charset.join(" ")));
        let font = file.and_then(|file| match files.iter().find(|(f, _)| *f == file) {
            Some((_, font)) => Some(*font),
            None => {
                let font: &'static FontVec = Box::leak(Box::new(load_font(Some(file.clone()))?));
                files.push((file, font));
                Some(font)
            }
        });
        let has = |ch: char| font.filter(|font| font.glyph_id(ch).0 != 0);
        let (drawn, rest): (Vec<char>, Vec<char>) = new.iter().partition(|&&ch| has(ch).is_some());
        // No font has the rest.
        let last = drawn.is_empty();
        for ch in drawn {
            found.insert(ch, has(ch));
        }
        if last {
            for ch in rest {
                found.insert(ch, None);
            }
            break;
        }
        new = rest;
    }
    chars
        .iter()
        .map(|ch| found.get(ch).copied().flatten())
        .collect()
}

/// How wide `text` is in the UI font at `size`, in logical pixels, for a
/// layout that has no painter at hand; `None` without a font. The font is
/// loaded the first time.
pub(super) fn measure(text: &str, size: f32) -> Option<f32> {
    static FONT: std::sync::OnceLock<Option<FontVec>> = std::sync::OnceLock::new();
    let font = FONT.get_or_init(|| load_font(find_font())).as_ref()?;
    Some(width_in(font, text, size, 1.0))
}

/// Width of `text` in the bold UI font (else the UI font) at `size`, at
/// scale 1, as `measure` has it.
pub(super) fn measure_bold(text: &str, size: f32) -> Option<f32> {
    static FONT: std::sync::OnceLock<Option<FontVec>> = std::sync::OnceLock::new();
    let font = FONT
        .get_or_init(|| load_bold().or_else(|| load_font(find_font())))
        .as_ref()?;
    Some(width_in(font, text, size, 1.0))
}

/// Width of `text` in the monospace font (else the UI font) at `size`,
/// at scale 1, as `measure` has it.
pub(super) fn measure_mono(text: &str, size: f32) -> Option<f32> {
    static FONT: std::sync::OnceLock<Option<FontVec>> = std::sync::OnceLock::new();
    let font = FONT
        .get_or_init(|| load_font(find_mono_font()).or_else(|| load_font(find_font())))
        .as_ref()?;
    Some(width_in(font, text, size, 1.0))
}

/// Width of `text` in `font` at `size` (logical) and `scale`, in physical
/// pixels.
fn width_in(font: &FontVec, text: &str, size: f32, scale: f64) -> f32 {
    let font = font.as_scaled(PxScale::from(size * scale as f32));
    text.chars().map(|c| font.h_advance(font.glyph_id(c))).sum()
}

/// Draw `text` in `font` as `Painter::text` does.
#[allow(clippy::too_many_arguments)]
fn text_in(
    font: &FontVec,
    c: &mut Canvas,
    text: &str,
    size: f32,
    x: f32,
    max_x: f32,
    (top, height): (i32, i32),
    scale: f64,
    color: Rgb,
) -> f32 {
    let scaled = font.as_scaled(PxScale::from(size * scale as f32));
    // Centre on the cap height; the font's ascent/descent include line gap padding.
    let cap_height = font
        .outline_glyph(scaled.scaled_glyph('H'))
        .map_or(scaled.ascent(), |o| -o.px_bounds().min.y);
    let baseline = (top as f32 + (height as f32 + cap_height) / 2.0).round();

    let (mut shown, mut cut) = (text, false);
    if x + width_in(font, text, size, scale) > max_x {
        // The longest start of `text` that fits with an ellipsis after it.
        let ellipsis = width_in(font, "…", size, scale);
        let mut width = 0.0;
        let mut end = 0;
        for (i, ch) in text.char_indices() {
            width += scaled.h_advance(scaled.glyph_id(ch));
            if x + width + ellipsis > max_x {
                break;
            }
            end = i + ch.len_utf8();
        }
        (shown, cut) = (&text[..end], true);
    }

    let mut pen = x;
    for ch in shown.chars().chain(cut.then_some('…')) {
        let id = scaled.glyph_id(ch);
        let glyph = id.with_scale_and_position(scaled.scale(), ab_glyph::point(pen, baseline));
        pen += scaled.h_advance(id);
        if let Some(outline) = font.outline_glyph(glyph) {
            let bounds = outline.px_bounds();
            outline.draw(|gx, gy, coverage| {
                c.blend(
                    bounds.min.x as i32 + gx as i32,
                    bounds.min.y as i32 + gy as i32,
                    color,
                    coverage,
                );
            });
        }
    }
    pen
}

impl Painter {
    pub(super) fn new() -> Self {
        let font = load_font(find_font());
        if font.is_none() {
            tracing::warn!("chrome: no usable font found, labels will be blank");
        }
        let mono = load_font(find_mono_font());
        if mono.is_none() {
            tracing::warn!("chrome: no monospace font found, diffs use the UI font");
        }
        Self {
            font,
            mono,
            bold: load_bold(),
            popups: RefCell::default(),
        }
    }

    /// A canvas for a popup panel of logical `size` with its shadow, border
    /// and fill drawn (see `Canvas::popup`), the panel `SHADOW` in from the
    /// top left.
    pub(super) fn popup(
        &self,
        size: Size<i32, Logical>,
        radius: f32,
        scale: f64,
        p: &Palette,
    ) -> Canvas {
        let key = PopupKey {
            size,
            radius,
            scale,
            border: p.border,
            elevated: p.elevated,
            shadow: p.shadow,
        };
        let mut popups = self.popups.borrow_mut();
        let entry = match popups.iter().position(|(k, _)| *k == key) {
            Some(i) => popups.remove(i),
            None => {
                let outer = Size::from((size.w + 2 * SHADOW, size.h + 2 * SHADOW));
                let mut c = Canvas::new(physical(outer, scale));
                c.popup(size, radius, scale, p);
                (key, c)
            }
        };
        popups.insert(0, entry);
        let mut kept = 0;
        popups.retain(|(_, c)| {
            let keep = kept == 0 || kept + c.px.len() <= POPUP_CACHE_BYTES;
            if keep {
                kept += c.px.len();
            }
            keep
        });
        popups[0].1.clone()
    }

    /// The monospace font, else the UI font.
    fn mono_font(&self) -> Option<&FontVec> {
        self.mono.as_ref().or(self.font.as_ref())
    }

    /// Whether the UI font has a glyph for `ch`.
    pub(super) fn has_glyph(&self, ch: char) -> bool {
        self.font
            .as_ref()
            .is_some_and(|font| font.glyph_id(ch).0 != 0)
    }

    /// Width of `text` in the monospace font, in physical pixels.
    pub(super) fn mono_width(&self, text: &str, size: f32, scale: f64) -> f32 {
        self.mono_font()
            .map_or(0.0, |font| width_in(font, text, size, scale))
    }

    /// Draw `text` in the monospace font as `text` does. Returns where it
    /// ends.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn mono_text(
        &self,
        c: &mut Canvas,
        text: &str,
        size: f32,
        x: f32,
        max_x: f32,
        band: (i32, i32),
        scale: f64,
        color: Rgb,
    ) -> f32 {
        let Some(font) = self.mono_font() else {
            return x;
        };
        let lacks = |ch: char| !ch.is_whitespace() && font.glyph_id(ch).0 == 0;
        let missing: Vec<char> = text.chars().filter(|&ch| lacks(ch)).collect();
        if missing.is_empty() {
            return text_in(font, c, text, size, x, max_x, band, scale, color);
        }
        // Runs the monospace font has, and the characters it lacks each in
        // the font fontconfig picks for it.
        let other: std::collections::HashMap<char, &FontVec> = missing
            .iter()
            .zip(fallbacks(&missing))
            .filter_map(|(ch, f)| Some((*ch, f?)))
            .collect();
        let font_of = |ch: char| other.get(&ch).copied().unwrap_or(font);
        let mut pen = x;
        let mut rest = text;
        while let Some(first) = rest.chars().next() {
            let run_font = font_of(first);
            let end = rest
                .char_indices()
                .find(|&(_, ch)| !std::ptr::eq(font_of(ch), run_font))
                .map_or(rest.len(), |(i, _)| i);
            let (run, after) = rest.split_at(end);
            let width = width_in(run_font, run, size, scale);
            if pen + width > max_x {
                // What fits of it, with "…": the end.
                return text_in(run_font, c, run, size, pen, max_x, band, scale, color);
            }
            pen = text_in(run_font, c, run, size, pen, f32::MAX, band, scale, color);
            rest = after;
        }
        pen
    }

    /// A checkbox at the left of `rect`, vertically centred: an outlined
    /// square, filled in the accent with a check mark when `On` and a dash
    /// when `Partial`; lighter when `hovered`. Returns where it ends
    /// (physical pixels).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn checkbox(
        &self,
        c: &mut Canvas,
        rect: Rect,
        state: Check,
        hovered: bool,
        scale: f64,
        p: &Palette,
    ) -> f32 {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let side = px(CHECKBOX_SIZE) as f32;
        let (bx, by) = (
            px(rect.loc.x) as f32,
            (px(rect.loc.y) as f32 + (px(rect.size.h) as f32 - side) / 2.0).round(),
        );
        let (b, r) = (px(1).max(1) as f32, CAP_RADIUS * s);
        if state == Check::Off {
            c.rounded(bx, by, side, side, r, p.border);
            let inner = if hovered { p.selected } else { p.elevated };
            c.rounded(bx + b, by + b, side - 2.0 * b, side - 2.0 * b, r - b, inner);
            return bx + side;
        }
        let fill = if hovered {
            mix(p.accent, p.text, 0.12)
        } else {
            p.accent
        };
        c.rounded(bx, by, side, side, r, fill);
        let (cx, cy) = (bx + side / 2.0, by + side / 2.0);
        c.paint_in(
            bx as i32,
            by as i32,
            side as i32,
            side as i32,
            p.on_accent,
            |x, y| {
                let (x, y) = (x - cx, y - cy);
                coverage(match state {
                    Check::Partial => segment(x, y, -3.5 * s, 0.0, 3.5 * s, 0.0) - 1.1 * s,
                    _ => check(x, y, s),
                })
            },
        );
        bx + side
    }

    /// A text field in `rect`, like the switcher's query: `line` (or
    /// `placeholder` without any) and, when `focused`, its caret and
    /// selection.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn field(
        &self,
        c: &mut Canvas,
        line: &Line,
        placeholder: &str,
        rect: Rect,
        focused: bool,
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let (x, y, w, h) = rect_px(rect, scale);
        let (b, r) = (px(1).max(1) as f32, BUTTON_RADIUS * s);
        c.rounded(x, y, w, h, r, p.border);
        c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, p.element);
        let field_band = band(rect, scale);
        self.edit_line(
            c,
            line,
            placeholder,
            field_text(rect, scale),
            field_band,
            focused,
            FONT_SIZE,
            scale,
            p,
        );
    }

    /// The text of a field being edited, `line`, between `x` and `max_x`
    /// (physical pixels) in the band `(top, height)`: `placeholder` without
    /// any text, and when `focused`, the selection behind it and the caret.
    /// A line too long to fit scrolls to keep the caret in view; otherwise
    /// it is cut off with "…".
    #[allow(clippy::too_many_arguments)]
    pub(super) fn edit_line(
        &self,
        c: &mut Canvas,
        line: &Line,
        placeholder: &str,
        (x, max_x): (f32, f32),
        field_band: (i32, i32),
        focused: bool,
        size: f32,
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        if line.is_empty() {
            let shown = x + px(CARET_WIDTH + 2) as f32;
            self.text(
                c,
                placeholder,
                size,
                shown,
                max_x,
                field_band,
                scale,
                p.placeholder,
            );
            if focused {
                caret(c, x, field_band, scale, p.accent);
            }
            return;
        }
        let width = |text: &str| self.text_width(text, size, scale);
        let to_caret = width(&line[..line.caret]);
        // How far the text moves left for the caret to show.
        let scrolled = (to_caret - (max_x - x)).max(0.0);
        let start = x - scrolled;
        let selection = line.selection();
        if focused && !selection.is_empty() {
            let from = (start + width(&line[..selection.start])).max(x);
            let to = (start + width(&line[..selection.end])).min(max_x);
            let (top, height) = field_band;
            let inset = px(3) as f32;
            c.rounded(
                from - s,
                top as f32 + inset,
                to - from + 2.0 * s,
                height as f32 - 2.0 * inset,
                2.0 * s,
                p.selected,
            );
        }
        if scrolled > 0.0 {
            let clip = (x, max_x);
            self.clipped_text(c, line, start, clip, field_band, size, scale, p.text);
        } else {
            self.text(c, line, size, x, max_x, field_band, scale, p.text);
        }
        if focused {
            let nudge = if line.caret > 0 { px(1) as f32 } else { 0.0 };
            caret(c, start + to_caret + nudge, field_band, scale, p.accent);
        }
    }

    /// A button in `rect` like the quit dialog's, labelled `label`, in
    /// `style`: plain, the primary one in the accent, or a destructive
    /// one in the error colour (`true` and `false` stand for primary and
    /// plain). `(hovered, focused)` lightens it and rings it, the ring in
    /// the button's own colour.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn button(
        &self,
        c: &mut Canvas,
        label: &str,
        rect: Rect,
        (hovered, focused): (bool, bool),
        style: impl Into<ButtonStyle>,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        let (x, y, w, h) = rect_px(rect, scale);
        let (b, r) = (((scale).round() as f32).max(1.0), BUTTON_RADIUS * s);
        let style = style.into();
        if focused {
            let ring = match style {
                ButtonStyle::Destructive => p.error,
                _ => p.accent,
            };
            focus_ring(c, rect, BUTTON_RADIUS, p.elevated, scale, ring);
        }
        let color = match style {
            ButtonStyle::Primary | ButtonStyle::Destructive => {
                let base = match style {
                    ButtonStyle::Destructive => p.error,
                    _ => p.accent,
                };
                let fill = if hovered {
                    mix(base, p.text, 0.12)
                } else {
                    base
                };
                c.rounded(x, y, w, h, r, fill);
                p.on_accent
            }
            ButtonStyle::Plain => {
                let fill = if hovered { p.selected } else { p.element };
                c.rounded(x, y, w, h, r, p.border);
                c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, fill);
                p.text
            }
        };
        self.centered(c, label, FONT_SIZE, rect, scale, color);
    }

    /// How wide a dialog button labelled `label` is (logical pixels): its
    /// text with room either side, and never under `BUTTON_MIN_WIDTH`, so
    /// no dialog keeps a table of widths that drift from its labels.
    #[allow(dead_code)] // The dialogs take it up in wave 2.
    pub(super) fn button_width(&self, label: &str) -> i32 {
        let text = measure(label, FONT_SIZE).unwrap_or(label.chars().count() as f32 * 8.0);
        (text.ceil() as i32 + 2 * BUTTON_PADDING).max(BUTTON_MIN_WIDTH)
    }

    /// A toggle between choices, like a popup's "This workspace / All"
    /// scope: an outlined `track`, and on it a segment per (label, rect),
    /// the one `on` filled in `selected` with its label in `text`, the one
    /// under the pointer (`hover`) lit as a hovered row.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn segmented(
        &self,
        c: &mut Canvas,
        track: Rect,
        segments: &[(&str, Rect)],
        on: usize,
        hover: Option<usize>,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        let (x, y, w, h) = rect_px(track, scale);
        let (b, r) = (((scale).round() as f32).max(1.0), BUTTON_RADIUS * s);
        c.rounded(x, y, w, h, r, p.border);
        c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, p.element);
        for (i, &(label, rect)) in segments.iter().enumerate() {
            let fill = if i == on {
                Some(p.selected)
            } else if hover == Some(i) {
                Some(mix(p.element, p.selected, 0.5))
            } else {
                None
            };
            if let Some(fill) = fill {
                let (x, y, w, h) = rect_px(rect, scale);
                c.rounded(x, y, w, h, ROW_HIGHLIGHT_RADIUS * s, fill);
            }
            let color = if i == on { p.text } else { p.muted };
            self.centered(c, label, SMALL_FONT_SIZE, rect, scale, color);
        }
    }

    /// The ground of a list row in `rect` (buffer coordinates): the
    /// selected row in `selected`, a hovered one fading into `element`
    /// as far as `hover` (0 to 1) says, rounded by
    /// `ROW_HIGHLIGHT_RADIUS`; nothing for a row neither selected nor
    /// hovered. Every list's rows are lit this one way.
    pub(super) fn row_highlight(
        &self,
        c: &mut Canvas,
        rect: Rect,
        (selected, hover): (bool, f32),
        scale: f64,
        p: &Palette,
    ) {
        let Some(fill) = row_fill(p, selected, hover) else {
            return;
        };
        let (x, y, w, h) = rect_px(rect, scale);
        c.rounded(x, y, w, h, ROW_HIGHLIGHT_RADIUS * scale as f32, fill);
    }

    /// `text` with the characters at `hits` (char positions) in the second
    /// of `colors`. Draws nothing where not even "…" fits.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn marked(
        &self,
        c: &mut Canvas,
        text: &str,
        hits: &[usize],
        size: f32,
        x: f32,
        max_x: f32,
        band: (i32, i32),
        scale: f64,
        (color, hit): (Rgb, Rgb),
    ) -> f32 {
        let chars: Vec<char> = text.chars().collect();
        let width = |n: usize| self.text_width(&chars[..n].iter().collect::<String>(), size, scale);
        let ellipsis = self.text_width("…", size, scale);
        let mut n = chars.len();
        let cut = x + width(n) > max_x;
        if cut {
            if x + ellipsis > max_x {
                return x;
            }
            while n > 0 && x + width(n) + ellipsis > max_x {
                n -= 1;
            }
        }
        let mut pen = x;
        let mut i = 0;
        while i < n {
            let is_hit = hits.contains(&i);
            let run = (i..n).find(|j| hits.contains(j) != is_hit).unwrap_or(n);
            let text: String = chars[i..run].iter().collect();
            pen = self.text(
                c,
                &text,
                size,
                pen,
                f32::MAX,
                band,
                scale,
                if is_hit { hit } else { color },
            );
            i = run;
        }
        if cut {
            pen = self.text(c, "…", size, pen, f32::MAX, band, scale, color);
        }
        pen
    }

    /// Width of `keys` drawn as keycaps, in physical pixels.
    pub(super) fn keycaps_width(&self, keys: &str, scale: f64) -> f32 {
        let pad = 2.0 * CAP_PADDING as f32 * scale as f32;
        let sep = self.text_width("/", KEY_FONT_SIZE, scale) + 2.0 * CAP_GAP as f32 * scale as f32;
        let caps: Vec<f32> = keys
            .split(" / ")
            .map(|k| self.text_width(&keycap_text(k), KEY_FONT_SIZE, scale) + pad)
            .collect();
        caps.iter().sum::<f32>() + sep * (caps.len() - 1) as f32
    }

    /// Draw `keys` as Zed-style keycaps from `x`, vertically centred in the band
    /// `(top, height)`, alternatives (" / ") split by a dim slash. Returns where they end.
    pub(super) fn keycaps(
        &self,
        c: &mut Canvas,
        keys: &str,
        x: f32,
        (top, height): (i32, i32),
        scale: f64,
        p: &Palette,
    ) -> f32 {
        let s = scale as f32;
        let (cap_h, border) = ((20.0 * s).round(), (s.round()).max(1.0));
        let cap_y = (top as f32 + (height as f32 - cap_h) / 2.0).round();
        let band = (cap_y as i32, cap_h as i32);
        let mut pen = x;
        for (i, key) in keys.split(" / ").enumerate() {
            let key = &*keycap_text(key);
            if i > 0 {
                pen += CAP_GAP as f32 * s;
                self.text(
                    c,
                    "/",
                    KEY_FONT_SIZE,
                    pen,
                    f32::MAX,
                    band,
                    scale,
                    p.placeholder,
                );
                pen += self.text_width("/", KEY_FONT_SIZE, scale) + CAP_GAP as f32 * s;
            }
            let w =
                (self.text_width(key, KEY_FONT_SIZE, scale) + 2.0 * CAP_PADDING as f32 * s).round();
            let (cx, r) = (pen.round(), CAP_RADIUS * s);
            c.rounded(cx, cap_y, w, cap_h, r, p.border);
            c.rounded(
                cx + border,
                cap_y + border,
                w - 2.0 * border,
                cap_h - 2.0 * border,
                r - border,
                p.element,
            );
            self.text(
                c,
                key,
                KEY_FONT_SIZE,
                cx + CAP_PADDING as f32 * s,
                f32::MAX,
                band,
                scale,
                p.muted,
            );
            pen = cx + w;
        }
        pen
    }

    pub(super) fn text_width(&self, text: &str, size: f32, scale: f64) -> f32 {
        let Some(font) = &self.font else { return 0.0 };
        width_in(font, text, size, scale)
    }

    /// The bold UI font, else the UI font.
    fn bold_font(&self) -> Option<&FontVec> {
        self.bold.as_ref().or(self.font.as_ref())
    }

    /// Width of `text` in the bold UI font, in physical pixels.
    pub(super) fn bold_width(&self, text: &str, size: f32, scale: f64) -> f32 {
        let Some(font) = self.bold_font() else {
            return 0.0;
        };
        width_in(font, text, size, scale)
    }

    /// Draw `text` centred in a logical rectangle.
    pub(super) fn centered(
        &self,
        c: &mut Canvas,
        text: &str,
        size: f32,
        rect: Rectangle<i32, Logical>,
        scale: f64,
        color: Rgb,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let x = px(rect.loc.x) as f32
            + (px(rect.size.w) as f32 - self.text_width(text, size, scale)) / 2.0;
        let band = (px(rect.loc.y), px(rect.size.h));
        self.text(
            c,
            text,
            size,
            x,
            px(rect.loc.x + rect.size.w) as f32,
            band,
            scale,
            color,
        );
    }

    /// A badge of `fill` with `count` (past `max`, "`max`+") on the top
    /// right corner of the button at `rect`, sticking `out` past it, in a
    /// ring of `ground` setting it off the button's icon.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn count_badge(
        &self,
        c: &mut Canvas,
        rect: Rect,
        count: usize,
        max: usize,
        out: i32,
        (ground, fill, ink): (Rgb, Rgb, Rgb),
        scale: f64,
    ) {
        let s = scale as f32;
        let label = if count > max {
            format!("{max}+")
        } else {
            count.to_string()
        };
        let text_w = (self.text_width(&label, TINY_FONT_SIZE, scale) / s).ceil() as i32;
        let w = (text_w + 2 * BADGE_PAD).max(BADGE_HEIGHT);
        let badge = Rectangle::new(
            (
                rect.loc.x + rect.size.w + out - w,
                (rect.loc.y - out).max(0),
            )
                .into(),
            (w, BADGE_HEIGHT).into(),
        );
        let pill = |c: &mut Canvas, rect: Rect, color: Rgb| {
            let (x, y, w, h) = rect_px(rect, scale);
            c.rounded(x, y, w, h, h / 2.0, color);
        };
        let ring = Rectangle::new(
            (badge.loc.x - 1, badge.loc.y - 1).into(),
            (w + 2, BADGE_HEIGHT + 2).into(),
        );
        pill(c, ring, ground);
        pill(c, badge, fill);
        self.centered(c, &label, TINY_FONT_SIZE, badge, scale, ink);
    }

    /// The marks after a branch: a red cross for a merge, rebase or the
    /// like stopped half-way or a conflicted file, a dot for uncommitted
    /// changes, then arrows
    /// with the commits ahead of and behind the upstream. Drawn from `x` in
    /// the band `(top, height)`, as far as `max_x` leaves room. Returns where
    /// they end.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn git_marks(
        &self,
        c: &mut Canvas,
        git: &GitStatus,
        x: f32,
        max_x: f32,
        (top, height): (i32, i32),
        scale: f64,
        p: &Palette,
    ) -> f32 {
        let s = scale as f32;
        let (w, gap) = (
            (GIT_MARK_WIDTH as f32 * s).round(),
            (GIT_MARK_GAP as f32 * s).round(),
        );
        let mark = |c: &mut Canvas, x: f32, color: Rgb, shape: &dyn Fn(f32, f32) -> f32| {
            let x = x + gap;
            if x + w > max_x {
                return None;
            }
            let (cx, cy) = (x + w / 2.0, top as f32 + height as f32 / 2.0);
            c.paint_in(x as i32, top, w as i32, height, color, |px, py| {
                coverage(shape(px - cx, py - cy))
            });
            Some(x + w)
        };
        let mut x = x;
        if git.op.is_some() || git.changes.iter().any(Change::conflicted) {
            let Some(end) = mark(c, x, p.error, &|x, y| cross(x, y, 0.85 * s) - 0.3 * s) else {
                return x;
            };
            x = end;
        }
        if git.dirty {
            let Some(end) = mark(c, x, p.warning, &|x, y| x.hypot(y) - 2.5 * s) else {
                return x;
            };
            x = end;
        }
        for (count, up) in [(git.ahead, true), (git.behind, false)] {
            if count == 0 {
                continue;
            }
            let Some(end) = mark(c, x, p.muted, &|x, y| arrow(up, x, y, s)) else {
                return x;
            };
            x = self.text(
                c,
                &count.to_string(),
                DETAIL_FONT_SIZE,
                end + s,
                max_x,
                (top, height),
                scale,
                p.muted,
            );
        }
        x
    }

    /// Draw `text` from `x`, vertically centred in the band `(top, height)`, cut
    /// off with "…" before `max_x`. Coordinates are physical pixels. Returns
    /// where it ends.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn text(
        &self,
        c: &mut Canvas,
        text: &str,
        size: f32,
        x: f32,
        max_x: f32,
        (top, height): (i32, i32),
        scale: f64,
        color: Rgb,
    ) -> f32 {
        let Some(font) = &self.font else { return x };
        text_in(font, c, text, size, x, max_x, (top, height), scale, color)
    }

    /// Draw `text` in the bold UI font as `text` does. Returns where it
    /// ends.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn bold_text(
        &self,
        c: &mut Canvas,
        text: &str,
        size: f32,
        x: f32,
        max_x: f32,
        band: (i32, i32),
        scale: f64,
        color: Rgb,
    ) -> f32 {
        let Some(font) = self.bold_font() else {
            return x;
        };
        text_in(font, c, text, size, x, max_x, band, scale, color)
    }

    /// The byte of `line`, drawn by `edit_line` between `x` and `max_x`,
    /// nearest `at` (physical pixels).
    pub(super) fn caret_at(
        &self,
        line: &Line,
        (x, max_x): (f32, f32),
        at: f32,
        size: f32,
        scale: f64,
    ) -> usize {
        let width = |text: &str| self.text_width(text, size, scale);
        let scrolled = (width(&line[..line.caret]) - (max_x - x)).max(0.0);
        let mut pen = x - scrolled;
        for (i, ch) in line.char_indices() {
            let advance = width(ch.encode_utf8(&mut [0; 4]));
            if at < pen + advance / 2.0 {
                return i;
            }
            pen += advance;
        }
        line.len()
    }

    /// Draw `text` from `x` as `text` does, but only the characters wholly
    /// between `min_x` and `max_x`: a line scrolled in a field.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn clipped_text(
        &self,
        c: &mut Canvas,
        text: &str,
        x: f32,
        (min_x, max_x): (f32, f32),
        (top, height): (i32, i32),
        size: f32,
        scale: f64,
        color: Rgb,
    ) {
        let Some(font) = &self.font else { return };
        let scaled = font.as_scaled(PxScale::from(size * scale as f32));
        let cap_height = font
            .outline_glyph(scaled.scaled_glyph('H'))
            .map_or(scaled.ascent(), |o| -o.px_bounds().min.y);
        let baseline = (top as f32 + (height as f32 + cap_height) / 2.0).round();
        let mut pen = x;
        for ch in text.chars() {
            let id = scaled.glyph_id(ch);
            let advance = scaled.h_advance(id);
            if pen + advance > max_x {
                break;
            }
            let glyph = id.with_scale_and_position(scaled.scale(), ab_glyph::point(pen, baseline));
            if pen >= min_x
                && let Some(outline) = font.outline_glyph(glyph)
            {
                let bounds = outline.px_bounds();
                outline.draw(|gx, gy, coverage| {
                    c.blend(
                        bounds.min.x as i32 + gx as i32,
                        bounds.min.y as i32 + gy as i32,
                        color,
                        coverage,
                    );
                });
            }
            pen += advance;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Canvas, Painter};
    use crate::chrome::{Check, FONT_SIZE, Theme};
    use crate::git::{Change, GitStatus, InProgress};
    use crate::text_field::Line;
    use ab_glyph::Font;

    #[test]
    fn a_conflict_or_an_operation_half_done_adds_a_mark_before_the_dirty_dot() {
        let painter = Painter::new();
        let p = Theme::Dark.palette();
        let end = |git: &GitStatus| {
            let mut c = Canvas::new((200, 30).into());
            painter.git_marks(&mut c, git, 0.0, 200.0, (0, 30), 1.0, p)
        };
        let dirty = GitStatus {
            dirty: true,
            ..GitStatus::default()
        };
        let conflicted = GitStatus {
            changes: vec![Change {
                path: "a.rs".into(),
                orig: None,
                code: 'U',
            }],
            ..dirty.clone()
        };
        let merging = GitStatus {
            op: Some(InProgress::Merge),
            ..dirty.clone()
        };
        assert!(end(&conflicted) > end(&dirty));
        assert_eq!(end(&merging), end(&conflicted));
    }

    #[test]
    fn rows_are_lit_one_way_and_buttons_are_as_wide_as_their_labels() {
        let p = crate::chrome::Theme::Dark.palette();
        assert_eq!(super::row_fill(p, true, 0.0), Some(p.selected));
        assert_eq!(super::row_fill(p, true, 1.0), Some(p.selected));
        assert_eq!(super::row_fill(p, false, 1.0), Some(p.element));
        assert_eq!(super::row_fill(p, false, 0.0), None);
        assert_eq!(super::ButtonStyle::from(true), super::ButtonStyle::Primary);
        assert_eq!(super::ButtonStyle::from(false), super::ButtonStyle::Plain);
        assert_eq!(super::keycap_text("Delete"), "Del");
        assert_eq!(super::keycap_text("Shift+Delete"), "Shift+Del");
        assert_eq!(super::keycap_text("KP_Delete"), "KP_Delete");
        let painter = Painter::new();
        if painter.font.is_none() {
            return;
        }
        assert_eq!(painter.button_width("Quit"), super::BUTTON_MIN_WIDTH);
        assert!(painter.button_width("Open in new gerdr") > painter.button_width("Switch"));
    }

    #[test]
    fn a_checkbox_ends_after_its_box_whatever_its_state() {
        let painter = Painter::new();
        let p = Theme::Light.palette();
        let rect = smithay::utils::Rectangle::new((10, 0).into(), (100, 24).into());
        for state in [Check::Off, Check::On, Check::Partial] {
            let mut c = Canvas::new((120, 24).into());
            let end = painter.checkbox(&mut c, rect, state, false, 1.0, p);
            assert_eq!(end, (10 + super::CHECKBOX_SIZE) as f32);
        }
    }

    #[test]
    fn what_the_monospace_font_lacks_is_drawn_from_another() {
        let painter = Painter::new();
        // `❯`: what Claude Code marks the chosen answer with.
        let lacked = '❯';
        let Some(mono) = painter.mono.as_ref() else {
            return;
        };
        if mono.glyph_id(lacked).0 != 0 || super::fallbacks(&[lacked])[0].is_none() {
            return;
        }
        let mut c = Canvas::new((60, 30).into());
        let p = Theme::Dark.palette();
        painter.mono_text(&mut c, "❯", 14.0, 4.0, 60.0, (0, 30), 1.0, p.text);
        assert!(c.px.iter().any(|&b| b != 0));
    }

    #[test]
    fn a_click_puts_the_caret_by_the_nearest_character() {
        let painter = Painter::new();
        if painter.font.is_none() {
            return;
        }
        let width = |text: &str| painter.text_width(text, super::FONT_SIZE, 1.0);
        let line = Line::from("abc def");
        let (x, max_x) = (100.0, 1000.0);
        let at = |px: f32| painter.caret_at(&line, (x, max_x), px, FONT_SIZE, 1.0);
        assert_eq!(at(0.0), 0);
        assert_eq!(at(x + width("ab") + 1.0), 2);
        assert_eq!(at(x + width("abc ") - 1.0), 4);
        assert_eq!(at(2000.0), 7);
        // Scrolled to keep the caret in view, the text starts left of `x`.
        let long = Line::from("a long line typed past the end of a narrow field");
        let narrow = (x, x + width("a long line"));
        let end = painter.caret_at(&long, narrow, narrow.1, FONT_SIZE, 1.0);
        assert_eq!(end, long.len());
        assert!(painter.caret_at(&long, narrow, x, FONT_SIZE, 1.0) > 0);
    }
}

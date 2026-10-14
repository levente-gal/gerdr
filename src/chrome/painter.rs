//! Drawing text, fields, keycaps and buttons into a `Canvas` with the
//! system font.

use super::{
    BUTTON_RADIUS, CAP_GAP, CAP_PADDING, CAP_RADIUS, CARET_HEIGHT, CARET_WIDTH, Canvas,
    DETAIL_FONT_SIZE, FOCUS_GAP, FOCUS_RING, FONT_SIZE, GIT_MARK_GAP, GIT_MARK_WIDTH, HINT_GAP,
    HINT_KEY_GAP, ICON_SIZE, KEY_FONT_SIZE, PADDING, Palette, Rect, Rgb, SMALL_FONT_SIZE, arrow,
    coverage, mix,
};
use crate::git::GitStatus;
use crate::text_field::Line;
use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use smithay::utils::{Logical, Rectangle, Size};

pub(super) struct Painter {
    pub(super) font: Option<FontVec>,
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

/// Pixel size of a buffer covering `size` logical pixels.
pub(super) fn physical(size: Size<i32, Logical>, scale: f64) -> Size<i32, Logical> {
    let px = |v: i32| ((v as f64 * scale).round() as i32).max(1);
    (px(size.w), px(size.h)).into()
}

pub(super) fn find_font() -> Option<String> {
    let out = std::process::Command::new("fc-match")
        .args(["-f", "%{file}", "sans-serif"])
        .output()
        .ok()?;
    let path = String::from_utf8(out.stdout).ok()?;
    (!path.is_empty()).then_some(path)
}

impl Painter {
    pub(super) fn new() -> Self {
        let font = find_font().and_then(|path| {
            let data = std::fs::read(&path).ok()?;
            FontVec::try_from_vec(data).ok()
        });
        if font.is_none() {
            tracing::warn!("chrome: no usable font found, labels will be blank");
        }
        Self { font }
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
                FONT_SIZE,
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
        let width = |text: &str| self.text_width(text, FONT_SIZE, scale);
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
            self.clipped_text(c, line, start, (x, max_x), field_band, scale, p.text);
        } else {
            self.text(c, line, FONT_SIZE, x, max_x, field_band, scale, p.text);
        }
        if focused {
            let nudge = if line.caret > 0 { px(1) as f32 } else { 0.0 };
            caret(c, start + to_caret + nudge, field_band, scale, p.accent);
        }
    }

    /// Key hints as keycaps, each followed by what it does, centred in
    /// `rect`: as many of the first as fit.
    pub(super) fn hints(
        &self,
        c: &mut Canvas,
        hints: &[(&str, &str)],
        rect: Rect,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        let key_gap = (HINT_KEY_GAP as f32 * s).round();
        let gap = (HINT_GAP as f32 * s).round();
        let (x, _, w, _) = rect_px(rect, scale);
        let mut width = -gap;
        for (n, (keys, action)) in hints.iter().enumerate() {
            let more = gap
                + self.keycaps_width(keys, scale)
                + key_gap
                + self.text_width(action, SMALL_FONT_SIZE, scale);
            if width + more > w {
                return self.hints(c, &hints[..n], rect, scale, p);
            }
            width += more;
        }
        let width = width.max(0.0);
        let hints_band = band(rect, scale);
        let mut pen = (x + (w - width) / 2.0).round();
        for (keys, action) in hints {
            let end = self.keycaps(c, keys, pen, hints_band, scale, p);
            pen = self.text(
                c,
                action,
                SMALL_FONT_SIZE,
                end + key_gap,
                f32::MAX,
                hints_band,
                scale,
                p.muted,
            ) + gap;
        }
    }

    /// A button in `rect` like the quit dialog's, labelled `label`: the
    /// `primary` one in the accent. `(hovered, focused)` lightens it and
    /// rings it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn button(
        &self,
        c: &mut Canvas,
        label: &str,
        rect: Rect,
        (hovered, focused): (bool, bool),
        primary: bool,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        let (x, y, w, h) = rect_px(rect, scale);
        let (b, r) = (((scale).round() as f32).max(1.0), BUTTON_RADIUS * s);
        if focused {
            focus_ring(c, rect, BUTTON_RADIUS, p.elevated, scale, p.accent);
        }
        let color = if primary {
            let fill = if hovered {
                mix(p.accent, p.text, 0.12)
            } else {
                p.accent
            };
            c.rounded(x, y, w, h, r, fill);
            p.on_accent
        } else {
            let fill = if hovered { p.selected } else { p.element };
            c.rounded(x, y, w, h, r, p.border);
            c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, fill);
            p.text
        };
        self.centered(c, label, FONT_SIZE, rect, scale, color);
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
            .map(|k| self.text_width(k, KEY_FONT_SIZE, scale) + pad)
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
        let font = font.as_scaled(PxScale::from(size * scale as f32));
        text.chars().map(|c| font.h_advance(font.glyph_id(c))).sum()
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

    /// Draw an icon glyph at `ICON_SIZE`, centred on its own ink in a logical
    /// rectangle ("×" and "+" sit off the cap-height centre `text` uses).
    pub(super) fn icon(
        &self,
        c: &mut Canvas,
        ch: char,
        rect: Rectangle<i32, Logical>,
        scale: f64,
        color: Rgb,
    ) {
        let Some(font) = &self.font else { return };
        let scaled = font.as_scaled(PxScale::from(ICON_SIZE * scale as f32));
        let Some(outline) = font.outline_glyph(scaled.scaled_glyph(ch)) else {
            return;
        };
        let ink = outline.px_bounds();
        let px = |v: i32| (v as f64 * scale) as f32;
        let x = (px(rect.loc.x) + (px(rect.size.w) - ink.width()) / 2.0 - ink.min.x).round();
        let y = (px(rect.loc.y) + (px(rect.size.h) - ink.height()) / 2.0 - ink.min.y).round();
        let glyph = scaled.scaled_glyph(ch);
        let glyph = ab_glyph::Glyph {
            position: ab_glyph::point(x, y),
            ..glyph
        };
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

    /// The marks after a branch: a dot for uncommitted changes, then arrows
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
        let scaled = font.as_scaled(PxScale::from(size * scale as f32));
        // Centre on the cap height; the font's ascent/descent include line gap padding.
        let cap_height = font
            .outline_glyph(scaled.scaled_glyph('H'))
            .map_or(scaled.ascent(), |o| -o.px_bounds().min.y);
        let baseline = (top as f32 + (height as f32 + cap_height) / 2.0).round();

        let (mut shown, mut cut) = (text, false);
        if x + self.text_width(text, size, scale) > max_x {
            // The longest start of `text` that fits with an ellipsis after it.
            let ellipsis = self.text_width("…", size, scale);
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

    /// The byte of `line`, drawn by `edit_line` between `x` and `max_x`,
    /// nearest `at` (physical pixels).
    pub(super) fn caret_at(
        &self,
        line: &Line,
        (x, max_x): (f32, f32),
        at: f32,
        scale: f64,
    ) -> usize {
        let width = |text: &str| self.text_width(text, FONT_SIZE, scale);
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
        scale: f64,
        color: Rgb,
    ) {
        let Some(font) = &self.font else { return };
        let scaled = font.as_scaled(PxScale::from(FONT_SIZE * scale as f32));
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
    use super::Painter;
    use crate::text_field::Line;

    #[test]
    fn a_click_puts_the_caret_by_the_nearest_character() {
        let painter = Painter::new();
        if painter.font.is_none() {
            return;
        }
        let width = |text: &str| painter.text_width(text, super::FONT_SIZE, 1.0);
        let line = Line::from("abc def");
        let (x, max_x) = (100.0, 1000.0);
        let at = |px: f32| painter.caret_at(&line, (x, max_x), px, 1.0);
        assert_eq!(at(0.0), 0);
        assert_eq!(at(x + width("ab") + 1.0), 2);
        assert_eq!(at(x + width("abc ") - 1.0), 4);
        assert_eq!(at(2000.0), 7);
        // Scrolled to keep the caret in view, the text starts left of `x`.
        let long = Line::from("a long line typed past the end of a narrow field");
        let narrow = (x, x + width("a long line"));
        let end = painter.caret_at(&long, narrow, narrow.1, 1.0);
        assert_eq!(end, long.len());
        assert!(painter.caret_at(&long, narrow, x, 1.0) > 0);
    }
}

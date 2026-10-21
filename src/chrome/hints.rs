//! Key hints: a keycap per key, then what it does, in a row or in a
//! footer's rows. Each is a button too: a click presses its key, as in the
//! repositories panel's footer.

use super::{
    BUTTON_RADIUS, CAP_GAP, CAP_PADDING, Canvas, FOOTER_PAD, FOOTER_ROW, HELP_PADDING, HINT_GAP,
    HINT_KEY_GAP, KEY_FONT_SIZE, Painter, Palette, Rect, SMALL_FONT_SIZE, band, footer_rows,
    measure, rect_px,
};
use smithay::utils::{Logical, Point, Rectangle};

/// How high the ground under a hovered hint is at most.
const HINT_HOVER_HEIGHT: i32 = 28;

/// Hints as the painter and the hit tests take them, from a view's owned
/// ones.
pub fn hint_strs(hints: &[(String, String)]) -> Vec<(&str, &str)> {
    hints
        .iter()
        .map(|(keys, action)| (keys.as_str(), action.as_str()))
        .collect()
}

/// How wide `text` is at `size` in logical pixels; without a font, a
/// guess on the wide side.
fn extent(text: &str, size: f32) -> f32 {
    measure(text, size).unwrap_or(text.chars().count() as f32 * size * 0.6)
}

/// How wide the keycaps of `keys` are: one per key offered (" / "
/// between), a slash between them.
fn keycaps_extent(keys: &str) -> f32 {
    let sep = extent("/", KEY_FONT_SIZE) + 2.0 * CAP_GAP as f32;
    let caps: Vec<f32> = keys
        .split(" / ")
        .map(|k| extent(k, KEY_FONT_SIZE) + 2.0 * CAP_PADDING as f32)
        .collect();
    caps.iter().sum::<f32>() + sep * (caps.len() - 1) as f32
}

/// How wide a hint is: its keycaps, then what it does.
fn hint_extent((keys, action): (&str, &str)) -> f32 {
    keycaps_extent(keys) + HINT_KEY_GAP as f32 + extent(action, SMALL_FONT_SIZE)
}

/// Where `hints` go centred in the row `rect`, as (index into `hints`,
/// rect): as many of the first as fit. Each reaches halfway to its
/// neighbours, so a click between two lands on one of them.
pub fn hint_spots(hints: &[(&str, &str)], rect: Rect) -> Vec<(usize, Rect)> {
    let gap = HINT_GAP as f32;
    let (mut widths, mut total) = (Vec::new(), -gap);
    for hint in hints {
        let w = hint_extent(*hint);
        if total + gap + w > rect.size.w as f32 {
            break;
        }
        total += gap + w;
        widths.push(w);
    }
    let mut x = rect.loc.x as f32 + (rect.size.w as f32 - total.max(0.0)) / 2.0;
    widths
        .into_iter()
        .enumerate()
        .map(|(i, w)| {
            let left = (x - gap / 2.0).round() as i32;
            let right = (x + w + gap / 2.0).round() as i32;
            x += w + gap;
            let spot = Rectangle::new(
                (left, rect.loc.y).into(),
                (right - left, rect.size.h).into(),
            );
            (i, spot)
        })
        .collect()
}

/// Where `hints` go in the footer `footer` (see `footer_rows`), as
/// (index into `hints`, rect): in rows in from its sides, a row taking
/// hints until the next would run past its end; the last row takes the
/// rest, as many as fit.
pub fn footer_hint_spots(hints: &[(&str, &str)], footer: Rect) -> Vec<(usize, Rect)> {
    let max_rows = footer_rows(hints.len()) as usize;
    let w = (footer.size.w - 2 * HELP_PADDING).max(0);
    let gap = HINT_GAP as f32;
    let mut rows: Vec<std::ops::Range<usize>> = Vec::new();
    let (mut start, mut width) = (0, -gap);
    for (i, hint) in hints.iter().enumerate() {
        let more = gap + hint_extent(*hint);
        if width + more > w as f32 && i > start && rows.len() + 1 < max_rows.max(1) {
            rows.push(start..i);
            (start, width) = (i, -gap);
        }
        width += more;
    }
    rows.push(start..hints.len());
    rows.into_iter()
        .enumerate()
        .flat_map(|(r, range)| {
            let row = Rectangle::new(
                (
                    footer.loc.x + HELP_PADDING,
                    footer.loc.y + FOOTER_PAD + r as i32 * FOOTER_ROW,
                )
                    .into(),
                (w, FOOTER_ROW).into(),
            );
            let first = range.start;
            hint_spots(&hints[range], row)
                .into_iter()
                .map(move |(i, spot)| (first + i, spot))
        })
        .collect()
}

/// Which of `hints` at their `spots` is under `pos`, if one is that can
/// be pressed: not one offering keys to choose from (`Up / Down`).
pub fn hint_at(
    hints: &[(&str, &str)],
    spots: &[(usize, Rect)],
    pos: Point<f64, Logical>,
) -> Option<usize> {
    spots
        .iter()
        .find(|(_, spot)| spot.to_f64().contains(pos))
        .map(|(i, _)| *i)
        .filter(|i| hints.get(*i).is_some_and(|(keys, _)| !keys.contains(" / ")))
}

impl Painter {
    /// `hints` at their `spots` (see `hint_spots`): keycaps, then what
    /// each does; the one at `hover` on a lighter ground, what it does
    /// brighter.
    pub(super) fn hints(
        &self,
        c: &mut Canvas,
        hints: &[(&str, &str)],
        spots: &[(usize, Rect)],
        hover: Option<usize>,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        let key_gap = (HINT_KEY_GAP as f32 * s).round();
        for &(i, spot) in spots {
            let Some((keys, action)) = hints.get(i) else {
                continue;
            };
            let (x, y, w, h) = rect_px(spot, scale);
            let hovered = hover == Some(i);
            if hovered {
                let ground = h.min((HINT_HOVER_HEIGHT as f32 * s).round());
                let top = (y + (h - ground) / 2.0).round();
                c.rounded(x, top, w, ground, BUTTON_RADIUS * s, p.selected);
            }
            let hint_band = band(spot, scale);
            let pen = (x + HINT_GAP as f32 * s / 2.0).round();
            let end = self.keycaps(c, keys, pen, hint_band, scale, p);
            let color = if hovered { p.text } else { p.muted };
            self.text(
                c,
                action,
                SMALL_FONT_SIZE,
                end + key_gap,
                f32::MAX,
                hint_band,
                scale,
                color,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{footer_hint_spots, hint_at, hint_spots};
    use crate::chrome::{FOOTER_ROW, Rect, footer_height};
    use smithay::utils::Rectangle;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    const HINTS: [(&str, &str); 8] = [
        ("Enter", "open"),
        ("Ctrl+N", "new"),
        ("Ctrl+R", "rename"),
        ("Ctrl+E", "commands"),
        ("Ctrl+O", "project file"),
        ("Ctrl+S", "stop"),
        ("Del", "forget"),
        ("Esc", "close"),
    ];

    #[test]
    fn a_row_is_centred_and_drops_what_does_not_fit() {
        let row = rect(100, 50, 600, 24);
        let spots = hint_spots(&HINTS[..3], row);
        assert_eq!(spots.iter().map(|(i, _)| *i).collect::<Vec<_>>(), [0, 1, 2]);
        // In order, touching, in the row, about as much room either side.
        for pair in spots.windows(2) {
            let (a, b) = (pair[0].1, pair[1].1);
            assert!((b.loc.x - (a.loc.x + a.size.w)).abs() <= 1, "{a:?} {b:?}");
        }
        let (first, last) = (spots[0].1, spots[2].1);
        let left = first.loc.x - row.loc.x;
        let right = row.loc.x + row.size.w - (last.loc.x + last.size.w);
        assert!((left - right).abs() <= 2, "{left} {right}");
        assert!(spots.iter().all(|(_, s)| s.loc.y == 50 && s.size.h == 24));

        let narrow = hint_spots(&HINTS, rect(0, 0, 200, 24));
        assert!(!narrow.is_empty() && narrow.len() < HINTS.len());
        assert!(hint_spots(&HINTS, rect(0, 0, 10, 24)).is_empty());
    }

    #[test]
    fn a_footer_wraps_its_hints_and_keeps_their_order() {
        for w in [360, 480, 640, 2000] {
            let footer = rect(10, 300, w, footer_height(HINTS.len()));
            let spots = footer_hint_spots(&HINTS, footer);
            let order: Vec<usize> = spots.iter().map(|(i, _)| *i).collect();
            let mut sorted = order.clone();
            sorted.sort();
            assert_eq!(order, sorted, "{w}");
            // Every one shown but on the last row, inside the footer.
            let rows: std::collections::BTreeSet<i32> =
                spots.iter().map(|(_, s)| s.loc.y).collect();
            assert!(rows.len() <= 2, "{w}");
            for (_, spot) in &spots {
                assert!(spot.loc.y + spot.size.h <= footer.loc.y + footer.size.h);
                assert_eq!(spot.size.h, FOOTER_ROW);
            }
            if w >= 640 {
                assert_eq!(order, (0..HINTS.len()).collect::<Vec<_>>(), "{w}");
            }
        }
    }

    #[test]
    fn a_click_finds_the_hint_under_it_but_not_a_choice_of_keys() {
        let hints = [("Up / Down", "select"), ("Enter", "open")];
        let spots = hint_spots(&hints, rect(0, 0, 600, 24));
        let centre = |r: Rect| (r.loc.x as f64 + r.size.w as f64 / 2.0, 12.0).into();
        assert_eq!(hint_at(&hints, &spots, centre(spots[1].1)), Some(1));
        assert_eq!(hint_at(&hints, &spots, centre(spots[0].1)), None);
        assert_eq!(hint_at(&hints, &spots, (1.0, 12.0).into()), None);
    }
}

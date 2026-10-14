//! The context menu: a popup under its anchor with a row per item, each
//! with the key doing the same at its right (see `crate::menu` for what
//! opens one and what its items do).

use super::{
    Canvas, FONT_SIZE, MENU_ITEM_HEIGHT, MENU_PADDING, MENU_RADIUS, MENU_WIDTH, PADDING, Painter,
    Palette, Rect, SHADOW, SMALL_FONT_SIZE, physical,
};
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// How much wider a menu is when its rows carry key hints.
const MENU_HINT_ROOM: i32 = 80;
/// The gap between the menu and its anchor.
const MENU_GAP: i32 = 2;
/// The gap between a row's label and its key hint.
const MENU_HINT_GAP: i32 = 16;

/// One row of a menu.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuRow {
    pub label: String,
    /// The key that does the same, at the right.
    pub hint: Option<String>,
    pub enabled: bool,
}

/// What a menu shows: its rows, under `anchor` (window coordinates), and
/// the row under the pointer or picked with the arrow keys.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuView {
    pub anchor: Rect,
    pub rows: Vec<MenuRow>,
    pub hover: Option<usize>,
}

/// How wide a menu of `rows` is: wider when any row has a key hint.
pub fn menu_width(rows: &[MenuRow]) -> i32 {
    if rows.iter().any(|row| row.hint.is_some()) {
        MENU_WIDTH + MENU_HINT_ROOM
    } else {
        MENU_WIDTH
    }
}

/// A menu of `rows` just below `anchor` (window coordinates; it may reach
/// over the apps), kept on an output of `output`: above the anchor when
/// there is no room below it, moved left when it would reach past the
/// right edge. The panel and one rect per item.
pub fn menu_layout(
    anchor: Rect,
    rows: &[MenuRow],
    output: Size<i32, Logical>,
) -> (Rect, Vec<Rect>) {
    let w = menu_width(rows);
    let h = 2 * MENU_PADDING + rows.len() as i32 * MENU_ITEM_HEIGHT;
    let below = anchor.loc.y + anchor.size.h + MENU_GAP;
    let top = if below + h > output.h && anchor.loc.y - MENU_GAP - h >= 0 {
        anchor.loc.y - MENU_GAP - h
    } else {
        below.min((output.h - h).max(0))
    };
    let x = anchor.loc.x.min((output.w - w).max(0));
    let panel = Rectangle::new((x, top).into(), (w, h).into());
    let items = (0..rows.len() as i32)
        .map(|i| {
            let y = panel.loc.y + MENU_PADDING + i * MENU_ITEM_HEIGHT;
            Rectangle::new(
                (x + MENU_PADDING, y).into(),
                (w - 2 * MENU_PADDING, MENU_ITEM_HEIGHT).into(),
            )
        })
        .collect();
    (panel, items)
}

impl Painter {
    /// The menu inside a margin of `SHADOW` holding its drop shadow, the
    /// `hover` row highlighted, each row's key hint at its right and the
    /// disabled rows muted.
    pub(super) fn menu(&self, view: &MenuView, scale: f64, p: &Palette) -> MemoryRenderBuffer {
        // The panel at the origin: an anchor whose bottom edge is a gap above
        // it, on an output tall enough that it hangs below.
        let anchor = Rectangle::new((0, -MENU_GAP).into(), (0, 0).into());
        let (panel, items) = menu_layout(anchor, &view.rows, (i32::MAX / 2, i32::MAX / 2).into());
        let outer = Size::from((panel.size.w + 2 * SHADOW, panel.size.h + 2 * SHADOW));
        let mut c = Canvas::new(physical(outer, scale));
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        c.popup(panel.size, MENU_RADIUS, scale, p);
        // Item rects relative to the buffer, which starts `SHADOW` before the panel.
        let shift = Point::from((SHADOW - panel.loc.x, SHADOW - panel.loc.y));
        for (i, (item, row)) in items.iter().zip(&view.rows).enumerate() {
            let item = Rectangle::new(item.loc + shift, item.size);
            let (x, y, w, h) = (
                px(item.loc.x),
                px(item.loc.y),
                px(item.size.w),
                px(item.size.h),
            );
            if view.hover == Some(i) && row.enabled {
                c.rounded(
                    x as f32,
                    y as f32,
                    w as f32,
                    h as f32,
                    (MENU_RADIUS - 2.0) * s,
                    p.selected,
                );
            }
            let color = if row.enabled { p.text } else { p.muted };
            let (left, right) = (x as f32 + PADDING * s, (x + w) as f32 - PADDING * s);
            let mut label_end = right;
            if let Some(hint) = &row.hint {
                let hint_w = self.text_width(hint, SMALL_FONT_SIZE, scale);
                let hint_x = (right - hint_w).max(left);
                self.text(
                    &mut c,
                    hint,
                    SMALL_FONT_SIZE,
                    hint_x,
                    right,
                    (y, h),
                    scale,
                    p.muted,
                );
                label_end = hint_x - MENU_HINT_GAP as f32 * s;
            }
            self.text(
                &mut c,
                &row.label,
                FONT_SIZE,
                left,
                label_end,
                (y, h),
                scale,
                color,
            );
        }
        c.into_buffer(false)
    }
}

#[cfg(test)]
mod tests {
    use super::{MENU_HINT_ROOM, MenuRow, menu_layout, menu_width};
    use crate::chrome::{BAR_HEIGHT, MENU_ITEM_HEIGHT, MENU_PADDING, MENU_WIDTH, Rect};

    fn rows(n: usize, hint: bool) -> Vec<MenuRow> {
        (0..n)
            .map(|i| MenuRow {
                label: format!("Item {i}"),
                hint: hint.then(|| "Ctrl+B x".into()),
                enabled: true,
            })
            .collect()
    }

    #[test]
    fn the_menu_hangs_under_its_anchor() {
        let anchor = Rect::new((150, 0).into(), (28, BAR_HEIGHT).into());
        let output = (1200, 800).into();
        let (panel, items) = menu_layout(anchor, &rows(3, false), output);
        assert_eq!(panel.loc, (150, BAR_HEIGHT + 2).into());
        assert_eq!(panel.size.w, MENU_WIDTH);
        assert_eq!(panel.size.h, 2 * MENU_PADDING + 3 * MENU_ITEM_HEIGHT);
        assert_eq!(items.len(), 3);
        for (i, item) in items.iter().enumerate() {
            assert_eq!(
                item.loc.y,
                panel.loc.y + MENU_PADDING + i as i32 * MENU_ITEM_HEIGHT
            );
            assert_eq!(item.size.w, MENU_WIDTH - 2 * MENU_PADDING);
        }
        assert_eq!(menu_layout(anchor, &[], output).1, Vec::<Rect>::new());
    }

    #[test]
    fn hints_make_the_menu_wider() {
        assert_eq!(menu_width(&rows(2, false)), MENU_WIDTH);
        assert_eq!(menu_width(&rows(2, true)), MENU_WIDTH + MENU_HINT_ROOM);
    }

    #[test]
    fn the_menu_stays_on_the_output() {
        let output = (800, 600).into();
        // Near the bottom it opens upwards.
        let low = Rect::new((100, 570).into(), (28, 20).into());
        let (panel, _) = menu_layout(low, &rows(4, false), output);
        assert_eq!(panel.loc.y + panel.size.h, 570 - 2);
        // Near the right edge it moves left.
        let right = Rect::new((780, 0).into(), (28, 20).into());
        let (panel, _) = menu_layout(right, &rows(2, true), output);
        assert_eq!(panel.loc.x + panel.size.w, 800);
    }
}

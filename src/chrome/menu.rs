//! The context menu: a popup under its anchor with a row per item, each
//! with the key doing the same at its right (see `crate::menu` for what
//! opens one and what its items do).

use super::{
    FONT_SIZE, MENU_ITEM_HEIGHT, MENU_PADDING, MENU_WIDTH, PADDING, POPUP_RADIUS, Painter, Palette,
    Rect, SHADOW, SMALL_FONT_SIZE,
};
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// How much wider a menu is when its rows carry key hints.
const MENU_HINT_ROOM: i32 = 80;
/// The gap between the menu and its anchor.
const MENU_GAP: i32 = 2;
/// The gap between a row's label and its key hint.
const MENU_HINT_GAP: i32 = 16;
/// How tall a row of a message is: its lines close, as a paragraph's.
const MENU_NOTE_HEIGHT: i32 = 20;
/// How tall a rule between groups of rows is (a note with no text).
const MENU_RULE_HEIGHT: i32 = 9;

/// One row of a menu.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuRow {
    pub label: String,
    /// The key that does the same, at the right.
    pub hint: Option<String>,
    pub enabled: bool,
    /// A line of a message, not a choice: shorter, its lines read as one.
    pub note: bool,
}

/// How tall `row` is in its menu.
fn row_height(row: &MenuRow) -> i32 {
    match row.note {
        true if row.label.is_empty() => MENU_RULE_HEIGHT,
        true => MENU_NOTE_HEIGHT,
        false => MENU_ITEM_HEIGHT,
    }
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

/// How many rows a menu has room for on an output `height` high: a menu
/// with more shows only these (see `crate::menu::ContextMenu`).
pub fn menu_rows_fitting(height: i32) -> usize {
    ((height - 2 * MENU_PADDING) / MENU_ITEM_HEIGHT).max(1) as usize
}

/// A menu of `rows` just below `anchor` (window coordinates; it may reach
/// over the apps), kept on an output of `output`: above the anchor when
/// there is no room below it, else on whichever side has more room, moved
/// up or down as far as it takes to stay on the output, and moved left
/// when it would reach past the right edge. The panel and one rect per
/// item.
pub fn menu_layout(
    anchor: Rect,
    rows: &[MenuRow],
    output: Size<i32, Logical>,
) -> (Rect, Vec<Rect>) {
    let w = menu_width(rows);
    let h = 2 * MENU_PADDING + rows.iter().map(row_height).sum::<i32>();
    let below = anchor.loc.y + anchor.size.h + MENU_GAP;
    let above = anchor.loc.y - MENU_GAP - h;
    let room_below = output.h - below;
    let room_above = anchor.loc.y - MENU_GAP;
    let top = if below + h <= output.h {
        below
    } else if above >= 0 || room_above > room_below {
        // Above it, or as near it as fits, from the top down.
        above.max(0)
    } else {
        below.min((output.h - h).max(0))
    };
    let x = anchor.loc.x.min((output.w - w).max(0));
    let panel = Rectangle::new((x, top).into(), (w, h).into());
    let mut y = panel.loc.y + MENU_PADDING;
    let items = (rows.iter())
        .map(|row| {
            let item = Rectangle::new(
                (x + MENU_PADDING, y).into(),
                (w - 2 * MENU_PADDING, row_height(row)).into(),
            );
            y += row_height(row);
            item
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
        let mut c = self.popup(panel.size, POPUP_RADIUS, scale, p);
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
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
            // A rule between groups: a line across, nothing to pick.
            if row.note && row.label.is_empty() {
                let rule = px(1).max(1);
                c.fill(x, y + (h - rule) / 2, w, rule, p.border);
                continue;
            }
            if view.hover == Some(i) && row.enabled {
                // The row Enter picks: lit as a list's selected row.
                self.row_highlight(&mut c, item, (true, 0.0), scale, p);
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
                note: false,
            })
            .collect()
    }

    #[test]
    fn a_messages_lines_sit_closer_than_rows() {
        let anchor = Rect::new((150, 0).into(), (28, BAR_HEIGHT).into());
        let mut menu = rows(1, false);
        let note = |label: &str| MenuRow {
            label: label.into(),
            hint: None,
            enabled: false,
            note: true,
        };
        menu.splice(0..0, [note("one line"), note("and the next")]);
        let (panel, items) = menu_layout(anchor, &menu, (1200, 800).into());
        let (a, b, row) = (items[0], items[1], items[2]);
        assert_eq!(b.loc.y, a.loc.y + a.size.h);
        assert_eq!(row.loc.y, b.loc.y + b.size.h);
        assert!(a.size.h < row.size.h);
        assert_eq!(row.size.h, MENU_ITEM_HEIGHT);
        assert_eq!(
            panel.size.h,
            2 * MENU_PADDING + 2 * a.size.h + MENU_ITEM_HEIGHT
        );
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
    fn a_menu_with_no_room_either_side_takes_the_roomier_one() {
        let output = (800, 300).into();
        // Ten rows are too tall for either side of an anchor mid-way.
        let mid = Rect::new((100, 140).into(), (28, 20).into());
        let (panel, _) = menu_layout(mid, &rows(8, false), output);
        assert!(
            panel.loc.y >= 0 && panel.loc.y + panel.size.h <= 300,
            "{panel:?}"
        );
        // Low down, it goes up from the anchor's top.
        let low = Rect::new((100, 250).into(), (28, 20).into());
        let (panel, _) = menu_layout(low, &rows(9, false), output);
        assert_eq!(panel.loc.y, 0);
        assert_eq!(super::menu_rows_fitting(300), 9);
        assert_eq!(super::menu_rows_fitting(10), 1);
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

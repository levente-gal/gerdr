//! The context menu: a popup under its anchor with a row per item (see
//! `crate::menu` for what opens one and what its items do).

use super::{
    Canvas, FONT_SIZE, MENU_ITEM_HEIGHT, MENU_PADDING, MENU_RADIUS, MENU_WIDTH, PADDING, Painter,
    Palette, Rect, SHADOW, physical,
};
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Point, Rectangle, Size};

/// One row of a menu.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuRow {
    pub label: String,
    /// The key that does the same, at the right.
    pub hint: Option<String>,
    pub enabled: bool,
}

/// What a menu shows: its rows, under `anchor` (window coordinates), and
/// the row under the pointer.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuView {
    pub anchor: Rect,
    pub rows: Vec<MenuRow>,
    pub hover: Option<usize>,
}

/// A menu of `rows` items just below `anchor` (window coordinates; it may
/// reach over the apps): the panel and one rect per item.
pub fn menu_layout(anchor: Rect, rows: usize) -> (Rect, Vec<Rect>) {
    let x = anchor.loc.x;
    let h = 2 * MENU_PADDING + rows as i32 * MENU_ITEM_HEIGHT;
    let top = anchor.loc.y + anchor.size.h + 2;
    let panel = Rectangle::new((x, top).into(), (MENU_WIDTH, h).into());
    let items = (0..rows as i32)
        .map(|i| {
            let y = panel.loc.y + MENU_PADDING + i * MENU_ITEM_HEIGHT;
            Rectangle::new(
                (x + MENU_PADDING, y).into(),
                (MENU_WIDTH - 2 * MENU_PADDING, MENU_ITEM_HEIGHT).into(),
            )
        })
        .collect();
    (panel, items)
}

impl Painter {
    /// The menu inside a margin of `SHADOW` holding its drop shadow, the
    /// `hover` row highlighted.
    pub(super) fn menu(&self, view: &MenuView, scale: f64, p: &Palette) -> MemoryRenderBuffer {
        // The panel at the origin: an anchor whose bottom edge is 2 above it.
        let anchor = Rectangle::new((0, -2).into(), (0, 0).into());
        let (panel, items) = menu_layout(anchor, view.rows.len());
        let outer = Size::from((panel.size.w + 2 * SHADOW, panel.size.h + 2 * SHADOW));
        let mut c = Canvas::new(physical(outer, scale));
        let px = |v: i32| (v as f64 * scale).round() as i32;
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
            if view.hover == Some(i) {
                c.rounded(
                    x as f32,
                    y as f32,
                    w as f32,
                    h as f32,
                    (MENU_RADIUS - 2.0) * scale as f32,
                    p.selected,
                );
            }
            let text_x = x as f32 + PADDING * scale as f32;
            self.text(
                &mut c,
                &row.label,
                FONT_SIZE,
                text_x,
                (x + w) as f32,
                (y, h),
                scale,
                p.text,
            );
        }
        c.into_buffer(false)
    }
}

#[cfg(test)]
mod tests {
    use super::menu_layout;
    use crate::chrome::{BAR_HEIGHT, MENU_ITEM_HEIGHT, MENU_PADDING, MENU_WIDTH, Rect};

    #[test]
    fn the_menu_hangs_under_its_anchor() {
        let anchor = Rect::new((150, 0).into(), (28, BAR_HEIGHT).into());
        let (panel, items) = menu_layout(anchor, 3);
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
        assert_eq!(menu_layout(anchor, 0).1, Vec::<Rect>::new());
    }
}

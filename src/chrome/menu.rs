//! The menu under the sidebar's terminal button.

use super::{
    BAR_HEIGHT, Canvas, FONT_SIZE, MENU_ITEM_HEIGHT, MENU_ITEMS, MENU_PADDING, MENU_RADIUS,
    MENU_WIDTH, PADDING, Painter, Palette, Rect, SHADOW, physical, sidebar_layout,
};
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Point, Rectangle, Size};

/// The terminal button's menu, below it, for a sidebar `width` wide (window
/// coordinates; it may reach over the apps): the panel and one rect per item.
pub fn terminal_menu(width: i32) -> (Rect, Vec<Rect>) {
    let x = sidebar_layout(&[], &[], width).terminal.loc.x;
    let h = 2 * MENU_PADDING + MENU_ITEMS.len() as i32 * MENU_ITEM_HEIGHT;
    let panel = Rectangle::new((x, BAR_HEIGHT + 2).into(), (MENU_WIDTH, h).into());
    let items = (0..MENU_ITEMS.len() as i32)
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
    /// The terminal button's menu inside a margin of `SHADOW` holding its drop
    /// shadow, `hover` item highlighted.
    pub(super) fn menu(&self, hover: Option<usize>, scale: f64, p: &Palette) -> MemoryRenderBuffer {
        let (panel, items) = terminal_menu(0);
        let outer = Size::from((panel.size.w + 2 * SHADOW, panel.size.h + 2 * SHADOW));
        let mut c = Canvas::new(physical(outer, scale));
        let px = |v: i32| (v as f64 * scale).round() as i32;
        c.popup(panel.size, MENU_RADIUS, scale, p);
        // Item rects relative to the buffer, which starts `SHADOW` before the panel.
        let shift = Point::from((SHADOW - panel.loc.x, SHADOW - panel.loc.y));
        for (i, (item, label)) in items.iter().zip(MENU_ITEMS).enumerate() {
            let item = Rectangle::new(item.loc + shift, item.size);
            let (x, y, w, h) = (
                px(item.loc.x),
                px(item.loc.y),
                px(item.size.w),
                px(item.size.h),
            );
            if hover == Some(i) {
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
                label,
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

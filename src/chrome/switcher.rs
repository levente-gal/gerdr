//! The switcher: its rows, where they go, and how it is drawn.
//!
//! The rows come in sections (Workspaces, Tabs, Agents, …), each under an
//! upper case heading. A row has two lines: what it is on the first, more
//! about it in muted text on the second, with a small pill at the right
//! naming its kind.

use super::{
    CARET_WIDTH, Canvas, FONT_SIZE, FieldSpot, HELP_MARGIN, HELP_PADDING, HELP_RADIUS, MENU_RADIUS,
    PADDING, PILL_HEIGHT, PILL_PADDING, Painter, Palette, ROW_ICON_GAP, ROW_ICON_WIDTH, Rect,
    SHADOW, SMALL_FONT_SIZE, SWITCHER_INPUT, SWITCHER_PADDING, SWITCHER_ROWS, SWITCHER_SECTION,
    SWITCHER_TOP, SWITCHER_WIDTH, TINY_FONT_SIZE, app, folder, folder_plus, icon_sdf, mix, pencil,
    physical, prompt, status_mark, tab, terminal,
};
use crate::WorkspaceKind;
use crate::app_icons::AppIcon;
use crate::herdr::AgentStatus;
use crate::text_field::Line;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// A row: two lines of text.
const ROW_HEIGHT: i32 = 40;
/// A section's heading: the gap above it and its text.
const SECTION_HEIGHT: i32 = SWITCHER_SECTION;
/// The heading's text sits in the lower part of its band.
const SECTION_TEXT: i32 = 22;
/// The gap between the two lines' bands and the row's edges.
const LINE_INSET: i32 = 4;
/// The first line's band; the second takes the rest.
const LABEL_LINE: i32 = 18;
/// The pill at the right of a row and the gap before it.
const PILL_GAP: i32 = 10;

/// The icon of a switcher row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemIcon {
    Workspace(WorkspaceKind),
    Tab,
    Agent(AgentStatus),
    /// A folder to open.
    Folder,
    /// An app or command to launch.
    App,
    /// An app's own icon.
    Picture(AppIcon),
    /// Something gerdr does itself: the shortcuts, the project menu.
    Command,
    /// A file to edit: the settings.
    Settings,
}

/// One row of the switcher, with the positions (in chars) of the characters
/// the query matched in its label and detail. `section` is set on the first
/// row of each section shown: the heading drawn above it.
#[derive(Clone, Debug, PartialEq)]
pub struct SwitcherRow {
    pub icon: ItemIcon,
    pub label: String,
    pub detail: String,
    /// What kind of thing it is, for the pill at the right.
    pub kind: &'static str,
    pub section: Option<&'static str>,
    pub label_hits: Vec<usize>,
    pub detail_hits: Vec<usize>,
}

/// What the switcher shows: the query (or `hint` without one), the rows in
/// view, which of them is selected (none when nothing matches), and which
/// the pointer is over.
#[derive(Clone, Debug, PartialEq)]
pub struct SwitcherView {
    pub query: Line,
    pub hint: &'static str,
    pub rows: Vec<SwitcherRow>,
    pub selected: Option<usize>,
    pub hover: Option<usize>,
}

/// Where the switcher's parts go: the panel, its query field, a rect per
/// row shown, and the section headings with their text.
#[derive(Clone, Debug, PartialEq)]
pub struct SwitcherLayout {
    pub panel: Rect,
    pub input: Rect,
    pub rows: Vec<Rect>,
    pub sections: Vec<(Rect, &'static str)>,
}

/// The switcher showing `view` in the app `area` (window coordinates).
pub fn switcher_geometry(view: &SwitcherView, area: Rect) -> SwitcherLayout {
    let sections: Vec<_> = view.rows.iter().map(|row| row.section).collect();
    switcher_parts(&sections, panel_origin(area), panel_width(area))
}

/// The switcher showing `rows` rows without any section heading in the app
/// `area`: the panel, its query field and the rows, for tests with only a
/// count (`switcher_geometry` knows the headings).
#[cfg(test)]
pub fn switcher_layout(rows: usize, area: Rect) -> (Rect, Rect, Vec<Rect>) {
    let layout = switcher_parts(&vec![None; rows], panel_origin(area), panel_width(area));
    (layout.panel, layout.input, layout.rows)
}

fn panel_width(area: Rect) -> i32 {
    SWITCHER_WIDTH.min(area.size.w - 2 * HELP_MARGIN).max(1)
}

fn panel_origin(area: Rect) -> Point<i32, Logical> {
    (
        area.loc.x + (area.size.w - panel_width(area)) / 2,
        area.loc.y + SWITCHER_TOP,
    )
        .into()
}

/// How many rows the switcher shows at most in the app `area`, keeping
/// room for the headings of `sections` sections.
pub fn switcher_page(area: Rect, sections: usize) -> usize {
    let room = area.size.h - SWITCHER_TOP - SWITCHER_INPUT - 2 * SWITCHER_PADDING - HELP_MARGIN;
    let headings = (sections.min(SWITCHER_ROWS as usize) as i32) * SECTION_HEIGHT;
    ((room - headings) / ROW_HEIGHT).clamp(1, SWITCHER_ROWS) as usize
}

/// `switcher_geometry` for a panel `w` wide at `loc`, with a row per entry
/// of `sections` (the heading over it, if any); it keeps room for one row
/// without any.
pub(super) fn switcher_parts(
    sections: &[Option<&'static str>],
    loc: Point<i32, Logical>,
    w: i32,
) -> SwitcherLayout {
    let mut y = loc.y + SWITCHER_INPUT + SWITCHER_PADDING;
    let x = loc.x + SWITCHER_PADDING;
    let row_w = w - 2 * SWITCHER_PADDING;
    let mut rows = Vec::with_capacity(sections.len());
    let mut headings = Vec::new();
    for section in sections {
        if let Some(label) = section {
            headings.push((
                Rectangle::new((x, y).into(), (row_w, SECTION_HEIGHT).into()),
                *label,
            ));
            y += SECTION_HEIGHT;
        }
        rows.push(Rectangle::new((x, y).into(), (row_w, ROW_HEIGHT).into()));
        y += ROW_HEIGHT;
    }
    if sections.is_empty() {
        y += ROW_HEIGHT;
    }
    let h = y + SWITCHER_PADDING - loc.y;
    SwitcherLayout {
        panel: Rectangle::new(loc, (w, h).into()),
        input: Rectangle::new(loc, (w, SWITCHER_INPUT).into()),
        rows,
        sections: headings,
    }
}

/// Where the text of the switcher's query, in its `input` rect, goes.
pub(super) fn switcher_text(input: Rect, scale: f64) -> (f32, f32) {
    let px = |v: i32| (v as f64 * scale).round() as f32;
    (
        px(input.loc.x + HELP_PADDING),
        px(input.loc.x + input.size.w - HELP_PADDING) - px(CARET_WIDTH + 1),
    )
}

/// The switcher's query field, in the app `area`.
pub fn switcher_field(view: &SwitcherView, area: Rect, scale: f64) -> FieldSpot {
    let input = switcher_geometry(view, area).input;
    FieldSpot {
        area: input,
        text: switcher_text(input, scale),
        line: view.query.clone(),
    }
}

impl Painter {
    /// The switcher `w` wide inside a margin of `SHADOW` holding its drop
    /// shadow: the query (or what to type) with a caret, then the rows
    /// under their section headings.
    pub(super) fn switcher(
        &self,
        view: &SwitcherView,
        w: i32,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let sections: Vec<_> = view.rows.iter().map(|row| row.section).collect();
        let layout = switcher_parts(&sections, (SHADOW, SHADOW).into(), w);
        let (panel, input) = (layout.panel, layout.input);
        let outer = Size::from((panel.size.w + 2 * SHADOW, panel.size.h + 2 * SHADOW));
        let mut c = Canvas::new(physical(outer, scale));
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let border = px(1).max(1);
        c.popup(panel.size, HELP_RADIUS, scale, p);

        let band = (px(input.loc.y), px(input.size.h));
        self.edit_line(
            &mut c,
            &view.query,
            view.hint,
            switcher_text(input, scale),
            band,
            true,
            scale,
            p,
        );
        let line_y = px(input.loc.y + input.size.h) - border;
        c.fill(
            px(panel.loc.x) + border,
            line_y,
            px(panel.size.w) - 2 * border,
            border,
            p.border_variant,
        );

        if view.rows.is_empty() {
            let empty = switcher_parts(&[None], (SHADOW, SHADOW).into(), w);
            let slot = empty.rows[0];
            let band = (px(slot.loc.y), px(slot.size.h));
            self.text(
                &mut c,
                "Nothing matches",
                FONT_SIZE,
                px(slot.loc.x) as f32 + PADDING * s,
                px(input.loc.x + input.size.w - HELP_PADDING) as f32,
                band,
                scale,
                p.placeholder,
            );
        }
        for (rect, label) in &layout.sections {
            let band = (
                px(rect.loc.y + rect.size.h - SECTION_TEXT),
                px(SECTION_TEXT),
            );
            self.text(
                &mut c,
                &label.to_uppercase(),
                TINY_FONT_SIZE,
                px(rect.loc.x) as f32 + PADDING * s,
                px(rect.loc.x + rect.size.w) as f32 - PADDING * s,
                band,
                scale,
                p.placeholder,
            );
        }
        for (i, (slot, row)) in layout.rows.iter().zip(&view.rows).enumerate() {
            let (x, y, w, h) = (
                px(slot.loc.x),
                px(slot.loc.y),
                px(slot.size.w),
                px(slot.size.h),
            );
            let fill = if view.selected == Some(i) {
                Some(p.selected)
            } else if view.hover == Some(i) {
                Some(p.element)
            } else {
                None
            };
            if let Some(fill) = fill {
                c.rounded(
                    x as f32,
                    y as f32,
                    w as f32,
                    h as f32,
                    (MENU_RADIUS - 2.0) * s,
                    fill,
                );
            }
            let icon = Rectangle::new(
                (slot.loc.x + PADDING as i32, slot.loc.y).into(),
                (ROW_ICON_WIDTH, slot.size.h).into(),
            );
            let small = 0.8 * s;
            match &row.icon {
                ItemIcon::Workspace(WorkspaceKind::Project) => {
                    icon_sdf(&mut c, icon, scale, p.placeholder, |x, y| {
                        folder(x, y, small)
                    })
                }
                ItemIcon::Workspace(WorkspaceKind::Terminal) => {
                    icon_sdf(&mut c, icon, scale, p.placeholder, |x, y| {
                        terminal(x, y, small)
                    })
                }
                ItemIcon::Workspace(WorkspaceKind::Cursor) => {
                    icon_sdf(&mut c, icon, scale, p.placeholder, |x, y| app(x, y, small))
                }
                ItemIcon::Tab => {
                    icon_sdf(&mut c, icon, scale, p.placeholder, |x, y| tab(x, y, small))
                }
                &ItemIcon::Agent(status) => {
                    icon_sdf(&mut c, icon, scale, p.status(status), |x, y| {
                        status_mark(status, x, y, s)
                    })
                }
                ItemIcon::Folder => icon_sdf(&mut c, icon, scale, p.placeholder, |x, y| {
                    folder_plus(x, y, small)
                }),
                ItemIcon::App => {
                    icon_sdf(&mut c, icon, scale, p.placeholder, |x, y| app(x, y, small))
                }
                ItemIcon::Picture(picture) => c.picture(picture, icon, scale, p.placeholder),
                ItemIcon::Command => icon_sdf(&mut c, icon, scale, p.placeholder, |x, y| {
                    prompt(x, y, small)
                }),
                ItemIcon::Settings => icon_sdf(&mut c, icon, scale, p.placeholder, |x, y| {
                    pencil(x, y, small)
                }),
            }

            // The kind pill at the right; the text stops before it.
            let pill_w =
                self.text_width(row.kind, TINY_FONT_SIZE, scale) + 2.0 * PILL_PADDING as f32 * s;
            let pill_x = ((x + w) as f32 - PADDING * s - pill_w).round();
            let pill_h = px(PILL_HEIGHT) as f32;
            let pill_y = (y as f32 + (h as f32 - pill_h) / 2.0).round();
            c.rounded(
                pill_x,
                pill_y,
                pill_w.round(),
                pill_h,
                pill_h / 2.0,
                mix(fill.unwrap_or(p.elevated), p.muted, 0.18),
            );
            self.text(
                &mut c,
                row.kind,
                TINY_FONT_SIZE,
                pill_x + PILL_PADDING as f32 * s,
                f32::MAX,
                (pill_y as i32, pill_h as i32),
                scale,
                p.muted,
            );

            let right = pill_x - px(PILL_GAP) as f32;
            let text_x = px(icon.loc.x + ROW_ICON_WIDTH + ROW_ICON_GAP) as f32;
            let label_band = (y + px(LINE_INSET), px(LABEL_LINE));
            self.marked(
                &mut c,
                &row.label,
                &row.label_hits,
                FONT_SIZE,
                text_x,
                right,
                label_band,
                scale,
                (p.text, p.accent),
            );
            let detail_top = y + px(LINE_INSET + LABEL_LINE);
            let detail_band = (detail_top, y + h - px(LINE_INSET) - detail_top);
            self.marked(
                &mut c,
                &row.detail,
                &row.detail_hits,
                SMALL_FONT_SIZE,
                text_x,
                right,
                detail_band,
                scale,
                (p.muted, p.accent),
            );
        }
        c.into_buffer(false)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ItemIcon, ROW_HEIGHT, SECTION_HEIGHT, SWITCHER_INPUT, SWITCHER_PADDING, SwitcherRow,
        SwitcherView, switcher_geometry, switcher_layout, switcher_page,
    };
    use smithay::utils::Rectangle;

    fn row(section: Option<&'static str>) -> SwitcherRow {
        SwitcherRow {
            icon: ItemIcon::Tab,
            label: "a".into(),
            detail: "b".into(),
            kind: "tab",
            section,
            label_hits: Vec::new(),
            detail_hits: Vec::new(),
        }
    }

    #[test]
    fn headings_take_their_own_band_above_the_first_row_of_a_section() {
        let area = Rectangle::new((200, 36).into(), (1000, 800).into());
        let view = SwitcherView {
            query: "".into(),
            hint: "",
            rows: vec![row(Some("Workspaces")), row(None), row(Some("Apps"))],
            selected: Some(0),
            hover: None,
        };
        let layout = switcher_geometry(&view, area);
        let top = layout.panel.loc.y + SWITCHER_INPUT + SWITCHER_PADDING;
        assert_eq!(layout.sections.len(), 2);
        assert_eq!(layout.sections[0].0.loc.y, top);
        assert_eq!(layout.sections[0].1, "Workspaces");
        assert_eq!(layout.rows[0].loc.y, top + SECTION_HEIGHT);
        assert_eq!(layout.rows[1].loc.y, top + SECTION_HEIGHT + ROW_HEIGHT);
        assert_eq!(
            layout.sections[1].0.loc.y,
            top + SECTION_HEIGHT + 2 * ROW_HEIGHT
        );
        assert_eq!(
            layout.rows[2].loc.y,
            top + 2 * SECTION_HEIGHT + 2 * ROW_HEIGHT
        );
        assert_eq!(
            layout.panel.size.h,
            SWITCHER_INPUT + 2 * SWITCHER_PADDING + 2 * SECTION_HEIGHT + 3 * ROW_HEIGHT
        );
        // The same rows without headings, from the count alone.
        let (panel, input, rows) = switcher_layout(3, area);
        assert_eq!(
            panel.size.h,
            SWITCHER_INPUT + 2 * SWITCHER_PADDING + 3 * ROW_HEIGHT
        );
        assert_eq!((panel.loc, input.loc), (layout.panel.loc, layout.input.loc));
        assert_eq!(rows[0].loc.y, top);
        // An empty list keeps room for the "Nothing matches" row.
        assert_eq!(
            switcher_layout(0, area).0.size.h,
            SWITCHER_INPUT + 2 * SWITCHER_PADDING + ROW_HEIGHT
        );
    }

    #[test]
    fn the_page_keeps_room_for_the_headings() {
        let area = Rectangle::new((0, 0).into(), (1000, 600).into());
        assert!(switcher_page(area, 7) < switcher_page(area, 1));
        assert!(switcher_page(area, 7) >= 1);
        assert_eq!(
            switcher_page(Rectangle::new((0, 0).into(), (1000, 100).into()), 7),
            1
        );
        assert_eq!(
            switcher_page(Rectangle::new((0, 0).into(), (1000, 2000).into()), 7),
            12
        );
    }
}

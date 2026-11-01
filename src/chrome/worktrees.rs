//! The worktrees popup: the worktrees of a workspace's repository, or of
//! every repository the workspaces are in, a section per repository, under
//! a field filtering them; each with where it stands and, on the selected
//! or hovered row, buttons to review, check out, merge and delete it.

use super::{
    BUTTON_RADIUS, Canvas, DETAIL_FONT_SIZE, FIELD_GAP, FONT_SIZE, FieldSpot, GIT_MARK_GAP,
    GIT_MARK_WIDTH, HELP_MARGIN, HELP_RADIUS, LABEL_FONT_SIZE, MENU_MARK_GAP, MENU_MARK_WIDTH,
    MENU_RADIUS, PADDING, PROJECT_MENU_INPUT, PROJECT_MENU_PADDING, PROJECT_MENU_ROW,
    PROJECT_MENU_SECTION, Painter, Palette, ROW_BUTTON, ROW_BUTTON_INSET, Rect, SHADOW,
    SMALL_FONT_SIZE, band, footer_height, footer_hint_spots, git_branch, hint_at, hint_strs,
    icon_sdf,
    layers::{Part, RowCache},
    magnifier, measure, mix, phys, physical,
    project_menu::{menu_mark, menu_query_text},
    rect_px,
};
use crate::git::GitStatus;
use crate::text_field::Line;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// The popup: at most this wide, its title row this high.
const WORKTREES_WIDTH: i32 = 640;
const WORKTREES_TITLE: i32 = 44;
/// The fewest and most entries it shows; it scrolls for more.
const WORKTREES_MIN_ROWS: i32 = 3;
const WORKTREES_MAX_ROWS: i32 = 14;
/// The gap between a row's buttons.
const WORKTREE_BUTTON_GAP: i32 = 4;
/// The scope toggle's segments in the title row: "This workspace" and
/// "All", and how far it keeps from the right edge.
const SCOPE_OWN: i32 = 120;
const SCOPE_ALL: i32 = 48;
const SCOPE_INSET: i32 = 12;
/// How high a worktree's row is, and a repository's label.
pub const WORKTREES_ROW: i32 = PROJECT_MENU_ROW;
pub const WORKTREES_LABEL: i32 = PROJECT_MENU_SECTION;

/// What the worktrees popup shows.
#[derive(Clone, Debug, PartialEq)]
pub struct WorktreesView {
    /// The entries in view, scrolled, then those under the list shown in
    /// part (in its last rows' room, or as it scrolls part of a row).
    pub entries: Vec<WorktreeEntry>,
    /// How high the list is kept, whatever is in view, so it does not
    /// change as it scrolls; entries past it show in part.
    pub list_height: i32,
    /// The selected entry, by index into `entries`.
    pub selected: Option<usize>,
    pub hover: Option<WorktreesHit>,
    /// The footer's key hints, as (key, what it does).
    pub hints: Vec<(String, String)>,
    /// What the list is filtered by.
    pub query: Line,
    /// Every repository's worktrees are listed, not just the workspace's.
    pub all: bool,
    /// The workspace's repository, by its folder's name; `None` when the
    /// workspace isn't in one (and there is no choice).
    pub repo: Option<String>,
    /// What the list says when it is empty.
    pub empty: String,
    /// The part of the first worktree scrolled past, 0 to 1.
    pub part: f64,
    /// How far the entries are moved down (up when negative) past an end
    /// of the list, in logical pixels, as it stretches or bounces.
    pub overscroll: f64,
    /// Where the list stands, for its scrollbar; none while it all shows.
    pub bar: Option<super::ListBar>,
}

/// An entry of the worktrees popup.
#[derive(Clone, Debug, PartialEq)]
pub enum WorktreeEntry {
    /// A repository's section label, its folder's name; not selectable.
    Repo(String),
    Worktree(WorktreeLine),
}

/// A worktree's row.
#[derive(Clone, Debug, PartialEq)]
pub struct WorktreeLine {
    /// Its branch, else its folder's name.
    pub name: String,
    /// Where it is, from its repository's main worktree.
    pub path: String,
    pub dirty: bool,
    /// Where it stands: "unmerged", "gone", "merged", "main worktree".
    pub tag: String,
    pub state: LineState,
    /// What it can have done, shown on it when selected or hovered.
    pub buttons: Vec<WorktreeButton>,
}

/// What a worktree's row says besides its tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LineState {
    /// Done with, or the main worktree: in muted colours.
    Settled,
    /// It needs a look: its tag in the warning colour.
    Attention,
    /// Delete was asked for once: what a second time does.
    Armed(String),
    /// Something is being done to it, as "deleting…".
    Busy(String),
}

/// A button on a worktree's row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorktreeButton {
    Diff,
    Checkout,
    Merge,
    Delete,
}

impl WorktreeButton {
    fn label(self) -> &'static str {
        match self {
            WorktreeButton::Diff => "Diff",
            WorktreeButton::Checkout => "Checkout",
            WorktreeButton::Merge => "Merge",
            WorktreeButton::Delete => "Delete",
        }
    }

    /// How wide it is: room for its label.
    fn width(self) -> i32 {
        match self {
            WorktreeButton::Diff => 48,
            WorktreeButton::Checkout => 76,
            WorktreeButton::Merge => 58,
            WorktreeButton::Delete => 62,
        }
    }
}

/// What the pointer is over in the worktrees popup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorktreesHit {
    /// An entry, by index into `WorktreesView::entries`.
    Entry(usize),
    /// A button on an entry's row.
    Button(usize, WorktreeButton),
    /// The filter field.
    Query,
    /// The scope toggle's segment for every repository (`true`) or the
    /// workspace's.
    Scope(bool),
    /// A key hint in the footer, by index into `WorktreesView::hints`.
    Hint(usize),
    /// Inside the panel, on nothing.
    Panel,
}

/// Where the worktrees popup's parts go, in output coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreesLayout {
    pub panel: Rect,
    pub title: Rect,
    /// The scope toggle's segments, as (for every repository, rect); none
    /// without a choice.
    pub scope: Vec<(bool, Rect)>,
    pub query: Rect,
    /// Per entry, its rect and its buttons' (none unless selected or
    /// hovered). A scrolled list's entries reach out of `list`.
    pub entries: Vec<(Rect, Vec<(WorktreeButton, Rect)>)>,
    /// Where the entries show.
    pub list: Rect,
    pub footer: Rect,
    /// The footer's key hints, by index into `WorktreesView::hints`.
    pub hints: Vec<(usize, Rect)>,
}

/// Where the worktrees popup's top is on an output of `size`: a sixth of
/// the way down.
fn worktrees_top(size: Size<i32, Logical>) -> i32 {
    (size.h / 6).max(HELP_MARGIN)
}

/// How many entries the worktrees popup shows at once on an output of
/// `size`: as many as fit between its top and the bottom margin with the
/// footer at its tallest.
pub fn worktrees_page(size: Size<i32, Logical>) -> usize {
    let room = size.h
        - worktrees_top(size)
        - HELP_MARGIN
        - WORKTREES_TITLE
        - PROJECT_MENU_INPUT
        - 2 * PROJECT_MENU_PADDING
        - footer_height(usize::MAX);
    (room / PROJECT_MENU_ROW).clamp(WORKTREES_MIN_ROWS, WORKTREES_MAX_ROWS) as usize
}

/// Where the parts of the worktrees popup showing `view` go, on an output
/// of `size`: centred across, a sixth of the way down; the title (with the
/// scope toggle at its right), the filter field, the entries, then the
/// footer.
pub fn worktrees_layout(view: &WorktreesView, size: Size<i32, Logical>) -> WorktreesLayout {
    let w = WORKTREES_WIDTH.min(size.w - 2 * HELP_MARGIN).max(1);
    let x = ((size.w - w) / 2).max(0);
    let top = worktrees_top(size);
    let title = Rectangle::new((x, top).into(), (w, WORKTREES_TITLE).into());
    let scope = if view.repo.is_some() {
        let y = top + (WORKTREES_TITLE - ROW_BUTTON) / 2;
        let all_x = x + w - SCOPE_INSET - SCOPE_ALL;
        let own_x = all_x - WORKTREE_BUTTON_GAP - SCOPE_OWN;
        vec![
            (
                false,
                Rectangle::new((own_x, y).into(), (SCOPE_OWN, ROW_BUTTON).into()),
            ),
            (
                true,
                Rectangle::new((all_x, y).into(), (SCOPE_ALL, ROW_BUTTON).into()),
            ),
        ]
    } else {
        Vec::new()
    };
    let query = Rectangle::new(
        (x, top + WORKTREES_TITLE).into(),
        (w, PROJECT_MENU_INPUT).into(),
    );
    let (row_x, row_w) = (
        x + PROJECT_MENU_PADDING,
        (w - 2 * PROJECT_MENU_PADDING).max(0),
    );
    let start = query.loc.y + query.size.h + PROJECT_MENU_PADDING;
    let with_buttons = match view.hover {
        Some(WorktreesHit::Entry(i) | WorktreesHit::Button(i, _)) => Some(i),
        _ => view.selected,
    };
    let entries = view
        .entries
        .iter()
        .zip(entry_tops(view, start))
        .enumerate()
        .map(|(i, (entry, top))| {
            let size = (row_w, height(entry)).into();
            let rect = Rectangle::new((row_x, top.round() as i32).into(), size);
            let buttons = match entry {
                WorktreeEntry::Worktree(line) if with_buttons == Some(i) => {
                    button_rects(&line.buttons, rect)
                }
                _ => Vec::new(),
            };
            (rect, buttons)
        })
        .collect();
    // As high as it is kept, the entries past it shown in part; with room
    // for the line saying there are none.
    let y = start + view.list_height.max(PROJECT_MENU_ROW);
    let list = Rectangle::new((row_x, start).into(), (row_w, y - start).into());
    let footer_h = super::measured_footer_height(&hint_strs(&view.hints), w);
    let bottom = y + PROJECT_MENU_PADDING + footer_h;
    let footer = Rectangle::new((x, bottom - footer_h).into(), (w, footer_h).into());
    WorktreesLayout {
        panel: Rectangle::new((x, top).into(), (w, bottom - top).into()),
        title,
        scope,
        query,
        entries,
        list,
        footer,
        hints: footer_hint_spots(&hint_strs(&view.hints), footer),
    }
}

/// The scrollbar of the popup's list laid out as `layout`, in the gutter
/// right of its entries; none while they all show.
pub fn worktrees_thumb(view: &WorktreesView, layout: &WorktreesLayout) -> Option<super::Thumb> {
    let list = layout.list;
    let track = super::gutter_track(list.loc.x + list.size.w, list);
    super::Thumb::of(track, list.size.h as f64, view.bar?)
}

/// How high `entry` is in the list.
fn height(entry: &WorktreeEntry) -> i32 {
    match entry {
        WorktreeEntry::Repo(_) => WORKTREES_LABEL,
        WorktreeEntry::Worktree(_) => WORKTREES_ROW,
    }
}

/// The list's first entry is the label of the first worktree's repository,
/// kept on top as the worktrees under it scroll.
fn pinned(view: &WorktreesView) -> bool {
    matches!(view.entries.first(), Some(WorktreeEntry::Repo(_)))
}

/// How far up `view`'s entries are moved by the part of its first
/// worktree scrolled past, and how far its label on top: not at all, but
/// for the next repository's label pushing it out (logical pixels, to the
/// fraction).
///
/// Scrolling one worktree moves the next one up to where the first is:
/// the first's height, and the next one's label's when it has one.
fn scrolled_by(view: &WorktreesView) -> (f64, f64) {
    let skip = usize::from(pinned(view));
    let mut rest = view.entries.iter().skip(skip);
    let Some(first) = rest.next() else {
        return (0.0, 0.0);
    };
    let label = rest
        .next()
        .filter(|entry| matches!(entry, WorktreeEntry::Repo(_)))
        .map_or(0, height);
    let shift = view.part.clamp(0.0, 1.0) * (height(first) + label) as f64;
    let pushed = if label > 0 {
        (shift - height(first) as f64).max(0.0)
    } else {
        0.0
    };
    (shift, pushed)
}

/// Where the tops of `view`'s entries go, the list starting at `start`
/// (logical pixels, to the fraction): moved up as scrolled (see
/// `scrolled_by`) and with the overscroll. The label on top keeps its
/// place past the end, the worktrees going on under it.
fn entry_tops(view: &WorktreesView, start: i32) -> Vec<f64> {
    let (shift, pushed) = scrolled_by(view);
    let pin = pinned(view);
    let mut y = start;
    view.entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let top = if i == 0 && pin {
                y as f64 - pushed + view.overscroll.max(0.0)
            } else {
                y as f64 - shift + view.overscroll
            };
            y += height(entry);
            top
        })
        .collect()
}

/// `buttons` at the right end of a row in `rect`, vertically centred.
fn button_rects(buttons: &[WorktreeButton], rect: Rect) -> Vec<(WorktreeButton, Rect)> {
    let mut right = rect.loc.x + rect.size.w - ROW_BUTTON_INSET;
    let top = rect.loc.y + (rect.size.h - ROW_BUTTON) / 2;
    let mut rects: Vec<_> = buttons
        .iter()
        .rev()
        .map(|&button| {
            let w = button.width();
            right -= w;
            let r = Rectangle::new((right, top).into(), (w, ROW_BUTTON).into());
            right -= WORKTREE_BUTTON_GAP;
            (button, r)
        })
        .collect();
    rects.reverse();
    rects
}

/// The marks on worktree row `rect`, its `buttons` at its right, showing
/// `line`: its branch icon, then its dirty dot (where the painter puts it
/// after the name, a gap either side to aim at), with what each says.
pub fn worktree_marks(
    line: &WorktreeLine,
    rect: Rect,
    buttons: &[(WorktreeButton, Rect)],
) -> Vec<(Rect, &'static str)> {
    let mark = menu_mark(rect);
    let kind = if line.tag == "main worktree" {
        "Main worktree"
    } else {
        "Linked worktree"
    };
    let mut marks = vec![(mark, kind)];
    if line.dirty {
        let end = match buttons.first() {
            Some((_, first)) => first.loc.x - FIELD_GAP,
            None => rect.loc.x + rect.size.w - PADDING as i32,
        };
        let name_x = mark.loc.x + MENU_MARK_WIDTH + MENU_MARK_GAP;
        let name_w = measure(&line.name, FONT_SIZE).unwrap_or(0.0).ceil() as i32;
        let x = (name_x + name_w).min(end);
        let w = 2 * GIT_MARK_GAP + GIT_MARK_WIDTH;
        if x + w <= end + GIT_MARK_GAP {
            let dot = Rectangle::new((x, rect.loc.y).into(), (w, rect.size.h).into());
            marks.push((dot, "Changes not committed"));
        }
    }
    marks
}

/// What `pos` (output coordinates) is over in the worktrees popup; `None`
/// outside it.
pub fn worktrees_hit(
    view: &WorktreesView,
    layout: &WorktreesLayout,
    pos: Point<f64, Logical>,
) -> Option<WorktreesHit> {
    if !layout.panel.to_f64().contains(pos) {
        return None;
    }
    if let Some(i) = hint_at(&hint_strs(&view.hints), &layout.hints, pos) {
        return Some(WorktreesHit::Hint(i));
    }
    if let Some((all, _)) = layout.scope.iter().find(|(_, r)| r.to_f64().contains(pos)) {
        return Some(WorktreesHit::Scope(*all));
    }
    if layout.query.to_f64().contains(pos) {
        return Some(WorktreesHit::Query);
    }
    if !layout.list.to_f64().contains(pos) {
        return Some(WorktreesHit::Panel);
    }
    // The label on top comes first: it covers the worktree scrolled under it.
    for (i, (rect, buttons)) in layout.entries.iter().enumerate() {
        if let Some((button, _)) = buttons.iter().find(|(_, r)| r.to_f64().contains(pos)) {
            return Some(WorktreesHit::Button(i, *button));
        }
        if rect.to_f64().contains(pos) {
            return Some(WorktreesHit::Entry(i));
        }
    }
    Some(WorktreesHit::Panel)
}

/// The worktrees popup's filter field, which has the keyboard, on an output
/// of `size`.
pub fn worktrees_field(view: &WorktreesView, size: Size<i32, Logical>, scale: f64) -> FieldSpot {
    let area = worktrees_layout(view, size).query;
    FieldSpot {
        size: FONT_SIZE,
        area,
        text: menu_query_text(area, scale),
        line: view.query.clone(),
    }
}

impl Painter {
    /// The worktrees popup for an output of `size` but its entries, which
    /// are drawn on their own (see `worktree_row`), inside a margin of
    /// `SHADOW` holding its drop shadow. `view` is the frame's (see
    /// `frame_view`): what the list says when it is empty shows in it.
    fn worktrees_frame(
        &self,
        view: &WorktreesView,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let layout = worktrees_layout(view, size);
        let panel = layout.panel;
        let mut c = self.popup(panel.size, HELP_RADIUS, scale, p);
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let border = px(1).max(1);
        let by = Point::from((SHADOW - panel.loc.x, SHADOW - panel.loc.y));
        let shift = |r: Rect| Rectangle::new(r.loc + by, r.size);
        let divider = |c: &mut Canvas, y: i32| {
            c.fill(
                px(SHADOW) + border,
                y,
                px(panel.size.w) - 2 * border,
                border,
                p.border_variant,
            );
        };

        let title = shift(layout.title);
        let (x, mut right) = phys(title, scale);
        let x = x + ((PROJECT_MENU_PADDING as f32 + PADDING) * s).round();
        let scope: Vec<_> = layout.scope.iter().map(|(a, r)| (*a, shift(*r))).collect();
        if let Some((_, first)) = scope.first() {
            right = px(first.loc.x) as f32 - (FIELD_GAP as f32 * s).round();
        }
        let heading = match &view.repo {
            Some(repo) if !view.all => format!("Worktrees · {repo}"),
            _ => "Worktrees".to_owned(),
        };
        self.text(
            &mut c,
            &heading,
            LABEL_FONT_SIZE,
            x,
            right,
            band(title, scale),
            scale,
            p.text,
        );
        for &(all, rect) in &scope {
            self.scope_segment(&mut c, view, all, rect, scale, p);
        }

        let query = shift(layout.query);
        let mark = Rectangle::new(
            (
                query.loc.x + PROJECT_MENU_PADDING + PADDING as i32,
                query.loc.y,
            )
                .into(),
            (MENU_MARK_WIDTH, query.size.h).into(),
        );
        icon_sdf(&mut c, mark, scale, p.placeholder, |x, y| {
            magnifier(x, y, s)
        });
        self.edit_line(
            &mut c,
            &view.query,
            "Filter worktrees…",
            menu_query_text(query, scale),
            band(query, scale),
            true,
            FONT_SIZE,
            scale,
            p,
        );
        divider(&mut c, px(query.loc.y + query.size.h) - border);

        if !view.empty.is_empty() {
            let first = Rectangle::new(
                (
                    query.loc.x + PROJECT_MENU_PADDING,
                    query.loc.y + query.size.h + PROJECT_MENU_PADDING,
                )
                    .into(),
                (query.size.w, PROJECT_MENU_ROW).into(),
            );
            let (x, right) = phys(first, scale);
            self.text(
                &mut c,
                &view.empty,
                FONT_SIZE,
                x + PADDING * s,
                right,
                band(first, scale),
                scale,
                p.placeholder,
            );
        }

        let footer = shift(layout.footer);
        divider(&mut c, px(footer.loc.y));
        let spots: Vec<_> = layout.hints.iter().map(|(i, r)| (*i, shift(*r))).collect();
        let hover = match view.hover {
            Some(WorktreesHit::Hint(i)) => Some(i),
            _ => None,
        };
        self.hints(&mut c, &hint_strs(&view.hints), &spots, hover, scale, p);
        c.into_buffer(false)
    }

    /// The row `key` says, on the list's background: a buffer as big as
    /// the row, at its scale.
    fn worktree_row(&self, key: &RowKey, p: &Palette) -> MemoryRenderBuffer {
        let size = physical(key.size, key.scale);
        let mut c = Canvas::new(size);
        c.fill(0, 0, size.w, size.h, p.elevated);
        let rect = Rectangle::from_size(key.size);
        let buttons = match &key.entry {
            WorktreeEntry::Worktree(line) if key.buttons => button_rects(&line.buttons, rect),
            _ => Vec::new(),
        };
        self.worktree_entry(
            &mut c,
            &key.entry,
            rect,
            &buttons,
            key.selected,
            key.hover,
            key.scale,
            p,
        );
        c.into_buffer(false)
    }

    /// An entry of the worktrees popup in `rect` (buffer coordinates),
    /// with its `buttons`; under the pointer when `hover`, on the button
    /// it holds if any.
    #[allow(clippy::too_many_arguments)]
    fn worktree_entry(
        &self,
        c: &mut Canvas,
        entry: &WorktreeEntry,
        rect: Rect,
        buttons: &[(WorktreeButton, Rect)],
        selected: bool,
        hover: Option<Option<WorktreeButton>>,
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let row_band = band(rect, scale);
        let mark = menu_mark(rect);
        let (x, y, w, h) = rect_px(rect, scale);
        let right = x + w - PADDING * s;
        let gap = (FIELD_GAP as f32 * s).round();
        let line = match entry {
            WorktreeEntry::Repo(name) => {
                self.text(
                    c,
                    &name.to_uppercase(),
                    DETAIL_FONT_SIZE,
                    px(mark.loc.x) as f32,
                    right,
                    row_band,
                    scale,
                    p.placeholder,
                );
                return;
            }
            WorktreeEntry::Worktree(line) => line,
        };

        // The highlight stops short of the rows above and below.
        let (y, h) = (y + px(2) as f32, h - 2.0 * px(2) as f32);
        let r = (MENU_RADIUS - 2.0) * s;
        let armed = matches!(line.state, LineState::Armed(_));
        if armed {
            c.rounded(x, y, w, h, r, mix(p.elevated, p.warning, 0.15));
        } else if selected {
            c.rounded(x, y, w, h, r, p.selected);
        } else if hover.is_some() {
            c.rounded(x, y, w, h, r, mix(p.elevated, p.selected, 0.5));
        }

        let mark_color = if line.tag == "main worktree" {
            p.accent
        } else {
            p.muted
        };
        icon_sdf(c, mark, scale, mark_color, |x, y| git_branch(x, y, 0.9 * s));
        let mut end = right;
        for &(button, rect) in buttons {
            let hot = hover == Some(Some(button));
            self.worktree_button(c, button, rect, hot, scale, p);
        }
        if let Some((_, first)) = buttons.first() {
            end = px(first.loc.x) as f32 - gap;
        }

        let label_x = px(mark.loc.x + MENU_MARK_WIDTH + MENU_MARK_GAP) as f32;
        let after = self.text(
            c, &line.name, FONT_SIZE, label_x, end, row_band, scale, p.text,
        );
        let dirty = GitStatus {
            dirty: line.dirty,
            ..GitStatus::default()
        };
        let after = self.git_marks(c, &dirty, after, end, row_band, scale, p);
        let (tag, color) = match &line.state {
            LineState::Armed(what) => (what.as_str(), p.warning),
            LineState::Busy(what) => (what.as_str(), p.placeholder),
            LineState::Attention => (line.tag.as_str(), p.warning),
            LineState::Settled => (line.tag.as_str(), p.placeholder),
        };
        let after = self.text(
            c,
            tag,
            SMALL_FONT_SIZE,
            after + gap,
            end,
            row_band,
            scale,
            color,
        );
        if !armed {
            self.text(
                c,
                &line.path,
                SMALL_FONT_SIZE,
                after + gap,
                end,
                row_band,
                scale,
                p.placeholder,
            );
        }
    }

    /// The scope toggle's segment for every repository (`all`) or the
    /// workspace's, in `rect`: an outlined pill, filled when it is the
    /// scope shown, lighter under the pointer.
    fn scope_segment(
        &self,
        c: &mut Canvas,
        view: &WorktreesView,
        all: bool,
        rect: Rect,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        let (x, y, w, h) = rect_px(rect, scale);
        let b = ((scale.round() as i32).max(1)) as f32;
        let r = BUTTON_RADIUS * s;
        let on = view.all == all;
        let fill = if on {
            p.selected
        } else if view.hover == Some(WorktreesHit::Scope(all)) {
            mix(p.element, p.selected, 0.5)
        } else {
            p.element
        };
        c.rounded(x, y, w, h, r, p.border);
        c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, fill);
        let label = if all { "All" } else { "This workspace" };
        let color = if on { p.text } else { p.muted };
        self.centered(c, label, SMALL_FONT_SIZE, rect, scale, color);
    }

    /// `button` of a row in `rect`: its label on an outlined pill,
    /// lighter under the pointer (`hot`); Delete's in the error colour.
    fn worktree_button(
        &self,
        c: &mut Canvas,
        button: WorktreeButton,
        rect: Rect,
        hot: bool,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        let (x, y, w, h) = rect_px(rect, scale);
        let b = ((scale.round() as i32).max(1)) as f32;
        let r = BUTTON_RADIUS * s;
        let fill = if hot { p.selected } else { p.element };
        c.rounded(x, y, w, h, r, p.border);
        c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, fill);
        let color = match button {
            WorktreeButton::Delete => p.error,
            _ => p.text,
        };
        self.centered(c, button.label(), SMALL_FONT_SIZE, rect, scale, color);
    }
}

/// What a worktree's row (or a repository's label) is drawn from: not
/// where it is, so scrolling it draws nothing again.
#[derive(Clone, Debug, PartialEq)]
struct RowKey {
    entry: WorktreeEntry,
    size: Size<i32, Logical>,
    selected: bool,
    /// Its buttons show.
    buttons: bool,
    /// Under the pointer, on the button it holds if any.
    hover: Option<Option<WorktreeButton>>,
    theme: super::Theme,
    scale: f64,
}

/// What of `view` the popup's frame shows, as its cache key: neither the
/// entries nor how far they are scrolled, nor the pointer over them; what
/// the list says when it is empty only while it is.
fn frame_view(view: &WorktreesView) -> WorktreesView {
    WorktreesView {
        entries: Vec::new(),
        list_height: view.list_height,
        selected: None,
        hover: view
            .hover
            .filter(|hit| matches!(hit, WorktreesHit::Scope(_) | WorktreesHit::Hint(_))),
        hints: view.hints.clone(),
        query: view.query.clone(),
        all: view.all,
        repo: view.repo.clone(),
        empty: if view.entries.is_empty() {
            view.empty.clone()
        } else {
            String::new()
        },
        part: 0.0,
        overscroll: 0.0,
        bar: None,
    }
}

/// The worktrees popup as the chrome shows it: its frame, and a buffer per
/// entry in view placed over it, fading in and out together (see
/// `Overlay`). Scrolling moves the entries' buffers only.
#[derive(Default)]
pub(super) struct WorktreesLayers {
    overlay: super::Overlay<(WorktreesView, Size<i32, Logical>, super::Theme, f64)>,
    rows: RowCache<RowKey>,
    /// The scrollbar's thumb, by its size.
    thumb: super::Cached<(Size<i32, Logical>, super::Theme, f64)>,
    /// How many frames were drawn, for tests.
    #[cfg(test)]
    frames: usize,
}

impl WorktreesLayers {
    /// The parts showing the worktrees popup at `now`, front to back.
    pub(super) fn parts(
        &mut self,
        view: Option<WorktreesView>,
        output: Size<i32, Logical>,
        theme: super::Theme,
        scale: f64,
        now: std::time::Instant,
        painter: &Painter,
    ) -> Vec<Part> {
        let placed = view.as_ref().map(|view| {
            let (loc, size) = super::shadowed(worktrees_layout(view, output).panel);
            ((frame_view(view), output, theme, scale), loc, size)
        });
        let (rows, thumb) = (&mut self.rows, &mut self.thumb);
        #[cfg(test)]
        let frames = &mut self.frames;
        let parts = self.overlay.parts(
            placed,
            now,
            true,
            |(frame, output, ..)| {
                #[cfg(test)]
                {
                    *frames += 1;
                }
                painter.worktrees_frame(frame, *output, scale, theme.palette())
            },
            |_| {
                let Some(view) = &view else {
                    return Vec::new();
                };
                let mut parts = Vec::new();
                let layout = worktrees_layout(view, output);
                if let Some(bar) = worktrees_thumb(view, &layout) {
                    let size = Size::from((bar.track.size.w, bar.h as i32));
                    let buffer = thumb.get((size, theme, scale), |_| {
                        painter.bar_thumb(size, scale, theme.palette())
                    });
                    let part = Part::new(buffer, bar.track.loc, size, 1.0);
                    parts.push(part.at((bar.track.loc.x as f64, bar.top).into()));
                }
                parts.extend(row_parts(rows, view, output, theme, scale, painter));
                parts
            },
        );
        self.rows.sweep();
        if !self.overlay.lingers() {
            self.rows.clear();
        }
        parts
    }
}

/// The parts of `view`'s entries in view, from `rows`, front to back: the
/// label on top (the first) over the worktrees scrolled under it.
fn row_parts(
    rows: &mut RowCache<RowKey>,
    view: &WorktreesView,
    output: Size<i32, Logical>,
    theme: super::Theme,
    scale: f64,
    painter: &Painter,
) -> Vec<Part> {
    let layout = worktrees_layout(view, output);
    let list = layout.list;
    let tops = entry_tops(view, list.loc.y);
    let mut parts = Vec::new();
    for (i, ((rect, buttons), top)) in layout.entries.iter().zip(tops).enumerate() {
        let (h, bottom) = (rect.size.h as f64, (list.loc.y + list.size.h) as f64);
        if top + h <= list.loc.y as f64 || top >= bottom {
            continue;
        }
        let hover = match view.hover {
            Some(WorktreesHit::Entry(j)) if j == i => Some(None),
            Some(WorktreesHit::Button(j, button)) if j == i => Some(Some(button)),
            _ => None,
        };
        let key = RowKey {
            entry: view.entries[i].clone(),
            size: rect.size,
            selected: view.selected == Some(i),
            buttons: !buttons.is_empty(),
            hover,
            theme,
            scale,
        };
        let buffer = rows.get(key, rect.size, scale, |key| {
            painter.worktree_row(key, theme.palette())
        });
        let part = Part::new(&buffer, rect.loc, rect.size, 1.0)
            .at((rect.loc.x as f64, top).into())
            .clipped(list);
        parts.push(part);
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chrome::inside;

    fn line(name: &str, buttons: Vec<WorktreeButton>) -> WorktreeEntry {
        WorktreeEntry::Worktree(WorktreeLine {
            name: name.into(),
            path: format!(".claude/worktrees/{name}"),
            dirty: false,
            tag: "unmerged".into(),
            state: LineState::Attention,
            buttons,
        })
    }

    fn view() -> WorktreesView {
        use WorktreeButton::*;
        WorktreesView {
            entries: vec![
                WorktreeEntry::Repo("gerdr".into()),
                line("main", vec![Diff]),
                line("topic", vec![Diff, Checkout, Merge, Delete]),
            ],
            list_height: WORKTREES_LABEL + 2 * WORKTREES_ROW,
            selected: Some(2),
            hover: None,
            hints: vec![("↵".into(), "diff".into()), ("Del".into(), "delete".into())],
            query: Line::from("top"),
            all: false,
            repo: Some("gerdr".into()),
            empty: "No worktrees match".into(),
            part: 0.0,
            overscroll: 0.0,
            bar: None,
        }
    }

    #[test]
    fn a_rows_marks_lie_on_it_clear_of_its_buttons() {
        let size: Size<i32, Logical> = (1920, 1080).into();
        let v = view();
        let l = worktrees_layout(&v, size);
        let (rect, buttons) = &l.entries[2];
        let WorktreeEntry::Worktree(clean) = &v.entries[2] else {
            panic!("a worktree");
        };
        let marks = worktree_marks(clean, *rect, buttons);
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].1, "Linked worktree");
        let dirty = WorktreeLine {
            dirty: true,
            tag: "main worktree".into(),
            ..clean.clone()
        };
        let marks = worktree_marks(&dirty, *rect, buttons);
        let texts: Vec<_> = marks.iter().map(|(_, t)| *t).collect();
        assert_eq!(texts, ["Main worktree", "Changes not committed"]);
        let (dot, _) = marks[1];
        assert!(inside(*rect, dot) && dot.loc.x > marks[0].0.loc.x);
        assert!(
            buttons
                .iter()
                .all(|(_, b)| dot.loc.x + dot.size.w <= b.loc.x)
        );
    }

    #[test]
    fn the_selected_row_has_its_buttons_and_they_are_hit() {
        let size: Size<i32, Logical> = (1920, 1080).into();
        let v = view();
        let l = worktrees_layout(&v, size);
        assert_eq!(l.panel.loc.x, 1920 - (l.panel.loc.x + l.panel.size.w));
        assert!(l.entries.iter().all(|(r, _)| inside(l.panel, *r)));
        assert!(inside(l.panel, l.footer));
        assert!(l.entries[1].1.is_empty());
        let buttons = &l.entries[2].1;
        assert_eq!(buttons.len(), 4);
        assert!(buttons.iter().all(|(_, r)| inside(l.entries[2].0, *r)));
        assert!(
            buttons
                .windows(2)
                .all(|b| b[0].1.loc.x + b[0].1.size.w < b[1].1.loc.x)
        );
        let centre = |r: Rect| (r.loc.x as f64 + 2.0, r.loc.y as f64 + 2.0).into();
        let (button, rect) = buttons[2];
        assert_eq!(
            worktrees_hit(&v, &l, centre(rect)),
            Some(WorktreesHit::Button(2, button))
        );
        assert_eq!(
            worktrees_hit(&v, &l, centre(l.entries[1].0)),
            Some(WorktreesHit::Entry(1))
        );
        assert_eq!(
            worktrees_hit(&v, &l, centre(l.footer)),
            Some(WorktreesHit::Panel)
        );
        assert_eq!(worktrees_hit(&v, &l, (1.0, 1.0).into()), None);
        // The footer's key hints are buttons.
        assert!(!l.hints.is_empty());
        for (i, rect) in &l.hints {
            assert!(inside(l.footer, *rect));
            let mid = (
                rect.loc.x as f64 + rect.size.w as f64 / 2.0,
                rect.loc.y as f64 + rect.size.h as f64 / 2.0,
            );
            assert_eq!(
                worktrees_hit(&v, &l, mid.into()),
                Some(WorktreesHit::Hint(*i))
            );
        }
        // The filter field sits between the title and the list.
        assert!(inside(l.panel, l.query));
        assert!(l.query.loc.y >= l.title.loc.y + l.title.size.h);
        assert!(l.entries[0].0.loc.y >= l.query.loc.y + l.query.size.h);
        assert_eq!(
            worktrees_hit(&v, &l, centre(l.query)),
            Some(WorktreesHit::Query)
        );
        // The scope toggle's segments, in the title row, are hit.
        assert_eq!(l.scope.len(), 2);
        for (all, rect) in &l.scope {
            assert!(inside(l.title, *rect));
            assert_eq!(
                worktrees_hit(&v, &l, centre(*rect)),
                Some(WorktreesHit::Scope(*all))
            );
        }
        // Without a repository of its own there is no choice.
        let unscoped = worktrees_layout(
            &WorktreesView {
                repo: None,
                ..v.clone()
            },
            size,
        );
        assert!(unscoped.scope.is_empty());
        // Hovering another row moves the buttons there.
        let hovered = WorktreesView {
            hover: Some(WorktreesHit::Entry(1)),
            ..v
        };
        let l = worktrees_layout(&hovered, size);
        assert_eq!(l.entries[1].1.len(), 1);
        assert!(l.entries[2].1.is_empty());
    }

    #[test]
    fn a_scrolled_list_moves_up_under_its_label_and_shows_the_next_in_part() {
        let size: Size<i32, Logical> = (1920, 1080).into();
        let label = |name: &str| WorktreeEntry::Repo(name.into());
        let scrolled = WorktreesView {
            entries: vec![
                label("r"),
                line("wip", Vec::new()),
                line("done", Vec::new()),
                label("s"),
                line("other", Vec::new()),
            ],
            list_height: 3 * WORKTREES_ROW,
            selected: None,
            part: 0.5,
            ..view()
        };
        let l = worktrees_layout(&scrolled, size);
        // As high as it is kept, its label on top staying, the worktrees
        // moved up half the first, the last reaching out under it.
        assert_eq!(l.list.size.h, 3 * WORKTREES_ROW);
        assert_eq!(l.entries[0].0.loc.y, l.list.loc.y);
        assert_eq!(
            l.entries[1].0.loc.y,
            l.list.loc.y + WORKTREES_LABEL - WORKTREES_ROW / 2
        );
        let bottom = l.list.loc.y + l.list.size.h;
        let (last, _) = l.entries.last().unwrap();
        assert!(last.loc.y < bottom && last.loc.y + last.size.h > bottom);
        // At rest, the label under the last rows peeks in their room, and
        // the list keeps its height.
        let rest = worktrees_layout(
            &WorktreesView {
                part: 0.0,
                ..scrolled.clone()
            },
            size,
        );
        assert_eq!(rest.list, l.list);
        let (label_under, _) = rest.entries[3];
        assert!(label_under.loc.y < bottom && label_under.loc.y + label_under.size.h > bottom);
        // The label covers the worktree under it; under the list is none.
        let x = l.list.loc.x as f64 + 40.0;
        let on_label = (x, l.list.loc.y as f64 + 4.0);
        assert_eq!(
            worktrees_hit(&scrolled, &l, on_label.into()),
            Some(WorktreesHit::Entry(0))
        );
        let under: Point<f64, Logical> = (x, bottom as f64 + 2.0).into();
        assert!(l.entries[4].0.to_f64().contains(under));
        assert_eq!(
            worktrees_hit(&scrolled, &l, under),
            Some(WorktreesHit::Panel)
        );
        // The next repository's label pushes the one on top out.
        let pushing = WorktreesView {
            entries: vec![
                label("r"),
                line("done", Vec::new()),
                label("s"),
                line("other", Vec::new()),
            ],
            part: 0.75,
            ..scrolled
        };
        let l = worktrees_layout(&pushing, size);
        let (top, next) = (l.entries[0].0, l.entries[2].0);
        assert!(top.loc.y < l.list.loc.y);
        assert_eq!(top.loc.y + top.size.h, next.loc.y);
        assert_eq!(l.list.size.h, 3 * WORKTREES_ROW);
    }

    #[test]
    fn the_buttons_labels_fit() {
        use WorktreeButton::*;
        for button in [Diff, Checkout, Merge, Delete] {
            let w = measure(button.label(), SMALL_FONT_SIZE).unwrap_or(0.0);
            assert!(w + 16.0 <= button.width() as f32, "{button:?}: {w}");
        }
    }

    #[test]
    fn the_page_stays_in_bounds() {
        let tiny = worktrees_page((800, 200).into());
        let tall = worktrees_page((800, 4000).into());
        assert_eq!(tiny, WORKTREES_MIN_ROWS as usize);
        assert_eq!(tall, WORKTREES_MAX_ROWS as usize);
    }

    #[test]
    fn a_full_page_keeps_the_footer_on_the_output() {
        let hint = |k: &str| (k.to_owned(), "does a thing".to_owned());
        let hints: Vec<_> = ["Enter", "Ctrl+S", "Ctrl+Shift+S", "Del", "Shift+Del", "Ctrl+O"]
            .into_iter()
            .map(hint)
            .collect();
        for h in (560..=1200).step_by(40) {
            let size: Size<i32, Logical> = (1280, h).into();
            let page = worktrees_page(size) as i32;
            if page == WORKTREES_MIN_ROWS {
                continue;
            }
            let v = WorktreesView {
                list_height: page * WORKTREES_ROW,
                hints: hints.clone(),
                ..view()
            };
            let l = worktrees_layout(&v, size);
            let bottom = l.panel.loc.y + l.panel.size.h;
            assert!(bottom <= h - HELP_MARGIN, "{h}: {bottom}");
        }
    }

    /// A list of two repositories' worktrees, longer than it is high, at
    /// rest.
    fn long() -> WorktreesView {
        let label = |name: &str| WorktreeEntry::Repo(name.into());
        WorktreesView {
            entries: vec![
                label("r"),
                line("wip", Vec::new()),
                line("done", Vec::new()),
                line("old", Vec::new()),
                label("s"),
                line("other", Vec::new()),
            ],
            list_height: 3 * WORKTREES_ROW,
            selected: Some(1),
            ..view()
        }
    }

    /// The parts `layers` shows `view` with on an output of `size` at
    /// `scale`, faded in: the rows' (front to back) then the frame's.
    fn faded_in(
        layers: &mut WorktreesLayers,
        view: &WorktreesView,
        size: Size<i32, Logical>,
        theme: crate::chrome::Theme,
        scale: f64,
        painter: &Painter,
    ) -> Vec<Part> {
        let now = std::time::Instant::now();
        let later = now + std::time::Duration::from_secs(1);
        let mut parts = Vec::new();
        for at in [now, later] {
            parts = layers.parts(Some(view.clone()), size, theme, scale, at, painter);
        }
        parts
    }

    /// `faded_in` on a dark 1080p output at scale 1.
    fn shown(layers: &mut WorktreesLayers, view: &WorktreesView, painter: &Painter) -> Vec<Part> {
        let theme = crate::chrome::Theme::Dark;
        faded_in(layers, view, (1920, 1080).into(), theme, 1.0, painter)
    }

    #[test]
    fn scrolling_part_of_a_row_draws_nothing_again() {
        let painter = Painter::new();
        let mut layers = WorktreesLayers::default();
        let rest = long();
        let parts = shown(&mut layers, &rest, &painter);
        let drawn = layers.rows.drawn();
        assert_eq!(layers.frames, 1);
        // The label on top and the worktrees in view, peeking under the
        // list too, then the frame.
        assert_eq!(drawn, parts.len() - 1);
        assert!(drawn >= 4, "{drawn}");
        let list = worktrees_layout(&rest, (1920, 1080).into()).list;
        assert!(parts[..drawn].iter().all(|p| p.clip == Some(list)));
        for part in [0.1, 0.37, 0.5] {
            let scrolled = WorktreesView {
                part,
                ..rest.clone()
            };
            let moved = shown(&mut layers, &scrolled, &painter);
            assert_eq!((layers.rows.drawn(), layers.frames), (drawn, 1), "{part}");
            // The label stays on top; the worktrees move up by the part
            // of the first, to the fraction of a pixel.
            assert_eq!(moved[0].loc, parts[0].loc);
            let up = part * WORKTREES_ROW as f64;
            assert!((parts[1].loc.y - moved[1].loc.y - up).abs() < 1e-9);
        }
        // Further, only the next repository's label coming into view is.
        let further = WorktreesView {
            part: 0.8,
            ..rest.clone()
        };
        assert_eq!(
            shown(&mut layers, &further, &painter).len(),
            parts.len() + 1
        );
        assert_eq!((layers.rows.drawn(), layers.frames), (drawn + 1, 1));
        // Stretched past the start, everything in the list moves down.
        let over = WorktreesView {
            overscroll: 12.5,
            ..rest.clone()
        };
        let stretched = shown(&mut layers, &over, &painter);
        assert_eq!((layers.rows.drawn(), layers.frames), (drawn + 1, 1));
        assert_eq!(stretched[0].loc.y, parts[0].loc.y + 12.5);
        assert_eq!(stretched[1].loc.y, parts[1].loc.y + 12.5);
        // A row selected anew is drawn again, the frame not.
        let reselected = WorktreesView {
            selected: Some(2),
            ..rest
        };
        shown(&mut layers, &reselected, &painter);
        assert_eq!((layers.rows.drawn(), layers.frames), (drawn + 3, 1));
    }

    #[test]
    fn the_scrollbar_thumb_goes_in_the_gutter_and_moves_alone() {
        let painter = Painter::new();
        let mut layers = WorktreesLayers::default();
        let at = |down: f64| WorktreesView {
            bar: Some(crate::chrome::ListBar {
                down,
                total: 2000.0,
            }),
            ..long()
        };
        let first = shown(&mut layers, &at(0.0), &painter);
        let moved = shown(&mut layers, &at(500.0), &painter);
        assert_eq!(layers.frames, 1);
        // Right of the entries, within the panel; in front of the rest.
        let layout = worktrees_layout(&at(0.0), (1920, 1080).into());
        let thumb = super::worktrees_thumb(&at(0.0), &layout).unwrap();
        let (list, panel) = (layout.list, layout.panel);
        assert_eq!(thumb.track.loc.x, list.loc.x + list.size.w + 1);
        assert!(thumb.track.loc.x + thumb.track.size.w < panel.loc.x + panel.size.w);
        assert_eq!(first[0].loc, (thumb.track.loc.x as f64, thumb.top).into());
        assert!(moved[0].loc.y > first[0].loc.y);
        assert_eq!(super::worktrees_thumb(&long(), &layout), None);
    }

    #[test]
    fn the_hits_follow_the_rows_as_they_move() {
        let size: Size<i32, Logical> = (1920, 1080).into();
        let rest = long();
        let still = worktrees_layout(&rest, size);
        // Stretched down past the start, the rows and their hits move with it.
        let over = WorktreesView {
            overscroll: 15.0,
            ..rest.clone()
        };
        let l = worktrees_layout(&over, size);
        let tops = entry_tops(&over, l.list.loc.y);
        for (i, ((rect, _), top)) in l.entries.iter().zip(&tops).enumerate() {
            assert_eq!(rect.loc.y, still.entries[i].0.loc.y + 15);
            assert_eq!(rect.loc.y, top.round() as i32);
        }
        let x = l.list.loc.x as f64 + 40.0;
        let at = |rect: Rect| (x, rect.loc.y as f64 + 2.0).into();
        assert_eq!(
            worktrees_hit(&over, &l, at(still.entries[2].0)),
            Some(WorktreesHit::Entry(1))
        );
        // Past the end, the label stays on top over the worktrees going on
        // under it.
        let past = WorktreesView {
            overscroll: -15.0,
            ..rest
        };
        let l = worktrees_layout(&past, size);
        assert_eq!(l.entries[0].0, still.entries[0].0);
        assert_eq!(l.entries[1].0.loc.y, still.entries[1].0.loc.y - 15);
        assert_eq!(
            worktrees_hit(&past, &l, at(still.entries[0].0)),
            Some(WorktreesHit::Entry(0))
        );
    }

    #[test]
    fn it_paints() {
        let painter = Painter::new();
        let mut v = view();
        if let WorktreeEntry::Worktree(line) = &mut v.entries[2] {
            line.state = LineState::Armed("Delete again to delete, with unmerged commits".into());
        }
        let mut layers = WorktreesLayers::default();
        assert_eq!(shown(&mut layers, &v, &painter).len(), 4);
        let empty = WorktreesView {
            entries: Vec::new(),
            selected: None,
            ..view()
        };
        let mut layers = WorktreesLayers::default();
        let light = crate::chrome::Theme::Light;
        let parts = faded_in(
            &mut layers,
            &empty,
            (1280, 800).into(),
            light,
            2.0,
            &painter,
        );
        assert_eq!(parts.len(), 1);
    }
}

//! The project commands dialog (see `crate::project_commands`): Verify's
//! checks and the setup commands of a project, a field of lines per
//! repository, on two tabs, and opened from a feature a third with the
//! checks only it runs. The list of fields scrolls; the title, the tabs and
//! the buttons stay.

use super::{
    ButtonStyle, Canvas, DIALOG_WIDTH, FONT_SIZE, FieldSpot, LABEL_FONT_SIZE, POPUP_RADIUS,
    Painter, Palette, QUIT_BODY, QUIT_BUTTON, QUIT_BUTTON_GAP, QUIT_LINE_GAP, QUIT_PADDING,
    QUIT_TITLE, Rect, Rgb, SHADOW, SMALL_FONT_SIZE, Thumb, band, button_width, dialog_buttons,
    field_text, gutter_track, phys, rect_px,
};
use crate::text_field::Line;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// The dialog's tabs for a project.
pub const COMMANDS_PROJECT_TABS: [CommandsTab; 2] = [CommandsTab::Checks, CommandsTab::Setup];
/// The dialog's tabs for a feature.
pub const COMMANDS_FEATURE_TABS: [CommandsTab; 3] = [
    CommandsTab::Checks,
    CommandsTab::Setup,
    CommandsTab::Feature,
];
/// The dialog's buttons while it can save.
pub const COMMANDS_BUTTONS: [&str; 2] = ["Cancel", "Save"];
/// The dialog's buttons once the project file was edited outside gerdr.
pub const COMMANDS_RELOAD_BUTTONS: [&str; 2] = ["Cancel", "Reload file"];
/// The button after Cancel: Save or Reload file, the dialog's default.
pub const COMMANDS_GO: usize = 1;
/// How far the dialog stays from the output's edges.
const MARGIN: i32 = 40;
/// How wide each tab is.
const TAB_W: i32 = 80;
/// The room between the tabs' outline and its segments.
const TRACK_PAD: i32 = 2;
/// How much higher a field gets for each line past its first.
const LINE_H: i32 = 20;
/// How high the row over a field is, holding its name and suggestion.
const ROW_LABEL: i32 = 26;
/// How high a borrowed check's line is, holding its button.
const BORROWED_H: i32 = 26;
/// How wide the label of the time limit's field is, and the field.
const LIMIT_LABEL: i32 = 170;
const LIMIT_FIELD: i32 = 100;
/// The room between a name and what is said beside it.
const TAG_GAP: f32 = 8.0;
/// How high the shortest list of fields is on a low output.
const LIST_MIN: i32 = 2 * (ROW_LABEL + QUIT_BUTTON);
/// How far a turn of the wheel moves the list.
pub const COMMANDS_NOTCH: i32 = 40;

/// Which commands the dialog shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CommandsTab {
    #[default]
    Checks,
    Setup,
    /// The checks only the dialog's feature runs.
    Feature,
}

impl CommandsTab {
    fn label(self) -> &'static str {
        match self {
            Self::Checks => "Checks",
            Self::Setup => "Setup",
            Self::Feature => "This feature",
        }
    }
}

/// What has the keyboard in the dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CommandsFocus {
    /// The field of `CommandsView::rows`' row.
    Row(usize),
    /// The time limit's field.
    Limit,
    /// A borrowed check, by its row and its line there.
    Borrowed(usize, usize),
}

/// A command a feature borrows from its project, as the dialog shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandsBorrowed {
    pub command: String,
    /// The feature doesn't run it.
    pub skipped: bool,
}

impl CommandsBorrowed {
    fn button(&self) -> &'static str {
        if self.skipped { "Run" } else { "Skip" }
    }
}

/// A field of the dialog, as it shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandsRow {
    /// The project it starts, once there are several.
    pub heading: Option<String>,
    /// The repository's name, or "Whole feature".
    pub label: String,
    /// What is said beside it: "in this feature", "not found".
    pub tag: Option<String>,
    /// What can fill it, while it is empty: the Use button's commands.
    pub suggestion: Option<String>,
    /// The commands the feature borrows from the project, said over the
    /// field of those it adds.
    pub borrowed: Vec<CommandsBorrowed>,
    pub line: Line,
}

/// What the project commands dialog shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandsView {
    /// The tabs it has.
    pub tabs: &'static [CommandsTab],
    pub tab: CommandsTab,
    /// What the tab's commands are, in a line.
    pub body: &'static str,
    /// How looking for the repositories went, while it says something; in
    /// the error colour when `true`.
    pub note: Option<(String, bool)>,
    pub rows: Vec<CommandsRow>,
    /// The time limit's field, on the tab that has it.
    pub limit: Option<Line>,
    pub focus: CommandsFocus,
    /// How far down the list is scrolled.
    pub down: i32,
    /// What is wrong with the form, as a sentence.
    pub error: Option<String>,
    /// The project file changed outside gerdr: it wants reloading first.
    pub stale: bool,
    pub hover: Option<CommandsHit>,
    /// The keys besides the buttons, said left of them.
    pub keys: Vec<(&'static str, &'static str)>,
}

/// What the pointer is over in the dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CommandsHit {
    Tab(CommandsTab),
    /// The field of `CommandsView::rows`' row.
    Field(usize),
    /// The Use button of `CommandsView::rows`' row.
    Use(usize),
    Limit,
    /// A borrowed check, by its row and its line there.
    Borrowed(usize, usize),
    /// 0 Cancel, 1 Save or Reload file.
    Button(usize),
}

/// Where a field of the dialog goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandsRowLayout {
    pub heading: Option<Rect>,
    pub label: Rect,
    /// The Use button, when it has something to use.
    pub use_button: Option<Rect>,
    /// What it borrows, a line each.
    pub borrowed: Vec<Rect>,
    pub field: Rect,
}

/// Where the project commands dialog's parts go, in output coordinates.
/// What is in the list stands where it is scrolled to, and shows only
/// inside `list`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandsLayout {
    pub panel: Rect,
    pub title: Rect,
    /// The tabs' outline, and its segments.
    pub track: Rect,
    pub tabs: Vec<(CommandsTab, Rect)>,
    pub body: Rect,
    pub note: Option<Rect>,
    pub list: Rect,
    pub rows: Vec<CommandsRowLayout>,
    /// The time limit's label, field and unit.
    pub limit: Option<[Rect; 3]>,
    /// How high all that is in the list holds.
    pub total: i32,
    pub error: Option<Rect>,
    /// Cancel, Save or Reload file.
    pub buttons: [Rect; 2],
}

/// How high a field of `lines` lines is.
fn field_height(lines: usize) -> i32 {
    QUIT_BUTTON + (lines.max(1) as i32 - 1) * LINE_H
}

/// How many lines `text` shows on.
fn lines_of(text: &str) -> usize {
    text.split('\n').count()
}

/// How high `view`'s list would be, all of it.
fn list_height(view: &CommandsView) -> i32 {
    let rows: i32 = (view.rows.iter())
        .map(|row| {
            let heading = if row.heading.is_some() {
                QUIT_BODY + QUIT_LINE_GAP
            } else {
                0
            };
            let borrowed = row.borrowed.len() as i32 * BORROWED_H;
            heading + ROW_LABEL + borrowed + field_height(lines_of(&row.line)) + QUIT_LINE_GAP
        })
        .sum();
    let limit = if view.limit.is_some() {
        QUIT_BUTTON + QUIT_LINE_GAP
    } else {
        0
    };
    (rows + limit - QUIT_LINE_GAP).max(0)
}

/// The labels of `view`'s buttons.
pub fn commands_buttons(stale: bool) -> [&'static str; 2] {
    match stale {
        true => COMMANDS_RELOAD_BUTTONS,
        false => COMMANDS_BUTTONS,
    }
}

/// Where the dialog's parts go, centred on an output of `size`.
pub fn commands_layout(view: &CommandsView, size: Size<i32, Logical>) -> CommandsLayout {
    let line = |shown: bool, h: i32| if shown { h + QUIT_LINE_GAP } else { 0 };
    let fixed = 2 * QUIT_PADDING
        + (QUIT_BUTTON + QUIT_LINE_GAP)
        + (QUIT_BODY + QUIT_LINE_GAP)
        + line(view.note.is_some(), QUIT_BODY)
        + QUIT_LINE_GAP
        + line(view.error.is_some(), QUIT_BODY)
        + QUIT_BUTTON;
    let total = list_height(view);
    let room = (size.h - 2 * MARGIN - fixed).max(LIST_MIN);
    let list_h = total.min(room);
    let height = fixed + list_h;
    let width = DIALOG_WIDTH.min(size.w);
    let panel = Rectangle::new(
        (
            ((size.w - width) / 2).max(0),
            ((size.h - height) / 2).max(0),
        )
            .into(),
        (width, height).into(),
    );
    let (x, mut y) = (panel.loc.x + QUIT_PADDING, panel.loc.y + QUIT_PADDING);
    let inner = width - 2 * QUIT_PADDING;
    let mut next = |h: i32| {
        let rect = Rectangle::new((x, y).into(), (inner, h).into());
        y += h + QUIT_LINE_GAP;
        rect
    };
    let header = next(QUIT_BUTTON);
    let body = next(QUIT_BODY);
    let note = view.note.as_ref().map(|_| next(QUIT_BODY));
    let list = next(list_h);
    let error = view.error.as_ref().map(|_| next(QUIT_BODY));

    let track_w = view.tabs.len() as i32 * TAB_W + 2 * TRACK_PAD;
    let track = Rectangle::new(
        (header.loc.x + header.size.w - track_w, header.loc.y).into(),
        (track_w, header.size.h).into(),
    );
    let title = Rectangle::new(
        (header.loc.x, header.loc.y + (QUIT_BUTTON - QUIT_TITLE) / 2).into(),
        ((header.size.w - track_w - QUIT_LINE_GAP).max(0), QUIT_TITLE).into(),
    );
    let segment = |i: i32| {
        Rectangle::new(
            (track.loc.x + TRACK_PAD + i * TAB_W, track.loc.y + TRACK_PAD).into(),
            (TAB_W, track.size.h - 2 * TRACK_PAD).into(),
        )
    };

    // The list, from where it is scrolled to.
    let mut at = list.loc.y - view.down;
    let rows = (view.rows.iter())
        .map(|row| {
            let heading = row.heading.as_ref().map(|_| {
                let rect = Rectangle::new((x, at).into(), (inner, QUIT_BODY).into());
                at += QUIT_BODY + QUIT_LINE_GAP;
                rect
            });
            let label = Rectangle::new((x, at).into(), (inner, ROW_LABEL).into());
            let use_button = row.suggestion.as_ref().map(|_| {
                let w = button_width(USE);
                Rectangle::new(
                    (x + inner - w, at + (ROW_LABEL - USE_H) / 2).into(),
                    (w, USE_H).into(),
                )
            });
            at += ROW_LABEL;
            let borrowed = (row.borrowed.iter())
                .map(|_| {
                    let rect = Rectangle::new((x, at).into(), (inner, BORROWED_H).into());
                    at += BORROWED_H;
                    rect
                })
                .collect();
            let h = field_height(lines_of(&row.line));
            let field = Rectangle::new((x, at).into(), (inner, h).into());
            at += h + QUIT_LINE_GAP;
            CommandsRowLayout {
                heading,
                label,
                use_button,
                borrowed,
                field,
            }
        })
        .collect();
    let limit = view.limit.as_ref().map(|_| {
        let cell =
            |from: i32, w: i32| Rectangle::new((x + from, at).into(), (w, QUIT_BUTTON).into());
        [
            cell(0, LIMIT_LABEL),
            cell(LIMIT_LABEL, LIMIT_FIELD),
            cell(
                LIMIT_LABEL + LIMIT_FIELD + QUIT_LINE_GAP,
                inner - LIMIT_LABEL - LIMIT_FIELD - QUIT_LINE_GAP,
            ),
        ]
    });
    let buttons = dialog_buttons(panel, &commands_buttons(view.stale));
    CommandsLayout {
        panel,
        title,
        track,
        tabs: (view.tabs.iter().enumerate())
            .map(|(i, tab)| (*tab, segment(i as i32)))
            .collect(),
        body,
        note,
        list,
        rows,
        limit,
        total,
        error,
        buttons: [buttons[0], buttons[1]],
    }
}

/// The Use button's label and height.
const USE: &str = "Use";
const USE_H: i32 = 22;

impl CommandsLayout {
    /// What `pos` is over.
    pub fn hit(&self, pos: Point<f64, Logical>) -> Option<CommandsHit> {
        let on = |r: &Rect| r.to_f64().contains(pos);
        if let Some(button) = self.buttons.iter().position(on) {
            return Some(CommandsHit::Button(button));
        }
        if let Some((tab, _)) = self.tabs.iter().find(|(_, rect)| on(rect)) {
            return Some(CommandsHit::Tab(*tab));
        }
        // What is scrolled out of the list isn't there.
        if !on(&self.list) {
            return None;
        }
        if let Some([label, field, _]) = &self.limit
            && (on(label) || on(field))
        {
            return Some(CommandsHit::Limit);
        }
        self.rows.iter().enumerate().find_map(|(i, row)| {
            if row.use_button.as_ref().is_some_and(on) {
                return Some(CommandsHit::Use(i));
            }
            if let Some(line) = row.borrowed.iter().position(on) {
                return Some(CommandsHit::Borrowed(i, line));
            }
            // A click on a name goes to its field, as on a form.
            (on(&row.label) || on(&row.field)).then_some(CommandsHit::Field(i))
        })
    }

    /// What the pointer can press.
    pub fn controls(&self) -> Vec<Rect> {
        let useful = (self.rows.iter())
            .filter_map(|row| row.use_button)
            .chain(self.rows.iter().flat_map(|row| row.borrowed.iter().copied()))
            .filter_map(|button| clip(button, self.list));
        (self.tabs.iter().map(|(_, rect)| *rect))
            .chain(self.buttons)
            .chain(useful)
            .collect()
    }

    /// How far down the list can scroll.
    pub fn most_down(&self) -> i32 {
        (self.total - self.list.size.h).max(0)
    }

    /// Where the focused line of `view` is in the list, unscrolled: its
    /// top, and how high it is.
    pub fn focus_span(&self, view: &CommandsView) -> Option<(i32, i32)> {
        if let CommandsFocus::Borrowed(i, line) = view.focus {
            let rect = *self.rows.get(i)?.borrowed.get(line)?;
            // Its first line brings the name over it along.
            let lead = if line == 0 { ROW_LABEL } else { 0 };
            let top = rect.loc.y - self.list.loc.y + view.down;
            return Some((top - lead, BORROWED_H + lead));
        }
        let (rect, line_at) = match view.focus {
            CommandsFocus::Row(i) => {
                let row = view.rows.get(i)?;
                let before = row.line.text[..row.line.caret].matches('\n').count() as i32;
                (self.rows.get(i)?.field, before)
            }
            CommandsFocus::Limit => (self.limit?[1], 0),
            CommandsFocus::Borrowed(..) => return None,
        };
        let top = rect.loc.y - self.list.loc.y + view.down + line_at * LINE_H;
        // A field's first line brings the name over it along.
        let lead = match view.focus {
            CommandsFocus::Row(i) if line_at == 0 => {
                ROW_LABEL + view.rows[i].borrowed.len() as i32 * BORROWED_H
            }
            _ => 0,
        };
        Some((top - lead, QUIT_BUTTON + lead))
    }
}

/// The part of `rect` inside `within`.
fn clip(rect: Rect, within: Rect) -> Option<Rect> {
    rect.intersection(within)
}

/// A line of a field of several: its text, with the caret and selection of
/// the whole as far as they are in it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SubLine {
    line: Line,
    /// The byte of the whole where it starts.
    start: usize,
    has_caret: bool,
}

/// The lines of `whole`.
fn sub_lines(whole: &Line) -> Vec<SubLine> {
    let mut start = 0;
    let mut lines = Vec::new();
    for text in whole.text.split('\n') {
        let end = start + text.len();
        let inside = |at: usize| at.clamp(start, end) - start;
        lines.push(SubLine {
            line: Line {
                text: text.to_owned(),
                caret: inside(whole.caret),
                anchor: inside(whole.anchor),
            },
            start,
            has_caret: (start..=end).contains(&whole.caret),
        });
        start = end + 1;
    }
    lines
}

/// The band of `field`'s line `i`.
fn line_rect(field: Rect, i: usize) -> Rect {
    Rectangle::new(
        (field.loc.x, field.loc.y + i as i32 * LINE_H).into(),
        (field.size.w, QUIT_BUTTON).into(),
    )
}

/// The dialog's field and line at `pos`, and the byte its text starts at,
/// for the pointer: only the field with the keyboard is there. Above or
/// below its lines, the nearest. None past the field while `pos` isn't on
/// it, unless `held`: a selection being dragged reaches out of it.
pub fn commands_field_at(
    view: &CommandsView,
    size: Size<i32, Logical>,
    scale: f64,
    pos: Point<f64, Logical>,
    held: bool,
) -> Option<(FieldSpot, usize)> {
    let layout = commands_layout(view, size);
    let (area, line) = match view.focus {
        CommandsFocus::Row(i) => (layout.rows.get(i)?.field, &view.rows.get(i)?.line),
        CommandsFocus::Limit => (layout.limit?[1], view.limit.as_ref()?),
        CommandsFocus::Borrowed(..) => return None,
    };
    let shown = clip(area, layout.list)?;
    if !held && !shown.to_f64().contains(pos) {
        return None;
    }
    let lines = sub_lines(line);
    let at = ((pos.y - area.loc.y as f64) / LINE_H as f64).floor();
    let sub = &lines[(at.max(0.0) as usize).min(lines.len() - 1)];
    let spot = FieldSpot {
        area: shown,
        text: field_text(area, scale),
        size: FONT_SIZE,
        line: sub.line.clone(),
    };
    Some((spot, sub.start))
}

/// The dialog's field with the keyboard, as the pointer's shape cares.
pub fn commands_field(
    view: &CommandsView,
    size: Size<i32, Logical>,
    scale: f64,
) -> Option<FieldSpot> {
    let layout = commands_layout(view, size);
    let (area, line) = match view.focus {
        CommandsFocus::Row(i) => (layout.rows.get(i)?.field, &view.rows.get(i)?.line),
        CommandsFocus::Limit => (layout.limit?[1], view.limit.as_ref()?),
        CommandsFocus::Borrowed(..) => return None,
    };
    Some(FieldSpot {
        area: clip(area, layout.list)?,
        text: field_text(area, scale),
        size: FONT_SIZE,
        line: line.clone(),
    })
}

impl Painter {
    /// A field of lines in `rect`: each as one line of a field is, the
    /// caret and selection where they are when `focused`, and `placeholder`
    /// while it is empty.
    #[allow(clippy::too_many_arguments)]
    fn command_lines(
        &self,
        c: &mut Canvas,
        whole: &Line,
        placeholder: &str,
        rect: Rect,
        focused: bool,
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let (x, y, w, h) = rect_px(rect, scale);
        let (b, r) = (px(1).max(1) as f32, super::BUTTON_RADIUS * s);
        c.rounded(x, y, w, h, r, p.border);
        c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, p.element);
        let text = field_text(rect, scale);
        for (i, sub) in sub_lines(whole).iter().enumerate() {
            let line_band = band(line_rect(rect, i), scale);
            let selection = sub.line.selection();
            if focused && !sub.has_caret && !selection.is_empty() {
                // The caret's line paints its own selection, with the
                // caret; the rest of it goes over these.
                let width = |t: &str| self.text_width(t, FONT_SIZE, scale);
                let from = text.0 + width(&sub.line[..selection.start]);
                let to = (text.0 + width(&sub.line[..selection.end])).min(text.1);
                let inset = px(3) as f32;
                c.rounded(
                    from - s,
                    line_band.0 as f32 + inset,
                    to - from + 2.0 * s,
                    line_band.1 as f32 - 2.0 * inset,
                    2.0 * s,
                    p.selected,
                );
            }
            self.edit_line(
                c,
                &sub.line,
                if i == 0 { placeholder } else { "" },
                text,
                line_band,
                focused && sub.has_caret,
                FONT_SIZE,
                scale,
                p,
            );
        }
    }

    /// The project commands dialog for an output of `size`, inside a
    /// margin of `SHADOW` holding its drop shadow.
    pub(super) fn commands_dialog(
        &self,
        view: &CommandsView,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let layout = commands_layout(view, size);
        let panel = layout.panel;
        let mut c = self.popup(panel.size, POPUP_RADIUS, scale, p);
        let shift = |r: Rect| {
            Rectangle::new(
                r.loc + Point::from((SHADOW - panel.loc.x, SHADOW - panel.loc.y)),
                r.size,
            )
        };
        let line = |c: &mut Canvas, text: &str, rect: Rect, size: f32, color: Rgb| {
            let rect = shift(rect);
            let (x, end) = phys(rect, scale);
            self.text(c, text, size, x, end, band(rect, scale), scale, color)
        };
        line(
            &mut c,
            match view.tab {
                CommandsTab::Feature => "Feature checks",
                _ => "Project commands",
            },
            layout.title,
            LABEL_FONT_SIZE,
            p.text,
        );
        let tabs: Vec<_> = (layout.tabs.iter())
            .map(|(tab, rect)| (tab.label(), shift(*rect)))
            .collect();
        let index = |tab: CommandsTab| view.tabs.iter().position(|t| *t == tab);
        let hover = match view.hover {
            Some(CommandsHit::Tab(tab)) => index(tab),
            _ => None,
        };
        self.segmented(
            &mut c,
            shift(layout.track),
            &tabs,
            index(view.tab).unwrap_or(0),
            hover,
            scale,
            p,
        );
        line(&mut c, view.body, layout.body, FONT_SIZE, p.muted);
        if let (Some(rect), Some((note, bad))) = (layout.note, &view.note) {
            let color = if *bad { p.error } else { p.muted };
            line(&mut c, note, rect, FONT_SIZE, color);
        }

        let list = shift(layout.list);
        let (x, y, w, h) = rect_px(list, scale);
        c.clipped(x as i32, y as i32, w as i32, h as i32, |c| {
            for (i, (row, rects)) in view.rows.iter().zip(&layout.rows).enumerate() {
                if let (Some(heading), Some(rect)) = (&row.heading, rects.heading) {
                    line(c, heading, rect, FONT_SIZE, p.text);
                }
                self.commands_row_label(c, view, row, rects, i, &shift, scale, p);
                for (k, (borrowed, rect)) in row.borrowed.iter().zip(&rects.borrowed).enumerate() {
                    let on_it = view.focus == CommandsFocus::Borrowed(i, k);
                    let hit = view.hover == Some(CommandsHit::Borrowed(i, k));
                    self.borrowed_line(c, borrowed, shift(*rect), (on_it, hit), scale, p);
                }
                let focused = view.focus == CommandsFocus::Row(i);
                let placeholder = match view.tab {
                    CommandsTab::Checks => "No checks",
                    CommandsTab::Setup => "No setup commands",
                    CommandsTab::Feature => "Nothing added",
                };
                self.command_lines(
                    c,
                    &row.line,
                    placeholder,
                    shift(rects.field),
                    focused,
                    scale,
                    p,
                );
            }
            if let (Some(limit), Some(rects)) = (&view.limit, layout.limit) {
                line(c, "Time limit per check", rects[0], FONT_SIZE, p.muted);
                self.field(
                    c,
                    limit,
                    "1200",
                    shift(rects[1]),
                    view.focus == CommandsFocus::Limit,
                    scale,
                    p,
                );
                line(c, "seconds", rects[2], FONT_SIZE, p.muted);
            }
        });
        let track = gutter_track(list.loc.x + list.size.w, list);
        let bar = super::ListBar {
            down: view.down as f64,
            total: layout.total as f64,
        };
        if let Some(thumb) = Thumb::of(track, list.size.h as f64, bar) {
            self.thumb_in(&mut c, &thumb, scale, p);
        }
        if let (Some(rect), Some(error)) = (layout.error, &view.error) {
            line(&mut c, error, rect, FONT_SIZE, p.error);
        }

        // Left of the buttons, the keys besides them; then Cancel and the
        // default.
        let first = shift(layout.buttons[0]);
        let left = shift(layout.title).loc.x;
        let row = Rectangle::new((left, first.loc.y).into(), (0, first.size.h).into());
        self.dialog_keys(
            &mut c,
            &view.keys,
            row,
            first.loc.x - QUIT_BUTTON_GAP,
            scale,
            p,
        );
        let labels = commands_buttons(view.stale);
        for (i, (rect, label)) in layout.buttons.iter().zip(labels).enumerate() {
            let style = if i == COMMANDS_GO {
                ButtonStyle::Primary
            } else {
                ButtonStyle::Plain
            };
            let hovered = view.hover == Some(CommandsHit::Button(i));
            self.button(
                &mut c,
                label,
                shift(*rect),
                (hovered, false),
                style,
                scale,
                p,
            );
        }
        c.into_buffer(false)
    }

    /// A borrowed check's line: what it is, and at the right the button
    /// that skips it or runs it again. `(focused, hovered)` light it up.
    fn borrowed_line(
        &self,
        c: &mut Canvas,
        borrowed: &CommandsBorrowed,
        rect: Rect,
        (focused, hovered): (bool, bool),
        scale: f64,
        p: &Palette,
    ) {
        let hover = if hovered { 1.0 } else { 0.0 };
        self.row_highlight(c, rect, (focused, hover), scale, p);
        let label = borrowed.button();
        let w = button_width(label);
        let button = Rectangle::new(
            (
                rect.loc.x + rect.size.w - w,
                rect.loc.y + (BORROWED_H - USE_H) / 2,
            )
                .into(),
            (w, USE_H).into(),
        );
        let style = ButtonStyle::Plain;
        self.button(c, label, button, (hovered, false), style, scale, p);
        let (x, _) = phys(rect, scale);
        let end = phys(button, scale).0 - TAG_GAP * scale as f32;
        let (text, color) = match borrowed.skipped {
            true => (format!("skipped: {}", borrowed.command), p.muted),
            false => (format!("project: {}", borrowed.command), p.text),
        };
        let row_band = band(rect, scale);
        self.text(c, &text, SMALL_FONT_SIZE, x, end, row_band, scale, color);
    }

    /// The row over a field: its name, what is said of it, and at the
    /// right what can fill it, with the button that does.
    #[allow(clippy::too_many_arguments)]
    fn commands_row_label(
        &self,
        c: &mut Canvas,
        view: &CommandsView,
        row: &CommandsRow,
        rects: &CommandsRowLayout,
        i: usize,
        shift: &impl Fn(Rect) -> Rect,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        let rect = shift(rects.label);
        let (x, mut end) = phys(rect, scale);
        let row_band = band(rect, scale);
        if let Some(button) = rects.use_button {
            let hovered = view.hover == Some(CommandsHit::Use(i));
            let button = shift(button);
            let style = ButtonStyle::Plain;
            self.button(c, USE, button, (hovered, false), style, scale, p);
            end = phys(button, scale).0 - TAG_GAP * s;
        }
        let name_w = self.text_width(&row.label, FONT_SIZE, scale);
        let tag_x = x + name_w + TAG_GAP * s;
        let tag_w = (row.tag.as_ref()).map_or(0.0, |tag| {
            self.text_width(tag, SMALL_FONT_SIZE, scale) + TAG_GAP * s
        });
        if let Some(suggestion) = &row.suggestion {
            let text = format!("suggested: {suggestion}");
            let w = self.text_width(&text, SMALL_FONT_SIZE, scale);
            let from = (end - w).max(tag_x + tag_w);
            self.text(
                c,
                &text,
                SMALL_FONT_SIZE,
                from,
                end,
                row_band,
                scale,
                p.muted,
            );
            end = from - TAG_GAP * s;
        }
        self.text(c, &row.label, FONT_SIZE, x, end, row_band, scale, p.text);
        if let Some(tag) = &row.tag {
            self.text(
                c,
                tag,
                SMALL_FONT_SIZE,
                tag_x,
                end,
                row_band,
                scale,
                p.muted,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(label: &str, text: &str) -> CommandsRow {
        CommandsRow {
            heading: None,
            label: label.into(),
            tag: None,
            suggestion: None,
            borrowed: Vec::new(),
            line: text.into(),
        }
    }

    fn view(rows: Vec<CommandsRow>) -> CommandsView {
        CommandsView {
            tabs: &COMMANDS_PROJECT_TABS,
            tab: CommandsTab::Checks,
            body: "",
            note: None,
            rows,
            limit: Some(Line::default()),
            focus: CommandsFocus::Row(0),
            down: 0,
            error: None,
            stale: false,
            hover: None,
            keys: Vec::new(),
        }
    }

    const BIG: (i32, i32) = (1400, 1000);

    fn centre(r: Rect) -> Point<f64, Logical> {
        let r = r.to_f64();
        (r.loc.x + r.size.w / 2.0, r.loc.y + r.size.h / 2.0).into()
    }

    #[test]
    fn a_line_more_makes_the_field_higher() {
        let one = commands_layout(&view(vec![row("api", "a")]), BIG.into());
        let two = commands_layout(&view(vec![row("api", "a\nb")]), BIG.into());
        assert_eq!(two.rows[0].field.size.h - one.rows[0].field.size.h, LINE_H);
        assert_eq!(two.panel.size.h - one.panel.size.h, LINE_H);
    }

    #[test]
    fn it_fits_a_low_output_and_the_list_scrolls() {
        let rows = (0..12).map(|i| row(&format!("r{i}"), "")).collect();
        let v = view(rows);
        let layout = commands_layout(&v, (870, 500).into());
        assert!(layout.panel.size.h <= 500);
        assert!(layout.list.size.h < layout.total);
        assert_eq!(layout.most_down(), layout.total - layout.list.size.h);
    }

    #[test]
    fn what_is_scrolled_out_of_the_list_takes_no_clicks() {
        let rows: Vec<_> = (0..12).map(|i| row(&format!("r{i}"), "")).collect();
        let mut v = view(rows);
        let layout = commands_layout(&v, (870, 500).into());
        let last = layout.rows[11].field;
        assert!(!layout.list.overlaps(last));
        assert_eq!(layout.hit(centre(last)), None);
        v.down = layout.most_down();
        let layout = commands_layout(&v, (870, 500).into());
        let last = layout.rows[11].field;
        assert_eq!(layout.hit(centre(last)), Some(CommandsHit::Field(11)));
        // The buttons stay, wherever the list stands.
        assert_eq!(
            layout.hit(centre(layout.buttons[1])),
            Some(CommandsHit::Button(1))
        );
    }

    #[test]
    fn its_parts_answer_the_pointer() {
        let mut v = view(vec![row("api", "")]);
        v.rows[0].suggestion = Some("cargo test".into());
        let layout = commands_layout(&v, BIG.into());
        let rows = &layout.rows[0];
        assert_eq!(
            layout.hit(centre(layout.tabs[1].1)),
            Some(CommandsHit::Tab(CommandsTab::Setup))
        );
        assert_eq!(
            layout.hit(centre(rows.use_button.unwrap())),
            Some(CommandsHit::Use(0))
        );
        assert_eq!(layout.hit(centre(rows.field)), Some(CommandsHit::Field(0)));
        // A click on a name goes to its field.
        let name = rows.label.loc.to_f64() + Point::from((4.0, 4.0));
        assert_eq!(layout.hit(name), Some(CommandsHit::Field(0)));
        assert_eq!(
            layout.hit(centre(layout.limit.unwrap()[1])),
            Some(CommandsHit::Limit)
        );
        assert_eq!(
            layout.hit(centre(layout.buttons[0])),
            Some(CommandsHit::Button(0))
        );
    }

    #[test]
    fn what_a_feature_borrows_stands_over_its_field_and_goes_to_it() {
        let mut v = view(vec![row("api", "")]);
        v.tabs = &COMMANDS_FEATURE_TABS;
        v.tab = CommandsTab::Feature;
        v.limit = None;
        let plain = commands_layout(&v, BIG.into());
        assert_eq!(
            plain.hit(centre(plain.tabs[2].1)),
            Some(CommandsHit::Tab(CommandsTab::Feature))
        );
        let borrowed = |command: &str| CommandsBorrowed {
            command: command.into(),
            skipped: false,
        };
        v.rows[0].borrowed = vec![borrowed("cargo test"), borrowed("cargo clippy")];
        let layout = commands_layout(&v, BIG.into());
        let rows = &layout.rows[0];
        assert_eq!(rows.field.loc.y - rows.label.loc.y, ROW_LABEL + 2 * BORROWED_H);
        assert_eq!(layout.total - plain.total, 2 * BORROWED_H);
        assert_eq!(
            layout.hit(centre(rows.borrowed[1])),
            Some(CommandsHit::Borrowed(0, 1))
        );
        assert!(layout.controls().contains(&rows.borrowed[1]));
        // Its first line brings them along, with the name.
        let (top, height) = layout.focus_span(&v).unwrap();
        assert_eq!(top, rows.label.loc.y - layout.list.loc.y);
        assert_eq!(height, ROW_LABEL + 2 * BORROWED_H + QUIT_BUTTON);
        // On a borrowed check, no field takes the pointer.
        v.focus = CommandsFocus::Borrowed(0, 1);
        let (top, height) = layout.focus_span(&v).unwrap();
        assert_eq!(top, rows.borrowed[1].loc.y - layout.list.loc.y);
        assert_eq!(height, BORROWED_H);
        assert!(commands_field(&v, BIG.into(), 1.0).is_none());
    }

    #[test]
    fn a_line_shows_only_its_part_of_the_caret_and_selection() {
        let whole = Line {
            text: "abc\ndef\nghi".into(),
            caret: 5,
            anchor: 2,
        };
        let lines = sub_lines(&whole);
        assert_eq!(lines.len(), 3);
        assert_eq!(
            (lines[0].line.selection(), lines[0].has_caret),
            (2..3, false)
        );
        assert_eq!(
            (lines[1].line.selection(), lines[1].has_caret),
            (0..1, true)
        );
        assert_eq!(
            (lines[2].line.selection(), lines[2].has_caret),
            (0..0, false)
        );
        assert_eq!((lines[1].start, lines[2].start), (4, 8));
    }

    #[test]
    fn the_pointer_finds_the_line_it_is_on() {
        let v = view(vec![row("api", "abc\ndef")]);
        let layout = commands_layout(&v, BIG.into());
        let field = layout.rows[0].field;
        let at = |y: i32| Point::from((field.loc.x as f64 + 5.0, (field.loc.y + y) as f64));
        let (first, base) = commands_field_at(&v, BIG.into(), 1.0, at(5), false).unwrap();
        assert_eq!((first.line.as_str(), base), ("abc", 0));
        let (second, base) = commands_field_at(&v, BIG.into(), 1.0, at(LINE_H + 5), false).unwrap();
        assert_eq!((second.line.as_str(), base), ("def", 4));
        // Off the field it counts only when dragging from it, then on the
        // nearest line.
        let below = at(field.size.h + 40);
        assert!(commands_field_at(&v, BIG.into(), 1.0, below, false).is_none());
        let (held, base) = commands_field_at(&v, BIG.into(), 1.0, below, true).unwrap();
        assert_eq!((held.line.as_str(), base), ("def", 4));
    }

    #[test]
    fn the_focused_line_is_found_with_the_list_scrolled_or_not() {
        let mut v = view(vec![row("api", "a"), row("web", "b\nc")]);
        v.focus = CommandsFocus::Row(1);
        let plain = commands_layout(&v, BIG.into()).focus_span(&v).unwrap();
        v.down = 30;
        assert_eq!(
            commands_layout(&v, BIG.into()).focus_span(&v).unwrap(),
            plain
        );
        // Its caret is on the second line.
        v.down = 0;
        v.rows[1].line.caret = 0;
        v.rows[1].line.anchor = 0;
        let first = commands_layout(&v, BIG.into()).focus_span(&v).unwrap();
        assert_eq!(plain.0 + plain.1 - (first.0 + first.1), LINE_H);
    }
}

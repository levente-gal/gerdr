//! The project menu under the sidebar header: its list, the new project
//! form and the project settings form.

use super::layers::{Part, RowCache};
use super::{
    BROWSE_WIDTH, BUTTON_RADIUS, CAP_RADIUS, CARET_WIDTH, Canvas, DETAIL_FONT_SIZE, ERROR_HEIGHT,
    FIELD_GAP, FONT_SIZE, FORM_BUTTONS, FORM_FIELD, FORM_GAP, FORM_LABEL_WIDTH, FORM_PADDING,
    FORM_ROW, FieldSpot, HELP_MARGIN, HELP_PADDING, HELP_RADIUS, KIND_CHOICES, KIND_HEIGHT,
    LABEL_FONT_SIZE, MENU_MARK_GAP, MENU_MARK_WIDTH, MENU_RADIUS, PADDING, PROJECT_MENU_FOOTER,
    PROJECT_MENU_INPUT, PROJECT_MENU_MAX_ROWS, PROJECT_MENU_MIN_ROWS, PROJECT_MENU_PADDING,
    PROJECT_MENU_ROW, PROJECT_MENU_SECTION, PROJECT_MENU_TOP, PROJECT_MENU_WIDTH, Painter, Palette,
    ProjectRow, QUIT_BUTTON, QUIT_BUTTON_GAP, RADIO_SIZE, RENAME_FIELD, RENAME_INSET, ROW_BUTTON,
    ROW_BUTTON_GAP, ROW_BUTTON_INSET, ROW_INSET, Rect, SETTINGS_BUTTONS, SHADOW, SMALL_FONT_SIZE,
    SUGGESTION_ROW, band, commands_prompt, cross, field_text, file, focus_ring, folder,
    footer_hint_spots, hint_at, hint_strs, icon_sdf, magnifier, mix, pencil, phys, plus, rect_px,
    stop,
};
use crate::WorkspaceKind;
use crate::project_list::RowTops;
use crate::text_field::Line;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// What the project menu shows: in list mode the query, the entries in view
/// and the selection; in form mode the new project's form.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectMenuView {
    /// What was typed to filter the projects; empty in form mode.
    pub query: Line,
    /// The entries in view, scrolled.
    pub entries: Vec<MenuEntry>,
    /// The selected entry, by index into `entries`.
    pub selected: Option<usize>,
    pub hover: Option<MenuHit>,
    /// Set in form mode.
    pub form: Option<FormView>,
    /// Set in settings mode.
    pub settings: Option<SettingsView>,
    /// The footer's key hints, as (key, what it does).
    pub hints: Vec<(String, String)>,
    /// How far the hover pill has faded in, 0 to 16.
    pub hover_alpha: u8,
    /// How far the entries are moved up from resting with the first one at
    /// the top, in logical pixels: part of the way to the next entry while
    /// scrolled, less than none while pulled down past the start, and more
    /// while pulled up past the end. The entries reach down past the list,
    /// and one may be shown in part at its bottom.
    pub part: f64,
    /// Where the list stands, for its scrollbar; none while it all shows,
    /// and in form and settings mode.
    pub bar: Option<super::ListBar>,
}

/// An entry of the project menu's list.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuEntry {
    /// A section label ("Current", "Recent"); not selectable.
    Section(String),
    Project {
        row: ProjectRow,
        /// The Alt+digit that opens it, 1 to 9.
        digit: Option<u8>,
        /// Set while renaming it: the name typed so far.
        renaming: Option<Line>,
        /// Delete was pressed on it once.
        confirm_forget: bool,
        /// The characters of "name detail" the query matched.
        hits: Vec<usize>,
    },
    /// Start a new project: the label, as `New project "blo"…`.
    New(String),
    /// Start a new project in this folder ("~/Sites/blog").
    Folder(String),
}

/// The project menu's form for a new project.
#[derive(Clone, Debug, PartialEq)]
pub struct FormView {
    pub name: Line,
    /// The folder as typed; empty for none.
    pub folder: Line,
    /// What the folder opens as.
    pub kind: WorkspaceKind,
    pub focus: FormField,
    /// The folders offered for what was typed in the folder field, as
    /// "~/Sites/blog"; empty when the list is closed.
    pub suggestions: Vec<String>,
    pub suggestion: Option<usize>,
    /// Why the project can't be created as it is, under the field at fault
    /// (the name or the folder).
    pub error: Option<(FormField, String)>,
}

/// A part of the project menu's form that takes the keyboard, in Tab order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormField {
    Name,
    Folder,
    Browse,
    Kind,
    Cancel,
    Create,
}

/// The project menu's form for a project's start and stop commands.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsView {
    /// The project's name.
    pub name: String,
    /// Its start commands, as typed.
    pub before: Line,
    /// Its stop commands, as typed.
    pub after: Line,
    /// Where they run, as "~/Work/app".
    pub dir: String,
    pub focus: SettingsField,
}

/// A part of the project settings form that takes the keyboard, in Tab
/// order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsField {
    Before,
    After,
    Cancel,
    Run,
    Save,
}

/// A button shown on a project row of the menu under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowButton {
    /// Its start and stop commands.
    Settings,
    /// Its file, to edit.
    ProjectFile,
    Rename,
    Stop,
    Forget,
}

impl RowButton {
    /// What it does and the key that does the same, for its tooltip.
    pub fn tip(self) -> (&'static str, &'static str) {
        match self {
            RowButton::Settings => ("Start and stop commands", "Ctrl+E"),
            RowButton::ProjectFile => ("Edit the project file", "Ctrl+O"),
            RowButton::Rename => ("Rename", "F2"),
            RowButton::Stop => ("Stop its session", "Ctrl+Q"),
            RowButton::Forget => ("Forget the project", "Del"),
        }
    }
}

/// Where the mark of the menu entry in `rect` goes: whether a project
/// runs, or what the entry is.
pub fn menu_mark(rect: Rect) -> Rect {
    Rectangle::new(
        (rect.loc.x + PADDING as i32, rect.loc.y).into(),
        (MENU_MARK_WIDTH, rect.size.h).into(),
    )
}

/// What the pointer is over in the project menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuHit {
    Query,
    /// An entry, by index into `ProjectMenuView::entries`.
    Entry(usize),
    /// A button on an entry's row.
    Button(usize, RowButton),
    Field(FormField),
    /// A part of the settings form.
    Setting(SettingsField),
    /// One of the form's two "Opens" choices.
    Kind(WorkspaceKind),
    Suggestion(usize),
    /// A key hint in the footer, by index into `ProjectMenuView::hints`.
    Hint(usize),
    /// Inside the panel, on nothing.
    Panel,
}

/// Where the project menu's parts go, in output coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectMenuLayout {
    pub panel: Rect,
    /// The query field (list mode) or the title (form mode).
    pub query: Rect,
    /// Per entry, its rect and its row buttons' (none when not hovered).
    /// A scrolled list's first and last entries reach out of `list`.
    pub entries: Vec<(Rect, Vec<(RowButton, Rect)>)>,
    /// Where the entries show (of no height in form and settings mode).
    pub list: Rect,
    pub footer: Rect,
    /// The footer's key hints, by index into `ProjectMenuView::hints`.
    pub hints: Vec<(usize, Rect)>,
    /// Set in form mode.
    pub form: Option<FormLayout>,
    /// Set in settings mode.
    pub settings: Option<SettingsLayout>,
}

/// Where the project menu's form parts go, in output coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormLayout {
    pub name: Rect,
    pub folder: Rect,
    pub browse: Rect,
    pub kinds: [(WorkspaceKind, Rect); 2],
    pub suggestions: Vec<Rect>,
    /// The line saying why the project can't be created, under the field at
    /// fault; of no height without an error.
    pub error: Rect,
    pub cancel: Rect,
    pub create: Rect,
}

/// The footer: `PROJECT_MENU_FOOTER` high with one row of key hints, which
/// has this much room above and below; a row more for every further row,
/// up to this many.
pub(super) const FOOTER_PAD: i32 = 4;
pub(super) const FOOTER_ROW: i32 = PROJECT_MENU_FOOTER - 2 * FOOTER_PAD;
pub(super) const FOOTER_ROWS: i32 = 2;

/// How many rows the footer's `hints` take: one up to four, two beyond.
pub fn footer_rows(hints: usize) -> i32 {
    if hints > 4 { FOOTER_ROWS } else { 1 }
}

/// How high the footer is with `hints`.
pub fn footer_height(hints: usize) -> i32 {
    PROJECT_MENU_FOOTER + (footer_rows(hints) - 1) * FOOTER_ROW
}

/// Where the project menu's settings form parts go, in output coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsLayout {
    pub before: Rect,
    pub after: Rect,
    /// The line saying where the commands run.
    pub note: Rect,
    pub cancel: Rect,
    pub run: Rect,
    pub save: Rect,
}

/// Where the parts of the project menu showing `view` go, on an output of
/// `size`: below the sidebar header, reaching over the apps. The query field
/// (the title in form mode), the entries or the form, then the footer. The
/// list is as high as its entries, `project_menu_room` at most; the entries
/// it doesn't show at all are cut to nothing.
pub fn project_menu_layout(view: &ProjectMenuView, size: Size<i32, Logical>) -> ProjectMenuLayout {
    let w = PROJECT_MENU_WIDTH.min(size.w - 2 * HELP_MARGIN).max(1);
    let (x, top) = (ROW_INSET, PROJECT_MENU_TOP);
    let query = Rectangle::new((x, top).into(), (w, PROJECT_MENU_INPUT).into());
    let mut y = top + PROJECT_MENU_INPUT + PROJECT_MENU_PADDING;
    let start = y;
    let (mut entries, form, settings) = match (&view.form, &view.settings) {
        (Some(form), _) => (Vec::new(), Some(form_layout(form, x, w, &mut y)), None),
        (None, Some(_)) => (Vec::new(), None, Some(settings_layout(x, w, &mut y))),
        (None, None) => {
            let entries = menu_entries(view, x, w, &mut y);
            y = y.min(start + project_menu_room(size));
            (entries, None, None)
        }
    };
    let footer_h = footer_height(view.hints.len());
    let bottom = (y + PROJECT_MENU_PADDING + footer_h)
        .min(size.h - HELP_MARGIN)
        .max(top + PROJECT_MENU_INPUT + footer_h);
    let footer = Rectangle::new((x, bottom - footer_h).into(), (w, footer_h).into());
    let limit = footer.loc.y - PROJECT_MENU_PADDING;
    let listed = view.form.is_none() && view.settings.is_none();
    let list_h = if listed { y.min(limit) - start } else { 0 };
    let list = Rectangle::new((x, start).into(), (w, list_h.max(0)).into());
    // Entries out of the list are cut to nothing; those at its edges are
    // shown in part, cut by `list`.
    let end = list.loc.y + list.size.h;
    for (rect, buttons) in &mut entries {
        if rect.loc.y >= end || rect.loc.y + rect.size.h <= list.loc.y {
            *rect = Rectangle::new(
                (rect.loc.x, rect.loc.y.clamp(list.loc.y, end)).into(),
                (rect.size.w, 0).into(),
            );
            buttons.clear();
        }
    }
    ProjectMenuLayout {
        panel: Rectangle::new((x, top).into(), (w, bottom - top).into()),
        query,
        entries,
        list,
        footer,
        hints: footer_hint_spots(&hint_strs(&view.hints), footer),
        form,
        settings,
    }
}

/// The list's entries in a panel `w` wide at `x`, from `y` down (moved past
/// them), with the row buttons of the project row under the pointer. It
/// keeps room for one row without any.
pub(super) fn menu_entries(
    view: &ProjectMenuView,
    x: i32,
    w: i32,
    y: &mut i32,
) -> Vec<(Rect, Vec<(RowButton, Rect)>)> {
    let (x, w) = (
        x + PROJECT_MENU_PADDING,
        (w - 2 * PROJECT_MENU_PADDING).max(0),
    );
    let start = *y;
    let entries = entry_tops(view)
        .into_iter()
        .zip(&view.entries)
        .enumerate()
        .map(|(i, (top, entry))| {
            let h = entry_height(entry);
            let rect = Rectangle::new((x, start + top.round() as i32).into(), (w, h).into());
            let buttons = match entry {
                MenuEntry::Project { row, .. } if EntryLook::of(view, i).hovered => {
                    row_button_rects(&row_buttons(row), rect)
                }
                _ => Vec::new(),
            };
            (rect, buttons)
        })
        .collect();
    let height: i32 = view.entries.iter().map(entry_height).sum();
    *y = start + height.max(PROJECT_MENU_ROW);
    entries
}

/// How high `entry` is.
fn entry_height(entry: &MenuEntry) -> i32 {
    project_menu_entry_height(matches!(entry, MenuEntry::Section(_)))
}

/// How high an entry of the project menu is: a section's label, or a row.
pub fn project_menu_entry_height(section: bool) -> i32 {
    if section {
        PROJECT_MENU_SECTION
    } else {
        PROJECT_MENU_ROW
    }
}

/// Where the entries of `view` go down its list, from its top: moved up by
/// its `part`.
fn entry_tops(view: &ProjectMenuView) -> Vec<f64> {
    let mut y = -view.part;
    view.entries
        .iter()
        .map(|entry| {
            let top = y;
            y += entry_height(entry) as f64;
            top
        })
        .collect()
}

/// The scrollbar of the project menu's list laid out as `layout`, in the
/// gutter right of its entries; none while they all show.
pub fn project_menu_thumb(
    view: &ProjectMenuView,
    layout: &ProjectMenuLayout,
) -> Option<super::Thumb> {
    let list = layout.list;
    let right = list.loc.x + list.size.w - PROJECT_MENU_PADDING;
    super::Thumb::of(
        super::gutter_track(right, list),
        list.size.h as f64,
        view.bar?,
    )
}

/// The places of the project menu's entries down its list, from whether
/// each is a section's label, in order.
pub fn project_menu_places(sections: impl IntoIterator<Item = bool>) -> RowTops {
    RowTops::new(sections.into_iter().map(project_menu_entry_height))
}

/// What an entry's pixels depend on besides the entry: whether it is
/// selected, whether the pointer is on it and on which of its buttons, and
/// how far its hover pill has faded in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct EntryLook {
    selected: bool,
    hovered: bool,
    button: Option<RowButton>,
    hover_alpha: u8,
}

impl EntryLook {
    /// How entry `i` of `view` looks.
    fn of(view: &ProjectMenuView, i: usize) -> Self {
        let hovered = matches!(
            view.hover,
            Some(MenuHit::Entry(j) | MenuHit::Button(j, _)) if j == i
        );
        Self {
            selected: view.selected == Some(i),
            hovered,
            button: match view.hover {
                Some(MenuHit::Button(j, button)) if j == i => Some(button),
                _ => None,
            },
            hover_alpha: if hovered { view.hover_alpha } else { 0 },
        }
    }
}

/// The buttons a project row shows under the pointer, left to right:
/// settings, its file and rename always; stop while its session may run,
/// and forget, unless it is the current project.
pub(super) fn row_buttons(row: &ProjectRow) -> Vec<RowButton> {
    let mut buttons = vec![
        RowButton::Settings,
        RowButton::ProjectFile,
        RowButton::Rename,
    ];
    if !row.current && row.running != Some(false) {
        buttons.push(RowButton::Stop);
    }
    if !row.current {
        buttons.push(RowButton::Forget);
    }
    buttons
}

/// Square `buttons` at the right end of a row in `rect`, vertically centred.
pub(super) fn row_button_rects(buttons: &[RowButton], rect: Rect) -> Vec<(RowButton, Rect)> {
    let n = buttons.len() as i32;
    let right = rect.loc.x + rect.size.w - ROW_BUTTON_INSET;
    let left = right - n * ROW_BUTTON - (n - 1).max(0) * ROW_BUTTON_GAP;
    let top = rect.loc.y + (rect.size.h - ROW_BUTTON) / 2;
    buttons
        .iter()
        .enumerate()
        .map(|(i, button)| {
            let x = left + i as i32 * (ROW_BUTTON + ROW_BUTTON_GAP);
            (
                *button,
                Rectangle::new((x, top).into(), (ROW_BUTTON, ROW_BUTTON).into()),
            )
        })
        .collect()
}

/// The form's parts in a panel `w` wide at `x`, from `y` down (moved past
/// them): a row each for the name, the folder (with Browse at its right,
/// then the suggestions under it) and the "Opens" choices, then Cancel and
/// Create at the right. The error line, if there is one, hangs under the
/// field at fault.
pub(super) fn form_layout(form: &FormView, x: i32, w: i32, y: &mut i32) -> FormLayout {
    let rect = |x: i32, y: i32, w: i32, h: i32| Rectangle::new((x, y).into(), (w.max(0), h).into());
    let right = x + w - FORM_PADDING;
    let field_x = x + FORM_PADDING + FORM_LABEL_WIDTH;
    let field_w = (right - field_x).max(0);
    let in_row = |y: i32, h: i32| y + (FORM_ROW - h) / 2;
    // The error line under the row just laid out, if it is `field`'s.
    let error_under = |y: &mut i32, field: FormField| {
        let at = *y - (FORM_ROW - FORM_FIELD) / 2;
        match &form.error {
            Some((at_fault, _)) if *at_fault == field => {
                *y += ERROR_HEIGHT;
                Some(rect(field_x, at, field_w, ERROR_HEIGHT))
            }
            _ => None,
        }
    };

    let name = rect(field_x, in_row(*y, FORM_FIELD), field_w, FORM_FIELD);
    *y += FORM_ROW;
    let mut error = error_under(y, FormField::Name);
    let browse_w = BROWSE_WIDTH.min(field_w);
    let folder_w = field_w - browse_w - FORM_GAP;
    let folder = rect(field_x, in_row(*y, FORM_FIELD), folder_w, FORM_FIELD);
    let browse = rect(right - browse_w, folder.loc.y, browse_w, FORM_FIELD);
    *y += FORM_ROW;
    let suggestions = (0..form.suggestions.len() as i32)
        .map(|i| rect(field_x, *y + i * SUGGESTION_ROW, folder_w, SUGGESTION_ROW))
        .collect::<Vec<_>>();
    if !suggestions.is_empty() {
        *y += suggestions.len() as i32 * SUGGESTION_ROW + FORM_GAP;
    }
    if let Some(under) = error_under(y, FormField::Folder) {
        error = Some(under);
    }
    let error = error.unwrap_or_else(|| rect(field_x, *y, field_w, 0));
    let mut kind_x = field_x;
    let kinds = KIND_CHOICES.map(|(kind, _, kind_w)| {
        let kind_w = kind_w.min((right - kind_x).max(0));
        let r = rect(kind_x, in_row(*y, KIND_HEIGHT), kind_w, KIND_HEIGHT);
        kind_x += kind_w + FORM_GAP;
        (kind, r)
    });
    *y += FORM_ROW + FORM_GAP;
    let [(_, cancel_w), (_, create_w)] = FORM_BUTTONS;
    let create = rect(right - create_w, *y, create_w, QUIT_BUTTON);
    let cancel = rect(
        create.loc.x - QUIT_BUTTON_GAP - cancel_w,
        *y,
        cancel_w,
        QUIT_BUTTON,
    );
    *y += QUIT_BUTTON + FORM_GAP;
    FormLayout {
        name,
        folder,
        browse,
        kinds,
        suggestions,
        error,
        cancel,
        create,
    }
}

/// The settings form's parts in a panel `w` wide at `x`, from `y` down
/// (moved past them): a row each for the start and stop commands, the line
/// saying where they run, then Cancel, Run now and Save at the right.
pub(super) fn settings_layout(x: i32, w: i32, y: &mut i32) -> SettingsLayout {
    let rect = |x: i32, y: i32, w: i32, h: i32| Rectangle::new((x, y).into(), (w.max(0), h).into());
    let right = x + w - FORM_PADDING;
    let field_x = x + FORM_PADDING + FORM_LABEL_WIDTH;
    let field_w = (right - field_x).max(0);
    let in_row = |y: i32| y + (FORM_ROW - FORM_FIELD) / 2;

    let before = rect(field_x, in_row(*y), field_w, FORM_FIELD);
    *y += FORM_ROW;
    let after = rect(field_x, in_row(*y), field_w, FORM_FIELD);
    *y += FORM_ROW;
    let note = rect(field_x, *y, field_w, ERROR_HEIGHT);
    *y += ERROR_HEIGHT + FORM_GAP;
    let [(_, cancel_w), (_, run_w), (_, save_w)] = SETTINGS_BUTTONS;
    let save = rect(right - save_w, *y, save_w, QUIT_BUTTON);
    let run = rect(save.loc.x - QUIT_BUTTON_GAP - run_w, *y, run_w, QUIT_BUTTON);
    let cancel = rect(
        run.loc.x - QUIT_BUTTON_GAP - cancel_w,
        *y,
        cancel_w,
        QUIT_BUTTON,
    );
    *y += QUIT_BUTTON + FORM_GAP;
    SettingsLayout {
        before,
        after,
        note,
        cancel,
        run,
        save,
    }
}

/// How many rows the project menu's list holds at most on an output of
/// `size`: as many full rows as fit above its bottom margin, within bounds.
pub fn project_menu_page(size: Size<i32, Logical>) -> usize {
    (project_menu_fits(size) / PROJECT_MENU_ROW).clamp(PROJECT_MENU_MIN_ROWS, PROJECT_MENU_MAX_ROWS)
        as usize
}

/// How high the project menu's list may be on an output of `size`: as
/// high as `project_menu_page` rows, or as fits, a row at the least.
pub fn project_menu_room(size: Size<i32, Logical>) -> i32 {
    (project_menu_page(size) as i32 * PROJECT_MENU_ROW)
        .min(project_menu_fits(size))
        .max(PROJECT_MENU_ROW)
}

/// The room above the project menu's bottom margin for its list, with the
/// footer at its tallest.
fn project_menu_fits(size: Size<i32, Logical>) -> i32 {
    size.h
        - HELP_MARGIN
        - PROJECT_MENU_TOP
        - PROJECT_MENU_INPUT
        - 2 * PROJECT_MENU_PADDING
        - footer_height(usize::MAX)
}

/// What `pos` is over in the project menu showing `view` on an output of
/// `size`; `None` outside its panel.
pub fn project_menu_hit(
    view: &ProjectMenuView,
    size: Size<i32, Logical>,
    pos: Point<f64, Logical>,
) -> Option<MenuHit> {
    let layout = project_menu_layout(view, size);
    let over = |rect: Rect| rect.to_f64().contains(pos);
    if !over(layout.panel) {
        return None;
    }
    if let Some(i) = hint_at(&hint_strs(&view.hints), &layout.hints, pos) {
        return Some(MenuHit::Hint(i));
    }
    if let Some(form) = &layout.form {
        let fields = [
            (form.name, FormField::Name),
            (form.folder, FormField::Folder),
            (form.browse, FormField::Browse),
            (form.cancel, FormField::Cancel),
            (form.create, FormField::Create),
        ];
        let hit = fields
            .iter()
            .find(|(rect, _)| over(*rect))
            .map(|(_, field)| MenuHit::Field(*field))
            .or_else(|| {
                let i = form.suggestions.iter().position(|rect| over(*rect))?;
                Some(MenuHit::Suggestion(i))
            })
            .or_else(|| {
                let (kind, _) = form.kinds.iter().find(|(_, rect)| over(*rect))?;
                Some(MenuHit::Kind(*kind))
            });
        return hit.or(Some(MenuHit::Panel));
    }
    if let Some(settings) = &layout.settings {
        let fields = [
            (settings.before, SettingsField::Before),
            (settings.after, SettingsField::After),
            (settings.cancel, SettingsField::Cancel),
            (settings.run, SettingsField::Run),
            (settings.save, SettingsField::Save),
        ];
        let hit = fields
            .iter()
            .find(|(rect, _)| over(*rect))
            .map(|(_, field)| MenuHit::Setting(*field));
        return hit.or(Some(MenuHit::Panel));
    }
    if over(layout.query) {
        return Some(MenuHit::Query);
    }
    if !over(layout.list) {
        return Some(MenuHit::Panel);
    }
    for (i, (rect, buttons)) in layout.entries.iter().enumerate() {
        if let Some((button, _)) = buttons.iter().find(|(_, rect)| over(*rect)) {
            return Some(MenuHit::Button(i, *button));
        }
        if over(*rect) {
            return Some(MenuHit::Entry(i));
        }
    }
    Some(MenuHit::Panel)
}

/// Where the text of the project menu's query field in `rect` goes, after
/// its magnifier.
pub(super) fn menu_query_text(rect: Rect, scale: f64) -> (f32, f32) {
    let px = |v: i32| (v as f64 * scale).round() as f32;
    let mark_x = rect.loc.x + PROJECT_MENU_PADDING + PADDING as i32;
    (
        px(mark_x + MENU_MARK_WIDTH + MENU_MARK_GAP),
        px(rect.loc.x + rect.size.w - HELP_PADDING) - px(CARET_WIDTH + 1),
    )
}

/// The field a project menu entry in `rect` turns into while its project is
/// renamed: over the name, up to the row's `buttons` if it shows them.
pub(super) fn rename_field(rect: Rect, buttons: &[(RowButton, Rect)]) -> Rect {
    let x = rect.loc.x + PADDING as i32 + MENU_MARK_WIDTH + MENU_MARK_GAP - RENAME_INSET;
    let end = buttons
        .first()
        .map_or(rect.loc.x + rect.size.w - PADDING as i32, |(_, b)| {
            b.loc.x - FIELD_GAP
        });
    Rectangle::new(
        (x, rect.loc.y + (rect.size.h - RENAME_FIELD) / 2).into(),
        ((end - x).max(0), RENAME_FIELD).into(),
    )
}

/// The project menu's text field with the keyboard, on an output of `size`.
pub fn project_menu_field(
    view: &ProjectMenuView,
    size: Size<i32, Logical>,
    scale: f64,
) -> Option<FieldSpot> {
    let layout = project_menu_layout(view, size);
    let boxed = |area: Rect, line: &Line| FieldSpot {
        size: FONT_SIZE,
        area,
        text: field_text(area, scale),
        line: line.clone(),
    };
    if let (Some(form), Some(rects)) = (&view.form, &layout.form) {
        return match form.focus {
            FormField::Name => Some(boxed(rects.name, &form.name)),
            FormField::Folder => Some(boxed(rects.folder, &form.folder)),
            _ => None,
        };
    }
    if let (Some(settings), Some(rects)) = (&view.settings, &layout.settings) {
        return match settings.focus {
            SettingsField::Before => Some(boxed(rects.before, &settings.before)),
            SettingsField::After => Some(boxed(rects.after, &settings.after)),
            _ => None,
        };
    }
    let renamed = view
        .entries
        .iter()
        .zip(&layout.entries)
        .find_map(|(entry, (rect, buttons))| match entry {
            MenuEntry::Project {
                renaming: Some(name),
                ..
            } => Some(boxed(rename_field(*rect, buttons), name)),
            _ => None,
        });
    renamed.or_else(|| {
        Some(FieldSpot {
            size: FONT_SIZE,
            area: layout.query,
            text: menu_query_text(layout.query, scale),
            line: view.query.clone(),
        })
    })
}

/// Whether an entry of `view` is being renamed.
fn renaming(view: &ProjectMenuView) -> bool {
    view.entries.iter().any(|e| {
        matches!(
            e,
            MenuEntry::Project {
                renaming: Some(_),
                ..
            }
        )
    })
}

/// Whether entry `i` of `view` starts the rows starting new projects, right
/// after a project: a line goes over it.
fn under_projects(view: &ProjectMenuView, i: usize) -> bool {
    let after_project = i
        .checked_sub(1)
        .is_some_and(|j| matches!(view.entries[j], MenuEntry::Project { .. }));
    matches!(view.entries[i], MenuEntry::New(_)) && after_project
}

/// A line across a panel `w` wide whose left edge is `x` physical pixels
/// in, at `y`, inside the panel's border.
fn menu_divider(c: &mut Canvas, x: i32, y: i32, w: i32, scale: f64, p: &Palette) {
    let px = |v: i32| (v as f64 * scale).round() as i32;
    let border = px(1).max(1);
    c.fill(x + border, y, px(w) - 2 * border, border, p.border_variant);
}

impl Painter {
    /// The project menu for an output of `size`, inside a margin of `SHADOW`
    /// holding its drop shadow, in one buffer: the query field and the
    /// entries, or the form under its title; then the key hints. The chrome
    /// shows it in parts (see `ProjectMenuLayers`).
    #[cfg(test)]
    pub(super) fn project_menu(
        &self,
        view: &ProjectMenuView,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let layout = project_menu_layout(view, size);
        let empty = view.entries.is_empty();
        let mut c = self.project_menu_frame(view, &layout, empty, renaming(view), scale, p);
        if layout.form.is_some() || layout.settings.is_some() {
            return c.into_buffer(false);
        }
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let panel = layout.panel;
        // Rects relative to the buffer, which starts `SHADOW` before the panel.
        let by = Point::from((SHADOW - panel.loc.x, SHADOW - panel.loc.y));
        let shift = |r: Rect| Rectangle::new(r.loc + by, r.size);
        let (x, y, w, h) = rect_px(shift(layout.list), scale);
        c.clipped(x as i32, y as i32, w as i32, h as i32, |c| {
            for (i, ((rect, buttons), entry)) in
                layout.entries.iter().zip(&view.entries).enumerate()
            {
                if rect.size.h == 0 {
                    continue;
                }
                let rect = shift(*rect);
                let buttons: Vec<_> = buttons.iter().map(|(b, r)| (*b, shift(*r))).collect();
                if under_projects(view, i) {
                    menu_divider(c, px(SHADOW), px(rect.loc.y), panel.size.w, scale, p);
                }
                let look = EntryLook::of(view, i);
                self.menu_entry(c, entry, look, rect, &buttons, scale, p);
            }
        });
        c.into_buffer(false)
    }

    /// The project menu laid out as `layout` but its entries, inside a
    /// margin of `SHADOW` holding its drop shadow: the query field of
    /// `view` (focused unless an entry is `renaming`), "Nothing matches"
    /// when `empty`, or the form under its title; then the key hints.
    fn project_menu_frame(
        &self,
        view: &ProjectMenuView,
        layout: &ProjectMenuLayout,
        empty: bool,
        renaming: bool,
        scale: f64,
        p: &Palette,
    ) -> Canvas {
        let panel = layout.panel;
        let mut c = self.popup(panel.size, HELP_RADIUS, scale, p);
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let border = px(1).max(1);
        // Rects relative to the buffer, which starts `SHADOW` before the panel.
        let by = Point::from((SHADOW - panel.loc.x, SHADOW - panel.loc.y));
        let shift = |r: Rect| Rectangle::new(r.loc + by, r.size);
        // A line across the panel, at `y` physical pixels.
        let divider = |c: &mut Canvas, y: i32| {
            menu_divider(c, px(SHADOW), y, panel.size.w, scale, p);
        };

        let query = shift(layout.query);
        match (&layout.form, &layout.settings) {
            (None, Some(settings)) => {
                if let Some(settings_view) = &view.settings {
                    let (x, right) = phys(query, scale);
                    self.text(
                        &mut c,
                        &format!("{} · start and stop", settings_view.name),
                        LABEL_FONT_SIZE,
                        x + (FORM_PADDING as f32 * s).round(),
                        right,
                        band(query, scale),
                        scale,
                        p.text,
                    );
                    self.menu_settings(&mut c, settings_view, view.hover, settings, by, scale, p);
                }
            }
            (Some(form), _) => {
                let (x, right) = phys(query, scale);
                self.text(
                    &mut c,
                    "New project",
                    LABEL_FONT_SIZE,
                    x + (FORM_PADDING as f32 * s).round(),
                    right,
                    band(query, scale),
                    scale,
                    p.text,
                );
                if let Some(form_view) = &view.form {
                    self.menu_form(&mut c, form_view, view.hover, form, by, scale, p);
                }
            }
            (None, None) => {
                self.menu_query(&mut c, &view.query, !renaming, query, scale, p);
                if empty {
                    let first = Rectangle::new(
                        (
                            query.loc.x,
                            query.loc.y + query.size.h + PROJECT_MENU_PADDING,
                        )
                            .into(),
                        (query.size.w, PROJECT_MENU_ROW).into(),
                    );
                    let (x, right) = phys(first, scale);
                    self.text(
                        &mut c,
                        "Nothing matches",
                        FONT_SIZE,
                        x + px(PROJECT_MENU_PADDING) as f32 + PADDING * s,
                        right,
                        band(first, scale),
                        scale,
                        p.placeholder,
                    );
                }
            }
        }
        divider(&mut c, px(query.loc.y + query.size.h) - border);

        let footer = shift(layout.footer);
        divider(&mut c, px(footer.loc.y));
        let spots: Vec<_> = layout.hints.iter().map(|(i, r)| (*i, shift(*r))).collect();
        let hover = match view.hover {
            Some(MenuHit::Hint(i)) => Some(i),
            _ => None,
        };
        self.hints(&mut c, &hint_strs(&view.hints), &spots, hover, scale, p);
        c
    }

    /// `entry` drawn alone, `size` big on the panel's colour, looking as
    /// `look` says.
    fn menu_entry_buffer(
        &self,
        entry: &MenuEntry,
        size: Size<i32, Logical>,
        look: EntryLook,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let mut c = Canvas::new(super::physical(size, scale));
        let (w, h) = (c.w, c.h);
        c.fill(0, 0, w, h, p.elevated);
        let rect = Rectangle::from_size(size);
        let buttons = match entry {
            MenuEntry::Project { row, .. } if look.hovered => {
                row_button_rects(&row_buttons(row), rect)
            }
            _ => Vec::new(),
        };
        self.menu_entry(&mut c, entry, look, rect, &buttons, scale, p);
        c.into_buffer(true)
    }

    /// The project menu's query field in `rect`: a magnifier, then `query`
    /// (or what to type) with a caret after it when `focused`.
    pub(super) fn menu_query(
        &self,
        c: &mut Canvas,
        query: &Line,
        focused: bool,
        rect: Rect,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        // Lined up with the marks and labels of the rows below.
        let mark_x = rect.loc.x + PROJECT_MENU_PADDING + PADDING as i32;
        let mark = Rectangle::new(
            (mark_x, rect.loc.y).into(),
            (MENU_MARK_WIDTH, rect.size.h).into(),
        );
        icon_sdf(c, mark, scale, p.placeholder, |x, y| magnifier(x, y, s));
        let query_band = band(rect, scale);
        let placeholder = "Find or name a project…";
        self.edit_line(
            c,
            query,
            placeholder,
            menu_query_text(rect, scale),
            query_band,
            focused,
            FONT_SIZE,
            scale,
            p,
        );
    }

    /// `entry` of the project menu in `rect` (buffer coordinates), looking
    /// as `look` says, with the row `buttons` it shows under the pointer.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn menu_entry(
        &self,
        c: &mut Canvas,
        entry: &MenuEntry,
        look: EntryLook,
        rect: Rect,
        buttons: &[(RowButton, Rect)],
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let row_band = band(rect, scale);
        let mark = menu_mark(rect);
        let label_lx = mark.loc.x + MENU_MARK_WIDTH + MENU_MARK_GAP;
        let label_x = px(label_lx) as f32;
        let (x, y, w, h) = rect_px(rect, scale);
        let right = x + w - PADDING * s;
        let gap = (FIELD_GAP as f32 * s).round();
        if let MenuEntry::Section(label) = entry {
            self.text(
                c,
                &label.to_uppercase(),
                DETAIL_FONT_SIZE,
                px(mark.loc.x) as f32,
                right,
                row_band,
                scale,
                p.placeholder,
            );
            return;
        }

        // The highlight stops short of the rows above and below.
        let (y, h) = (y + px(2) as f32, h - 2.0 * px(2) as f32);
        let r = (MENU_RADIUS - 2.0) * s;
        let confirm = matches!(
            entry,
            MenuEntry::Project {
                confirm_forget: true,
                ..
            }
        );
        if confirm {
            c.rounded(x, y, w, h, r, mix(p.elevated, p.warning, 0.15));
        } else if look.selected {
            c.rounded(x, y, w, h, r, p.selected);
        } else if look.hovered {
            let t = 0.5 * look.hover_alpha as f32 / 16.0;
            c.rounded(x, y, w, h, r, mix(p.elevated, p.selected, t));
        }

        // A keycap at the right end of the row; returns where it starts.
        let keycap = |c: &mut Canvas, keys: &str| {
            let cap_x = (right - self.keycaps_width(keys, scale)).round();
            self.keycaps(c, keys, cap_x, row_band, scale, p);
            cap_x
        };
        match entry {
            MenuEntry::Section(_) => {}
            MenuEntry::Project {
                row,
                digit,
                renaming,
                hits,
                ..
            } => {
                let dot = move |x: f32, y: f32| x.hypot(y) - 4.2 * s;
                let ring = move |x: f32, y: f32| (x.hypot(y) - 3.6 * s).abs() - 1.1 * s;
                match (confirm, row.running) {
                    (true, _) => icon_sdf(c, mark, scale, p.warning, dot),
                    (false, Some(true)) => icon_sdf(c, mark, scale, p.success, dot),
                    (false, Some(false)) => icon_sdf(c, mark, scale, p.placeholder, ring),
                    (false, None) => {}
                }
                let mut end = right;
                if let Some((_, first)) = buttons.first() {
                    self.row_buttons(c, look, buttons, confirm, scale, p);
                    end = px(first.loc.x) as f32 - gap;
                } else if !confirm && renaming.is_none() {
                    if let Some(digit) = digit {
                        end = keycap(c, &format!("Alt+{digit}")) - gap;
                    }
                    let ago_x = (end - self.text_width(&row.ago, SMALL_FONT_SIZE, scale)).round();
                    self.text(
                        c,
                        &row.ago,
                        SMALL_FONT_SIZE,
                        ago_x,
                        f32::MAX,
                        row_band,
                        scale,
                        p.placeholder,
                    );
                    end = ago_x - gap;
                }
                if confirm {
                    self.text(
                        c,
                        &format!("Press Delete again to forget {}", row.name),
                        FONT_SIZE,
                        label_x,
                        end,
                        row_band,
                        scale,
                        p.warning,
                    );
                } else if let Some(name) = renaming {
                    let field = rename_field(rect, buttons);
                    self.field(c, name, "Project name", field, true, scale, p);
                } else {
                    // The hits are over "name detail": split between the two.
                    let name_len = row.name.chars().count();
                    let name_hits: Vec<usize> =
                        hits.iter().copied().filter(|&h| h < name_len).collect();
                    let detail_hits: Vec<usize> = hits
                        .iter()
                        .filter_map(|&h| h.checked_sub(name_len + 1))
                        .collect();
                    let after = self.marked(
                        c,
                        &row.name,
                        &name_hits,
                        FONT_SIZE,
                        label_x,
                        end,
                        row_band,
                        scale,
                        (p.text, p.accent),
                    );
                    self.marked(
                        c,
                        &row.detail,
                        &detail_hits,
                        SMALL_FONT_SIZE,
                        after + (12.0 * s).round(),
                        end,
                        row_band,
                        scale,
                        (p.muted, p.accent),
                    );
                }
            }
            MenuEntry::New(label) => {
                icon_sdf(c, mark, scale, p.muted, |x, y| {
                    plus(x, y, 0.0, 0.0, 5.0 * s, s)
                });
                let end = keycap(c, "Ctrl+N") - gap;
                self.text(c, label, FONT_SIZE, label_x, end, row_band, scale, p.text);
            }
            MenuEntry::Folder(path) => {
                let small = 0.8 * s;
                icon_sdf(c, mark, scale, p.placeholder, |x, y| folder(x, y, small));
                let suffix = "new project";
                let suffix_w = self.text_width(suffix, SMALL_FONT_SIZE, scale);
                let after = self.text(
                    c,
                    path,
                    FONT_SIZE,
                    label_x,
                    right - suffix_w - gap,
                    row_band,
                    scale,
                    p.text,
                );
                self.text(
                    c,
                    suffix,
                    SMALL_FONT_SIZE,
                    after + gap,
                    right,
                    row_band,
                    scale,
                    p.placeholder,
                );
            }
        }
    }

    /// The `buttons` of an entry looking as `look` says: icons, the one
    /// under the pointer on a rounded square; forget in the warning colour
    /// while it asks to be confirmed.
    pub(super) fn row_buttons(
        &self,
        c: &mut Canvas,
        look: EntryLook,
        buttons: &[(RowButton, Rect)],
        confirm: bool,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        for &(button, rect) in buttons {
            let hot = look.button == Some(button);
            if hot {
                let (x, y, w, h) = rect_px(rect, scale);
                c.rounded(x, y, w, h, CAP_RADIUS * s, mix(p.selected, p.text, 0.12));
            }
            let color = match button {
                RowButton::Forget if confirm => p.warning,
                _ if hot => p.text,
                _ => p.muted,
            };
            icon_sdf(c, rect, scale, color, |x, y| match button {
                RowButton::Settings => commands_prompt(x, y, s),
                RowButton::ProjectFile => file(x, y, s),
                RowButton::Rename => pencil(x, y, s),
                RowButton::Stop => stop(x, y, s),
                RowButton::Forget => cross(x, y, s),
            });
        }
    }

    /// The project menu's settings form, its parts at `layout` moved `by`
    /// into the buffer: the labelled start and stop command fields, where
    /// they run, and the buttons.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn menu_settings(
        &self,
        c: &mut Canvas,
        form: &SettingsView,
        hover: Option<MenuHit>,
        layout: &SettingsLayout,
        by: Point<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let shift = |r: Rect| Rectangle::new(r.loc + by, r.size);
        let focused = |field: SettingsField| form.focus == field;
        let hovered = |hit: MenuHit| hover == Some(hit);
        for (label, text, placeholder, rect, field) in [
            (
                "Start",
                &form.before,
                "e.g. docker compose up -d",
                layout.before,
                SettingsField::Before,
            ),
            (
                "Stop",
                &form.after,
                "e.g. docker compose down",
                layout.after,
                SettingsField::After,
            ),
        ] {
            let rect = shift(rect);
            let x = px(rect.loc.x - FORM_LABEL_WIDTH) as f32;
            let end = px(rect.loc.x - FORM_GAP) as f32;
            let label_band = band(rect, scale);
            self.text(c, label, FONT_SIZE, x, end, label_band, scale, p.muted);
            if focused(field) {
                focus_ring(c, rect, BUTTON_RADIUS, p.elevated, scale, p.accent);
            }
            self.field(c, text, placeholder, rect, focused(field), scale, p);
        }

        let note = shift(layout.note);
        let (x, right) = phys(note, scale);
        self.text(
            c,
            &format!("Run with sh -c in {}", form.dir),
            SMALL_FONT_SIZE,
            x,
            right,
            band(note, scale),
            scale,
            p.muted,
        );

        let [(cancel, _), (run, _), (save, _)] = SETTINGS_BUTTONS;
        for (label, rect, field, primary) in [
            (cancel, layout.cancel, SettingsField::Cancel, false),
            (run, layout.run, SettingsField::Run, false),
            (save, layout.save, SettingsField::Save, true),
        ] {
            let state = (hovered(MenuHit::Setting(field)), focused(field));
            self.button(c, label, shift(rect), state, primary, scale, p);
        }
    }

    /// The project menu's form, its parts at `layout` moved `by` into the
    /// buffer: the labelled name and folder fields, Browse, the folders
    /// offered, the "Opens" choices, the error line and the buttons.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn menu_form(
        &self,
        c: &mut Canvas,
        form: &FormView,
        hover: Option<MenuHit>,
        layout: &FormLayout,
        by: Point<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let shift = |r: Rect| Rectangle::new(r.loc + by, r.size);
        let (name, folder_rect) = (shift(layout.name), shift(layout.folder));
        let kinds = layout.kinds.map(|(kind, rect)| (kind, shift(rect)));
        // A label in the column left of the row holding `rect`.
        let label = |c: &mut Canvas, text: &str, rect: Rect| {
            let x = px(rect.loc.x - FORM_LABEL_WIDTH) as f32;
            let end = px(rect.loc.x - FORM_GAP) as f32;
            self.text(
                c,
                text,
                FONT_SIZE,
                x,
                end,
                band(rect, scale),
                scale,
                p.muted,
            );
        };
        let focused = |field: FormField| form.focus == field;
        let hovered = |hit: MenuHit| hover == Some(hit);

        label(c, "Name", name);
        if focused(FormField::Name) {
            focus_ring(c, name, BUTTON_RADIUS, p.elevated, scale, p.accent);
        }
        let name_focus = focused(FormField::Name);
        self.field(c, &form.name, "Project name", name, name_focus, scale, p);

        label(c, "Folder", folder_rect);
        let folder_focus = focused(FormField::Folder);
        if folder_focus {
            focus_ring(c, folder_rect, BUTTON_RADIUS, p.elevated, scale, p.accent);
        }
        let placeholder = "None — add later";
        self.field(
            c,
            &form.folder,
            placeholder,
            folder_rect,
            folder_focus,
            scale,
            p,
        );
        self.button(
            c,
            "Browse…",
            shift(layout.browse),
            (
                hovered(MenuHit::Field(FormField::Browse)),
                focused(FormField::Browse),
            ),
            false,
            scale,
            p,
        );

        // The folders offered, in a box hanging under the folder field.
        if let (Some(first), Some(last)) = (layout.suggestions.first(), layout.suggestions.last()) {
            let (first, last) = (shift(*first), shift(*last));
            let (x, y, w, _) = rect_px(first, scale);
            let h = px(last.loc.y + last.size.h) as f32 - y;
            let (b, r) = (px(1).max(1) as f32, BUTTON_RADIUS * s);
            c.rounded(x, y, w, h, r, p.border);
            c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, p.element);
            for (i, (rect, path)) in layout.suggestions.iter().zip(&form.suggestions).enumerate() {
                let rect = shift(*rect);
                let (x, y, w, h) = rect_px(rect, scale);
                let (x, y, w, h) = (x + 2.0 * b, y + 2.0 * b, w - 4.0 * b, h - 4.0 * b);
                if form.suggestion == Some(i) {
                    c.rounded(x, y, w, h, r - 2.0 * b, p.selected);
                } else if hovered(MenuHit::Suggestion(i)) {
                    c.rounded(x, y, w, h, r - 2.0 * b, mix(p.element, p.selected, 0.5));
                }
                let mark = Rectangle::new(
                    (rect.loc.x + FORM_GAP, rect.loc.y).into(),
                    (MENU_MARK_WIDTH, rect.size.h).into(),
                );
                let small = 0.7 * s;
                icon_sdf(c, mark, scale, p.placeholder, |x, y| folder(x, y, small));
                let text_x = px(mark.loc.x + MENU_MARK_WIDTH + FORM_GAP) as f32;
                let end = phys(rect, scale).1 - PADDING * s;
                self.text(
                    c,
                    path,
                    SMALL_FONT_SIZE,
                    text_x,
                    end,
                    band(rect, scale),
                    scale,
                    p.text,
                );
            }
        }

        // What the folder opens as: a pair of round choices, dimmed without
        // a folder.
        label(c, "Opens", kinds[0].1);
        let enabled = !form.folder.trim().is_empty();
        for ((kind, rect), (_, text, _)) in kinds.iter().zip(KIND_CHOICES) {
            let (x, y, w, h) = rect_px(*rect, scale);
            let chosen = form.kind == *kind;
            if focused(FormField::Kind) && chosen {
                focus_ring(c, *rect, BUTTON_RADIUS, p.elevated, scale, p.accent);
            }
            if enabled && hovered(MenuHit::Kind(*kind)) {
                c.rounded(
                    x,
                    y,
                    w,
                    h,
                    BUTTON_RADIUS * s,
                    mix(p.elevated, p.selected, 0.5),
                );
            }
            let side = px(RADIO_SIZE) as f32;
            let (bx, by) = (x + px(6) as f32, (y + (h - side) / 2.0).round());
            let b = px(1).max(1) as f32;
            let accent = if enabled {
                p.accent
            } else {
                mix(p.accent, p.elevated, 0.5)
            };
            if chosen {
                c.rounded(bx, by, side, side, side / 2.0, accent);
                let dot = (6.0 * s).round();
                let (dx, dy) = (bx + (side - dot) / 2.0, by + (side - dot) / 2.0);
                c.rounded(dx, dy, dot, dot, dot / 2.0, p.on_accent);
            } else {
                let edge = if enabled { p.border } else { p.border_variant };
                c.rounded(bx, by, side, side, side / 2.0, edge);
                let inner = side - 2.0 * b;
                c.rounded(bx + b, by + b, inner, inner, inner / 2.0, p.element);
            }
            self.text(
                c,
                text,
                FONT_SIZE,
                bx + side + (FORM_GAP as f32 * s).round(),
                x + w,
                band(*rect, scale),
                scale,
                if enabled { p.text } else { p.placeholder },
            );
        }

        if let (Some((_, error)), true) = (&form.error, layout.error.size.h > 0) {
            let error_rect = shift(layout.error);
            let (x, right) = phys(error_rect, scale);
            self.text(
                c,
                error,
                SMALL_FONT_SIZE,
                x,
                right,
                band(error_rect, scale),
                scale,
                p.warning,
            );
        }

        let [(cancel_label, _), (create_label, _)] = FORM_BUTTONS;
        for (label, rect, field, primary) in [
            (cancel_label, layout.cancel, FormField::Cancel, false),
            (create_label, layout.create, FormField::Create, true),
        ] {
            let state = (hovered(MenuHit::Field(field)), focused(field));
            self.button(c, label, shift(rect), state, primary, scale, p);
        }
    }
}

/// What the project menu's frame is drawn from: its view without the
/// entries and what only they show, its layout without them, whether there
/// are none, whether one is renamed, the theme and the scale.
type FrameKey = (
    ProjectMenuView,
    ProjectMenuLayout,
    bool,
    bool,
    super::Theme,
    f64,
);

/// What an entry's buffer is drawn from: the entry, its size, how it
/// looks, the theme and the scale.
type EntryKey = (MenuEntry, Size<i32, Logical>, EntryLook, super::Theme, f64);

/// The project menu as the chrome shows it, fading in and out (see
/// `Overlay`): a frame buffer with what stays put, and a buffer per entry,
/// which scrolling only moves.
#[derive(Default)]
pub(super) struct ProjectMenuLayers {
    overlay: super::Overlay<FrameKey>,
    entries: RowCache<EntryKey>,
    /// The line between the projects and the rows starting new ones, by
    /// the panel's width, the theme and the scale.
    divider: super::Cached<(i32, super::Theme, f64)>,
    /// The scrollbar's thumb, by its size.
    thumb: super::Cached<(Size<i32, Logical>, super::Theme, f64)>,
    /// How many frames were drawn.
    #[cfg(test)]
    frames: std::cell::Cell<usize>,
}

impl ProjectMenuLayers {
    /// The parts showing the project menu at `now`, front to back.
    pub(super) fn parts(
        &mut self,
        view: Option<ProjectMenuView>,
        output: Size<i32, Logical>,
        theme: super::Theme,
        scale: f64,
        now: std::time::Instant,
        painter: &Painter,
    ) -> Vec<Part> {
        let shown = view.map(|view| {
            let layout = project_menu_layout(&view, output);
            (view, layout)
        });
        let placed = shown.as_ref().map(|(view, layout)| {
            let listed = layout.form.is_none() && layout.settings.is_none();
            let still = ProjectMenuView {
                entries: Vec::new(),
                selected: None,
                // In the list, only the key hints light up in the frame.
                hover: view
                    .hover
                    .filter(|hit| !listed || matches!(hit, MenuHit::Hint(_))),
                hover_alpha: 0,
                part: 0.0,
                bar: None,
                ..view.clone()
            };
            let frame = ProjectMenuLayout {
                entries: Vec::new(),
                ..layout.clone()
            };
            let (loc, size) = super::shadowed(layout.panel);
            let key = (
                still,
                frame,
                view.entries.is_empty(),
                renaming(view),
                theme,
                scale,
            );
            (key, loc, size)
        });
        let parts = self.overlay.parts(
            placed,
            now,
            true,
            |(view, layout, empty, renaming, ..)| {
                #[cfg(test)]
                self.frames.set(self.frames.get() + 1);
                painter
                    .project_menu_frame(view, layout, *empty, *renaming, scale, theme.palette())
                    .into_buffer(false)
            },
            |_| {
                let Some((view, layout)) = &shown else {
                    return Vec::new();
                };
                let p = theme.palette();
                let (list, panel) = (layout.list, layout.panel);
                let top = list.loc.y as f64;
                let mut parts = Vec::new();
                if let Some(bar) = project_menu_thumb(view, layout) {
                    let size = Size::from((bar.track.size.w, bar.h as i32));
                    let buffer = self
                        .thumb
                        .get((size, theme, scale), |_| painter.bar_thumb(size, scale, p));
                    let part = Part::new(buffer, bar.track.loc, size, 1.0);
                    parts.push(part.at((bar.track.loc.x as f64, bar.top).into()));
                }
                let tops = entry_tops(view);
                for (i, ((rect, _), entry)) in layout.entries.iter().zip(&view.entries).enumerate()
                {
                    if rect.size.h == 0 {
                        continue;
                    }
                    let y = top + tops[i];
                    if under_projects(view, i) {
                        let size = Size::from((panel.size.w, 1));
                        let buffer = self.divider.get((panel.size.w, theme, scale), |_| {
                            let mut c = Canvas::new(super::physical(size, scale));
                            menu_divider(&mut c, 0, 0, panel.size.w, scale, p);
                            c.into_buffer(false)
                        });
                        let part = Part::new(buffer, panel.loc, size, 1.0);
                        parts.push(part.at((panel.loc.x as f64, y).into()).clipped(list));
                    }
                    let look = EntryLook::of(view, i);
                    let key = (entry.clone(), rect.size, look, theme, scale);
                    let buffer = self
                        .entries
                        .get(key, rect.size, scale, |(entry, size, ..)| {
                            painter.menu_entry_buffer(entry, *size, look, scale, p)
                        });
                    let part = Part::new(&buffer, rect.loc, rect.size, 1.0);
                    parts.push(part.at((rect.loc.x as f64, y).into()).clipped(list));
                }
                parts
            },
        );
        self.entries.sweep();
        if !self.overlay.lingers() {
            self.entries.clear();
        }
        parts
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FormField, FormView, MenuEntry, MenuHit, PROJECT_MENU_SECTION, ProjectMenuLayers,
        ProjectMenuView, RowButton, SettingsField, SettingsView, project_menu_field,
        project_menu_hit, project_menu_layout, project_menu_page, project_menu_room,
    };
    use crate::WorkspaceKind;
    use crate::chrome::{
        BAR_HEIGHT, HELP_MARGIN, PROJECT_MENU_WIDTH, Painter, ProjectRow, ROW_INSET, Rect,
        SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH, SIDEBAR_WIDTH, SidebarState, SidebarView, Theme,
        ToastView, inside,
    };
    use crate::text_field::Line;
    use crate::toast::Level;
    use smithay::utils::{Logical, Point, Size};

    fn menu_row(name: &str, running: Option<bool>, current: bool) -> ProjectRow {
        ProjectRow {
            name: name.into(),
            detail: "3 repos · ~/Work/aft".into(),
            ago: "2h ago".into(),
            running,
            current,
        }
    }

    fn project(row: ProjectRow, digit: Option<u8>) -> MenuEntry {
        MenuEntry::Project {
            row,
            digit,
            renaming: None,
            confirm_forget: false,
            hits: vec![0, 1, 2],
        }
    }

    /// The list: the current project, three recent ones (running, stopped,
    /// not known yet), then the rows starting new ones.
    fn list_view() -> ProjectMenuView {
        ProjectMenuView {
            query: "blo".into(),
            entries: vec![
                MenuEntry::Section("Current".into()),
                project(menu_row("aft", Some(true), true), None),
                MenuEntry::Section("Recent".into()),
                project(menu_row("gerdr", Some(true), false), Some(1)),
                project(menu_row("blog", Some(false), false), Some(2)),
                project(menu_row("notes", None, false), Some(3)),
                MenuEntry::New("New project \"blo\"…".into()),
                MenuEntry::Folder("~/Sites/blogging".into()),
            ],
            selected: Some(3),
            hover: None,
            form: None,
            settings: None,
            hints: vec![
                ("Enter".into(), "open".into()),
                ("Ctrl+N".into(), "new".into()),
                ("F2".into(), "rename".into()),
                ("Ctrl+E".into(), "commands".into()),
                ("Ctrl+Q".into(), "stop".into()),
                ("Del".into(), "forget".into()),
                ("Esc".into(), "close".into()),
            ],
            hover_alpha: 16,
            part: 0.0,
            bar: None,
        }
    }

    fn form_view(suggestions: usize, error: bool) -> ProjectMenuView {
        ProjectMenuView {
            query: Line::default(),
            entries: Vec::new(),
            selected: None,
            hover: None,
            form: Some(FormView {
                name: "blog-v2".into(),
                folder: "~/Sites/blog".into(),
                kind: WorkspaceKind::Project,
                focus: FormField::Folder,
                suggestions: (0..suggestions)
                    .map(|i| format!("~/Sites/blog{i}"))
                    .collect(),
                suggestion: (suggestions > 0).then_some(0),
                error: error.then(|| (FormField::Name, "A project named blog-v2 exists.".into())),
            }),
            settings: None,
            hints: vec![
                ("Tab".into(), "next".into()),
                ("Enter".into(), "create".into()),
                ("Esc".into(), "back".into()),
            ],
            hover_alpha: 0,
            part: 0.0,
            bar: None,
        }
    }

    fn settings_view(before: &str) -> ProjectMenuView {
        ProjectMenuView {
            query: Line::default(),
            entries: Vec::new(),
            selected: None,
            hover: None,
            form: None,
            settings: Some(SettingsView {
                name: "blog".into(),
                before: before.into(),
                after: Line::default(),
                dir: "~/Sites/blog".into(),
                focus: SettingsField::Before,
            }),
            hints: vec![("Esc".into(), "back".into())],
            hover_alpha: 0,
            part: 0.0,
            bar: None,
        }
    }

    fn below(upper: Rect, lower: Rect) -> bool {
        upper.loc.y + upper.size.h <= lower.loc.y
    }

    fn centre(rect: Rect) -> Point<f64, Logical> {
        (
            rect.loc.x as f64 + rect.size.w as f64 / 2.0,
            rect.loc.y as f64 + rect.size.h as f64 / 2.0,
        )
            .into()
    }

    #[test]
    fn project_menu_layout_fits_the_output() {
        for (w, h) in [(800, 600), (1920, 1080)] {
            let size: Size<i32, Logical> = (w, h).into();
            let margin = Rect::new((0, 0).into(), (w, h - HELP_MARGIN).into());
            // A full page of rows, with a section label at the top.
            let mut full = list_view();
            full.entries.truncate(1);
            while full.entries.len() < project_menu_page(size) {
                full.entries.push(project(menu_row("x", None, false), None));
            }
            full.hover = Some(MenuHit::Entry(2));
            let mut views = vec![list_view(), full];
            for (suggestions, error) in [(0, false), (5, false), (0, true), (5, true)] {
                views.push(form_view(suggestions, error));
            }
            for view in &views {
                let l = project_menu_layout(view, size);
                assert!(inside(margin, l.panel), "{:?} outside {w}x{h}", l.panel);
                assert!(l.panel.size.w <= PROJECT_MENU_WIDTH);
                assert_eq!(l.panel.loc, (ROW_INSET, BAR_HEIGHT + 2).into());
                assert!(inside(l.panel, l.query) && inside(l.panel, l.footer));
                assert!(below(l.query, l.footer));
                assert_eq!(
                    l.footer.loc.y + l.footer.size.h,
                    l.panel.loc.y + l.panel.size.h
                );
                for (rect, buttons) in &l.entries {
                    assert!(rect.size.h > 0, "{rect:?} cut off at {w}x{h}");
                    assert!(
                        inside(l.panel, *rect) && below(l.query, *rect) && below(*rect, l.footer)
                    );
                    assert!(buttons.iter().all(|(_, b)| inside(*rect, *b)));
                }
                assert!(l.entries.windows(2).all(|e| below(e[0].0, e[1].0)));
                match (&l.form, &view.form) {
                    (None, None) => assert_eq!(l.entries.len(), view.entries.len()),
                    (Some(f), Some(form)) => {
                        assert!(l.entries.is_empty());
                        assert_eq!(f.suggestions.len(), form.suggestions.len());
                        let mut parts =
                            vec![f.name, f.folder, f.browse, f.error, f.cancel, f.create];
                        parts.extend(f.kinds.iter().map(|(_, r)| *r));
                        parts.extend(&f.suggestions);
                        for part in parts {
                            assert!(inside(l.panel, part), "{part:?} outside the panel");
                        }
                        assert!(below(l.query, f.name) && below(f.name, f.folder));
                        assert_eq!(f.folder.loc.y, f.browse.loc.y);
                        assert!(f.folder.loc.x + f.folder.size.w < f.browse.loc.x);
                        let last = f.suggestions.last().copied().unwrap_or(f.folder);
                        assert!(below(f.folder, last) || last == f.folder);
                        assert!(f.suggestions.windows(2).all(|s| below(s[0], s[1])));
                        let [(_, zed), (_, term)] = f.kinds;
                        assert!(below(last, zed) && zed.loc.x + zed.size.w < term.loc.x);
                        assert!(below(zed, f.create));
                        // The error hangs under the field at fault, the name here.
                        match &form.error {
                            Some(_) => {
                                assert_eq!(f.error.size.h, super::ERROR_HEIGHT);
                                assert!(below(f.name, f.error) && below(f.error, f.folder));
                            }
                            None => assert_eq!(f.error.size.h, 0),
                        }
                        assert_eq!(f.cancel.loc.y, f.create.loc.y);
                        assert!(f.cancel.loc.x + f.cancel.size.w < f.create.loc.x);
                        assert!(below(f.create, l.footer));
                    }
                    _ => panic!("form layout without a form, or the other way round"),
                }
            }
        }
    }

    #[test]
    fn the_error_hangs_under_the_folder_field_too() {
        let size: Size<i32, Logical> = (1280, 800).into();
        let mut view = form_view(0, false);
        let plain = project_menu_layout(&view, size);
        view.form.as_mut().unwrap().error = Some((FormField::Folder, "No folder at ~/x.".into()));
        let l = project_menu_layout(&view, size);
        let f = l.form.as_ref().unwrap();
        assert!(below(f.folder, f.error) && below(f.error, f.kinds[0].1));
        assert_eq!(f.error.loc.x, f.folder.loc.x);
        // With the folders offered open, it hangs under them.
        let mut open = view.clone();
        let suggest = open.form.as_mut().unwrap();
        suggest.suggestions = vec!["~/x".into(), "~/xy".into()];
        let o = project_menu_layout(&open, size).form.unwrap();
        assert!(below(*o.suggestions.last().unwrap(), o.error));
        assert!(below(o.error, o.kinds[0].1));
        // The rows under it move down by the line; the name row stays.
        let p = plain.form.as_ref().unwrap();
        assert_eq!(f.name, p.name);
        assert_eq!(f.folder, p.folder);
        assert_eq!(f.create.loc.y, p.create.loc.y + super::ERROR_HEIGHT);
        assert_eq!(l.panel.size.h, plain.panel.size.h + super::ERROR_HEIGHT);
    }

    #[test]
    fn the_footer_wraps_its_keycap_hints() {
        let size: Size<i32, Logical> = (1280, 800).into();
        let list = list_view();
        let few = settings_view("");
        let tall = project_menu_layout(&list, size).footer;
        let short = project_menu_layout(&few, size).footer;
        assert_eq!(short.size.h, super::footer_height(1));
        assert_eq!(tall.size.h, short.size.h + super::FOOTER_ROW);
        assert_eq!(super::footer_rows(4), 1);
        assert_eq!(super::footer_rows(5), 2);

        // In order, inside the footer, on its rows.
        let spots = project_menu_layout(&list, size).hints;
        let order: Vec<usize> = spots.iter().map(|(i, _)| *i).collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
        assert!(!spots.is_empty());
        for (_, spot) in &spots {
            assert!(tall.contains_rect(*spot), "{spot:?} {tall:?}");
        }
    }

    #[test]
    fn project_menu_page_stays_in_bounds() {
        let mut last = 0;
        for h in (100..2200).step_by(10) {
            let page = project_menu_page((1000, h).into());
            assert!((3..=12).contains(&page) && page >= last);
            last = page;
        }
        assert_eq!(project_menu_page((1000, 100).into()), 3);
        assert_eq!(project_menu_page((1920, 1080).into()), 12);
        assert!(project_menu_page((800, 600).into()) > 3);
    }

    /// The list with more projects than fit, `part` scrolled.
    fn long_view(part: f64) -> ProjectMenuView {
        let mut view = list_view();
        let more = (0..20).map(|i| project(menu_row(&format!("p{i}"), None, false), None));
        view.entries.splice(3..3, more);
        view.part = part;
        view
    }

    #[test]
    fn a_scrolled_list_moves_up_and_keeps_its_height() {
        let size: Size<i32, Logical> = (1280, 800).into();
        let (a, b) = (
            project_menu_layout(&long_view(0.0), size),
            project_menu_layout(&long_view(30.0), size),
        );
        // As high as the room, whatever the scroll; the entries moved up.
        assert_eq!((a.list, a.panel), (b.list, b.panel));
        assert_eq!(a.list.size.h, project_menu_room(size));
        let bottom = b.list.loc.y + b.list.size.h;
        for ((ra, _), (rb, _)) in a.entries.iter().zip(&b.entries) {
            if ra.size.h > 0 && rb.size.h > 0 {
                assert_eq!(rb.loc.y, ra.loc.y - 30);
            }
        }
        // The first entry, a section's label, went up out of the list and is
        // cut to nothing; the next one reaches up out of it, and one is
        // shown in part at its bottom; those under it are cut to nothing.
        let cut = |(rect, _): &(Rect, _)| rect.size.h == 0;
        assert!(cut(&b.entries[0]));
        assert_eq!(
            b.entries[1].0.loc.y,
            b.list.loc.y + PROJECT_MENU_SECTION - 30
        );
        let last = b.entries.iter().rposition(|e| !cut(e)).unwrap();
        let (rect, _) = b.entries[last];
        assert!(rect.loc.y < bottom && rect.loc.y + rect.size.h > bottom);
        assert!(b.entries[last + 1..].iter().all(cut));
        // Above the list, the first entry is not hit.
        let above = (b.list.loc.x as f64 + 40.0, b.list.loc.y as f64 - 2.0);
        let hit = project_menu_hit(&long_view(30.0), size, above.into());
        assert_ne!(hit, Some(MenuHit::Entry(0)));
        // Pulled down past the start, the entries come down.
        let c = project_menu_layout(&long_view(-20.0), size);
        assert_eq!(c.entries[1].0.loc.y, a.entries[1].0.loc.y + 20);
    }

    #[test]
    fn the_scrollbar_thumb_goes_in_the_gutter_right_of_the_entries() {
        let size: Size<i32, Logical> = (1280, 800).into();
        let view = ProjectMenuView {
            bar: Some(super::super::ListBar {
                down: 0.0,
                total: 2000.0,
            }),
            ..long_view(0.0)
        };
        let layout = project_menu_layout(&view, size);
        let thumb = super::project_menu_thumb(&view, &layout).unwrap();
        let (entry, panel) = (layout.entries[3].0, layout.panel);
        assert_eq!(thumb.track.loc.x, entry.loc.x + entry.size.w + 1);
        assert!(thumb.track.loc.x + thumb.track.size.w < panel.loc.x + panel.size.w);
        assert_eq!(thumb.track.loc.y, layout.list.loc.y);
        // In form mode, there is none.
        assert_eq!(
            super::project_menu_thumb(&form_view(0, false), &layout),
            None
        );
    }

    #[test]
    fn scrolling_by_part_of_a_row_draws_nothing_again() {
        let painter = Painter::new();
        let size: Size<i32, Logical> = (1280, 800).into();
        let mut layers = ProjectMenuLayers::default();
        let start = std::time::Instant::now();
        let draw = |layers: &mut ProjectMenuLayers, part: f64, ms: u64| {
            let view = ProjectMenuView {
                hover: Some(MenuHit::Entry(3)),
                part,
                ..list_view()
            };
            let at = start + std::time::Duration::from_millis(ms);
            layers.parts(Some(view), size, Theme::Dark, 1.25, at, &painter)
        };
        // Faded in first.
        draw(&mut layers, 0.0, 0);
        let first = draw(&mut layers, 5.0, 300);
        let (frames, entries) = (layers.frames.get(), layers.entries.drawn());
        assert_eq!(frames, 1);
        let moved = draw(&mut layers, 17.5, 400);
        assert_eq!(layers.frames.get(), frames);
        assert_eq!(layers.entries.drawn(), entries);
        // Only the entries and the line over the new ones moved, by as
        // much; the frame stayed.
        assert_eq!(moved.len(), first.len());
        assert_eq!(moved.last().unwrap().loc, first.last().unwrap().loc);
        for (a, b) in first.iter().zip(&moved).take(first.len() - 1) {
            assert_eq!(b.loc.y, a.loc.y - 12.5);
        }
    }

    #[test]
    fn project_menu_hit_agrees_with_the_layout() {
        let size: Size<i32, Logical> = (1280, 800).into();
        let mut view = list_view();
        let l = project_menu_layout(&view, size);
        let hit = |view: &ProjectMenuView, pos| project_menu_hit(view, size, pos);
        assert!(l.entries.iter().all(|(_, buttons)| buttons.is_empty()));
        assert_eq!(hit(&view, centre(l.query)), Some(MenuHit::Query));
        let corner = (l.footer.loc.x as f64 + 2.0, l.footer.loc.y as f64 + 2.0);
        assert_eq!(hit(&view, corner.into()), Some(MenuHit::Panel));
        assert_eq!(hit(&view, (1200.0, 700.0).into()), None);
        assert_eq!(hit(&view, (2.0, 60.0).into()), None);
        for (i, (rect, _)) in l.entries.iter().enumerate() {
            assert_eq!(hit(&view, centre(*rect)), Some(MenuHit::Entry(i)));
        }

        // Row buttons: only on the hovered project row, and which depend on
        // whether it is current and runs.
        use RowButton::{Forget, ProjectFile, Rename, Settings, Stop};
        let expected: [&[RowButton]; 8] = [
            &[],
            &[Settings, ProjectFile, Rename],
            &[],
            &[Settings, ProjectFile, Rename, Stop, Forget],
            &[Settings, ProjectFile, Rename, Forget],
            &[Settings, ProjectFile, Rename, Stop, Forget],
            &[],
            &[],
        ];
        for (i, want) in expected.iter().enumerate() {
            for hover in [MenuHit::Entry(i), MenuHit::Button(i, Rename)] {
                view.hover = Some(hover);
                let l = project_menu_layout(&view, size);
                for (j, (rect, buttons)) in l.entries.iter().enumerate() {
                    let shown: Vec<_> = buttons.iter().map(|(b, _)| *b).collect();
                    assert_eq!(shown, if i == j { want.to_vec() } else { vec![] });
                    for (button, b) in buttons {
                        assert_eq!(hit(&view, centre(*b)), Some(MenuHit::Button(j, *button)));
                    }
                    // Left of the buttons is still the row.
                    let left = (rect.loc.x as f64 + 20.0, centre(*rect).y).into();
                    assert_eq!(hit(&view, left), Some(MenuHit::Entry(j)));
                }
                // Right-aligned in order, side by side.
                let buttons = &l.entries[i].1;
                assert!(
                    buttons
                        .windows(2)
                        .all(|b| b[0].1.loc.x + b[0].1.size.w <= b[1].1.loc.x)
                );
            }
        }

        let view = form_view(3, true);
        let l = project_menu_layout(&view, size);
        let f = l.form.as_ref().unwrap();
        let fields = [
            (f.name, FormField::Name),
            (f.folder, FormField::Folder),
            (f.browse, FormField::Browse),
            (f.cancel, FormField::Cancel),
            (f.create, FormField::Create),
        ];
        for (rect, field) in fields {
            assert_eq!(hit(&view, centre(rect)), Some(MenuHit::Field(field)));
        }
        for (kind, rect) in f.kinds {
            assert_eq!(hit(&view, centre(rect)), Some(MenuHit::Kind(kind)));
        }
        for (i, rect) in f.suggestions.iter().enumerate() {
            assert_eq!(hit(&view, centre(*rect)), Some(MenuHit::Suggestion(i)));
        }
        for nothing in [l.query, f.error] {
            assert_eq!(hit(&view, centre(nothing)), Some(MenuHit::Panel));
        }
        // The footer's key hints are buttons.
        assert!(!l.hints.is_empty());
        for (i, rect) in &l.hints {
            assert_eq!(hit(&view, centre(*rect)), Some(MenuHit::Hint(*i)));
        }
        let past = l.panel.loc.y as f64 + l.panel.size.h as f64 + 1.0;
        assert_eq!(hit(&view, (100.0, past).into()), None);

        let view = settings_view("docker compose up -d");
        let l = project_menu_layout(&view, size);
        assert!(l.entries.is_empty() && l.form.is_none());
        let f = l.settings.as_ref().unwrap();
        assert!(below(l.query, f.before) && below(f.before, f.after));
        assert!(below(f.after, f.note) && below(f.note, f.save));
        assert!(below(f.save, l.footer));
        assert!(f.cancel.loc.x + f.cancel.size.w < f.run.loc.x);
        assert!(f.run.loc.x + f.run.size.w < f.save.loc.x);
        let fields = [
            (f.before, SettingsField::Before),
            (f.after, SettingsField::After),
            (f.cancel, SettingsField::Cancel),
            (f.run, SettingsField::Run),
            (f.save, SettingsField::Save),
        ];
        for (rect, field) in fields {
            assert!(inside(l.panel, rect), "{rect:?} outside the panel");
            assert_eq!(hit(&view, centre(rect)), Some(MenuHit::Setting(field)));
        }
        for nothing in [l.query, f.note] {
            assert_eq!(hit(&view, centre(nothing)), Some(MenuHit::Panel));
        }
        for (i, rect) in &l.hints {
            assert_eq!(hit(&view, centre(*rect)), Some(MenuHit::Hint(*i)));
        }
    }

    #[test]
    fn project_menu_header_and_notice_paint() {
        let painter = Painter::new();
        let mut list = list_view();
        list.hover = Some(MenuHit::Button(4, RowButton::Forget));
        if let MenuEntry::Project { confirm_forget, .. } = &mut list.entries[4] {
            *confirm_forget = true;
        }
        let mut renaming = list_view();
        renaming.hover = Some(MenuHit::Entry(3));
        if let MenuEntry::Project { renaming: name, .. } = &mut renaming.entries[3] {
            *name = Some("gerdr renamed".into());
        }
        let mut empty = list_view();
        empty.entries.clear();
        empty.query = Line::default();
        let mut views = vec![
            list,
            renaming,
            empty,
            form_view(5, true),
            form_view(0, false),
        ];
        let mut no_folder = form_view(0, false);
        if let Some(form) = &mut no_folder.form {
            form.folder = Line::default();
            form.focus = FormField::Kind;
        }
        views.push(no_folder);
        views.push(settings_view(""));
        let mut long_line = settings_view(&"docker compose up -d && ".repeat(8));
        long_line.hover = Some(MenuHit::Setting(SettingsField::Run));
        views.push(long_line);
        let long = "a project with a very long name indeed, longer than the header";
        for theme in [Theme::Dark, Theme::Light] {
            for scale in [1.0, 1.25, 2.0] {
                for (w, h) in [(800, 600), (1920, 1080), (120, 90)] {
                    let size = (w, h).into();
                    for view in &views {
                        painter.project_menu(view, size, scale, theme.palette());
                    }
                    let toast = ToastView {
                        id: 0,
                        text: "Switched to gerdr · aft keeps running".into(),
                        level: Level::Info,
                        action: None,
                        button_w: 0,
                        leaving: false,
                    };
                    painter.toast(&toast, (w / 2).max(1), false, scale, theme.palette());
                }
                for (name, lit) in [(Some(long), true), (Some("aft"), false), (None, false)] {
                    for width in [SIDEBAR_MIN_WIDTH, SIDEBAR_WIDTH, SIDEBAR_MAX_WIDTH] {
                        let size = (width, 400).into();
                        let view = SidebarView {
                            workspaces: Vec::new(),
                            agents: Vec::new(),
                            active: None,
                            project: name.map(str::to_owned),
                            header_lit: lit,
                            state: SidebarState::default(),
                        };
                        painter.sidebar_view(&view, size, scale, theme.palette());
                    }
                }
            }
        }
    }

    #[test]
    fn the_focused_field_is_found_where_it_is_drawn() {
        let size = Size::from((1280, 800));
        let field = |view: &ProjectMenuView| project_menu_field(view, size, 1.0).unwrap();
        let list = list_view();
        let l = project_menu_layout(&list, size);
        assert_eq!(field(&list).area, l.query);
        assert_eq!(field(&list).line, "blo");

        let mut renaming = list_view();
        renaming.hover = Some(MenuHit::Entry(3));
        if let MenuEntry::Project { renaming: name, .. } = &mut renaming.entries[3] {
            *name = Some("gerdr x".into());
        }
        let r = project_menu_layout(&renaming, size);
        let (row, buttons) = &r.entries[3];
        let spot = field(&renaming);
        assert_eq!(spot.line, "gerdr x");
        assert!(row.contains(spot.area.loc) && spot.area.size.w > 0);
        // It stops short of the row's buttons.
        let first = buttons.first().expect("the hovered row shows buttons").1;
        assert!(spot.area.loc.x + spot.area.size.w < first.loc.x);

        let mut form = form_view(0, false);
        let f = project_menu_layout(&form, size).form.unwrap();
        form.form.as_mut().unwrap().focus = FormField::Name;
        assert_eq!(field(&form).area, f.name);
        form.form.as_mut().unwrap().focus = FormField::Folder;
        assert_eq!(field(&form).area, f.folder);
        form.form.as_mut().unwrap().focus = FormField::Create;
        assert_eq!(project_menu_field(&form, size, 1.0), None);

        let mut settings = settings_view("make up");
        let st = project_menu_layout(&settings, size).settings.unwrap();
        assert_eq!(field(&settings).area, st.before);
        settings.settings.as_mut().unwrap().focus = SettingsField::After;
        assert_eq!(field(&settings).area, st.after);
    }
}

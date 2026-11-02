//! The switcher: everything gerdr can switch to (the workspaces, their tabs,
//! the coding agents, the other projects, the apps named in the config file,
//! the folders zoxide knows, and gerdr's own commands), in sections, narrowed
//! down by fuzzy search as the user types. Enter switches to the selected
//! item; a folder no workspace has yet opens as a project, an app launches
//! into a split. zoxide is optional: without it there are just no folders.
//!
//! Within a section, what was switched to often and recently comes first
//! (see `frecency`); with a query, how well it matches comes first and the
//! frecency breaks ties. The selection starts on the best match overall.
//!
//! The same panel is the run prompt: what is typed is a command to launch
//! into a split or a new tab, offered first, over the apps named in the
//! config file, the apps installed on the system and the commands typed
//! before; an app whose name starts with what is typed is selected instead.
//!
//! Alt+Enter launches the picked app or command in a new tab, wherever
//! Enter would launch it.
//!
//! A row of category chips over the list, "All" first, narrows it to one
//! section; Tab and Shift+Tab move along them. Up/Down (or Ctrl+J and
//! Ctrl+K) move the selection; Esc clears the query, then closes. The key
//! hints show in a footer.

use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
    ffi::OsStr,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Instant, SystemTime},
};

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    reexports::calloop::channel,
    utils::{Logical, Point},
};

use crate::{
    Gerdr, WorkspaceKind,
    agents::AgentKey,
    app_icons::AppIcon,
    chrome::{self, ItemIcon, SwitcherRow, SwitcherView, status_label},
    desktop, fuzzy, git, home_dir,
    input::Overlay,
    launch::{self, Place},
    paths::tilde,
    profile_import,
    project_list::RowTops,
    projects::Project,
    scroll::{Bounds, Motion, Scroll},
    text_field::{Edit, TextField},
    toast::{Level, ToastHit},
    tooltip::Tooltip,
};

/// What an item switches to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Workspace(usize),
    /// A tab of a workspace.
    Tab(usize, usize),
    /// An agent: herdr's in a pane, a Cursor chat.
    Agent(AgentKey),
    /// Another project, by id.
    Project(String),
    /// The project menu, to find, start or manage projects there (the
    /// welcome screen while no project is shown).
    Welcome,
    /// The key help, kept open.
    Shortcuts,
    /// The inbox panel.
    Inbox,
    /// An agent CLI's command, to run in a new tab of workspace `.0` (see
    /// `Gerdr::new_agent`).
    NewAgent(usize, String),
    /// The config file, to edit.
    Settings,
    /// A project's file, by id, to edit in a new tab.
    ProjectFile(String),
    /// The projects file, read again after an edit.
    ReloadProjects,
    /// A folder, opened as a project unless a workspace has it already.
    Folder(PathBuf),
    /// The command of an app named in the config file or installed, to
    /// launch.
    App(String),
    /// A command typed in the run prompt (now or before), to launch and
    /// remember.
    Command(String),
    /// The repository's page, to show in a tab of the shown workspace (see
    /// `Gerdr::open_page`).
    RepoPage(String),
    /// An app's own profile, to bring into gerdr's shared one of it.
    ImportProfile(profile_import::Source),
    /// T3 Code's app, to show for workspace `.0` (see `Gerdr::open_t3`).
    T3(usize),
    /// T3 Code, updated (see `Gerdr::update_t3`).
    UpdateT3,
    /// Taking newer versions of T3 Code without asking, turned on or off.
    T3Updates,
    /// The tasks running in the background (see `tasks`).
    Tasks,
}

impl Target {
    /// The section it is listed under.
    fn section(&self) -> Section {
        match self {
            Target::Workspace(_) => Section::Workspaces,
            Target::Tab(..) => Section::Tabs,
            Target::Agent(_) => Section::Agents,
            Target::Project(_) => Section::Projects,
            Target::App(_) => Section::Apps,
            Target::Folder(_) => Section::Folders,
            Target::Welcome
            | Target::Shortcuts
            | Target::Inbox
            | Target::NewAgent(..)
            | Target::T3(_)
            | Target::UpdateT3
            | Target::T3Updates
            | Target::Tasks
            | Target::Settings
            | Target::ProjectFile(_)
            | Target::ReloadProjects
            | Target::Command(_)
            | Target::RepoPage(_)
            | Target::ImportProfile(_) => Section::Commands,
        }
    }

    /// What kind of thing it is, for the pill on its row.
    fn kind(&self) -> &'static str {
        match self {
            Target::Workspace(_) => "workspace",
            Target::Tab(..) => "tab",
            Target::Agent(_) => "agent",
            Target::Project(_) => "project",
            Target::App(_) => "app",
            Target::Folder(_) => "folder",
            Target::Welcome
            | Target::Shortcuts
            | Target::Inbox
            | Target::NewAgent(..)
            | Target::T3(_)
            | Target::UpdateT3
            | Target::T3Updates
            | Target::Tasks
            | Target::Settings
            | Target::ProjectFile(_)
            | Target::ReloadProjects
            | Target::Command(_)
            | Target::RepoPage(_)
            | Target::ImportProfile(_) => "command",
        }
    }
}

/// The sections of the list, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    /// The run prompt's typed command.
    Run,
    Workspaces,
    Tabs,
    Agents,
    Projects,
    Apps,
    Folders,
    Commands,
}

impl Section {
    /// Its heading.
    pub fn label(self) -> &'static str {
        match self {
            Section::Run => "Run",
            Section::Workspaces => "Workspaces",
            Section::Tabs => "Tabs",
            Section::Agents => "Agents",
            Section::Projects => "Projects",
            Section::Apps => "Apps",
            Section::Folders => "Recent folders",
            Section::Commands => "Commands",
        }
    }

    /// Its category chip's label.
    fn chip(self) -> &'static str {
        match self {
            Section::Folders => "Folders",
            section => section.label(),
        }
    }
}

/// One thing to switch to, as listed.
pub struct Item {
    pub icon: ItemIcon,
    pub label: String,
    /// More about it, shown under the label, and searched too.
    pub detail: String,
    pub section: Section,
    /// Its frecency: what orders it among the items that match alike.
    pub weight: f32,
    pub target: Target,
}

impl Item {
    /// An item in its target's section, never switched to yet.
    fn new(
        icon: ItemIcon,
        label: impl Into<String>,
        detail: impl Into<String>,
        target: Target,
    ) -> Self {
        Self {
            icon,
            label: label.into(),
            detail: detail.into(),
            section: target.section(),
            weight: 0.0,
            target,
        }
    }
}

/// An item the query matches, with the positions (in chars) of the matched
/// characters in its label and detail.
struct Match {
    item: usize,
    label: Vec<usize>,
    detail: Vec<usize>,
}

/// What a key did to the switcher.
enum Outcome {
    Open,
    Closed,
    /// Enter picked this, to launch (if it is an app or a command) in that place.
    Picked(Target, Place),
}

pub struct Switcher {
    /// Tells it from the switchers opened before and after, for what comes
    /// back for it later (a paste).
    pub serial: u64,
    /// The run prompt: the first item is the query, as a command.
    run: bool,
    /// Where Enter launches an app or a command.
    place: Place,
    query: TextField,
    items: Vec<Item>,
    /// The items `query` matches, best first.
    matches: Vec<Match>,
    /// The selected match.
    selected: usize,
    /// Where the list stands, in matches.
    scroll: Scroll,
    /// Where the matches' rows go down the list.
    places: RowTops,
    /// How high the list may be, in logical pixels (see
    /// `chrome::switcher_room`).
    room: i32,
    /// The shown row the pointer is over.
    hover: Option<usize>,
    /// The sections the items span, but the typed command's: a category
    /// chip each.
    categories: Vec<Section>,
    /// The category the list is narrowed to; all of them when `None`.
    category: Option<Section>,
    /// The category chip the pointer is over, by index among the chips.
    chip_hover: Option<usize>,
    /// The key hint under the pointer, by index into `hints()`.
    hint_hover: Option<usize>,
    /// The installed apps' icons by program, for the typed command.
    icons: HashMap<String, AppIcon>,
}

impl Switcher {
    /// Stop the list flung; whether it was.
    pub fn stop_fling(&mut self) -> bool {
        self.scroll.stop_fling()
    }

    fn new(items: Vec<Item>, run: bool, place: Place) -> Self {
        static OPENED: AtomicU64 = AtomicU64::new(0);
        let mut switcher = Self {
            serial: OPENED.fetch_add(1, Ordering::Relaxed),
            run,
            place,
            query: TextField::default(),
            items: Vec::new(),
            matches: Vec::new(),
            selected: 0,
            scroll: Scroll::default(),
            places: RowTops::default(),
            room: i32::MAX,
            hover: None,
            categories: Vec::new(),
            category: None,
            chip_hover: None,
            hint_hover: None,
            icons: HashMap::new(),
        };
        switcher.set_items(items);
        switcher
    }

    /// Replace the items, keeping the query, and the selection on the same
    /// target if it is still there.
    fn set_items(&mut self, mut items: Vec<Item>) {
        let picked = self.picked().cloned();
        if self.run {
            // The typed command's place; `refilter` fills it in.
            items.insert(0, typed_command("", &self.icons, self.place));
        }
        let mut categories: Vec<Section> = items
            .iter()
            .map(|item| item.section)
            .filter(|section| *section != Section::Run)
            .collect();
        categories.sort();
        categories.dedup();
        if self.category.is_some_and(|c| !categories.contains(&c)) {
            self.category = None;
        }
        self.categories = categories;
        self.items = items;
        self.refilter();
        if let Some(i) = picked.and_then(|target| {
            self.matches
                .iter()
                .position(|m| self.items[m.item].target == target)
        }) {
            self.selected = i;
        }
        self.reveal();
    }

    /// Whether `section` is in the category shown; the typed command's
    /// always is.
    fn shows(&self, section: Section) -> bool {
        section == Section::Run || self.category.is_none_or(|c| c == section)
    }

    /// Match the items in the category against the query again, in their
    /// sections, and select the best match. In the run prompt, the typed
    /// command goes first and is selected, unless the best match is an app
    /// or a project file command whose name starts with it.
    fn refilter(&mut self) {
        let command = self.query.trim().to_owned();
        if self.run {
            self.items[0] = typed_command(&command, &self.icons, self.place);
        }
        let typed = self.run.then(|| self.items[0].target.clone());
        let candidates = self
            .items
            .iter()
            .enumerate()
            .skip(usize::from(self.run))
            .filter(|(_, item)| self.shows(item.section))
            // The typed command, run before, is listed once: first.
            .filter(|(_, item)| Some(&item.target) != typed.as_ref());
        let ranked = fuzzy::rank(
            &self.query,
            candidates,
            |(_, item)| format!("{} {}", item.label, item.detail),
            |(_, item)| item.weight,
        );
        let best = ranked.first().map(|r| r.item.0);
        let mut found: Vec<Match> = ranked
            .into_iter()
            .map(|r| {
                let (i, item) = r.item;
                let n = item.label.chars().count();
                let (label, detail): (Vec<_>, Vec<_>) = r.hits.into_iter().partition(|h| *h < n);
                // The query has no whitespace, so nothing lands on the space between.
                let detail = detail.into_iter().map(|h| h - n - 1).collect();
                Match {
                    item: i,
                    label,
                    detail,
                }
            })
            .collect();
        // A stable sort: within a section the ranking stays.
        found.sort_by_key(|m| self.items[m.item].section);
        let at = |found: &[Match], item: usize| found.iter().position(|m| m.item == item);
        let named = |item: usize| {
            let item = &self.items[item];
            matches!(
                item.target,
                Target::App(_) | Target::ProjectFile(_) | Target::ReloadProjects
            ) && item
                .label
                .to_lowercase()
                .starts_with(&command.to_lowercase())
        };
        let mut selected = 0;
        if self.run && !command.is_empty() {
            found.insert(
                0,
                Match {
                    item: 0,
                    label: Vec::new(),
                    detail: Vec::new(),
                },
            );
            if let Some(best) = best.filter(|best| named(*best)) {
                selected = at(&found, best).unwrap_or(0);
            }
        } else if !command.is_empty()
            && let Some(best) = best
        {
            selected = at(&found, best).unwrap_or(0);
        }
        let sections: Vec<Section> = found.iter().map(|m| self.items[m.item].section).collect();
        self.places = chrome::switcher_places(
            (0..sections.len()).map(|i| i == 0 || sections[i - 1] != sections[i]),
        );
        self.matches = found;
        self.selected = selected;
        self.scroll = Scroll::default();
        self.reveal();
    }

    /// Its text field: the query.
    pub fn field_mut(&mut self) -> &mut TextField {
        &mut self.query
    }

    /// Whether there are category chips: with more than one category.
    fn chips(&self) -> bool {
        self.categories.len() > 1
    }

    /// Show category chip `chip` (0 for all of them), keeping the query.
    fn set_category(&mut self, chip: usize) {
        let category = chip
            .checked_sub(1)
            .and_then(|i| self.categories.get(i).copied());
        if category != self.category {
            self.category = category;
            self.refilter();
        }
    }

    /// Move along the category chips by `by`, wrapping around.
    fn step_category(&mut self, by: isize) {
        let n = self.categories.len() as isize + 1;
        let at = self
            .category
            .and_then(|c| self.categories.iter().position(|s| *s == c))
            .map_or(0, |i| i as isize + 1);
        self.set_category((at + by).rem_euclid(n) as usize);
    }

    fn picked(&self) -> Option<&Target> {
        self.matches
            .get(self.selected)
            .map(|m| &self.items[m.item].target)
    }

    /// Handle a key: `sym` is the key's own symbol, `typed` the one with
    /// Shift and the layout applied.
    fn key(&mut self, mods: &ModifiersState, sym: Keysym, typed: Keysym) -> Outcome {
        // The query takes the keys that edit it first.
        match self.query.key(mods, sym, typed) {
            Edit::Changed => {
                self.refilter();
                return Outcome::Open;
            }
            Edit::Moved => return Outcome::Open,
            Edit::Ignored => {}
        }
        let ctrl = mods.ctrl;
        let page = self.shown().map_or(1, |(first, last)| last - first + 1) as isize;
        match sym {
            // Esc clears the query first, then closes.
            Keysym::Escape if !self.query.is_empty() => {
                self.query.clear();
                self.refilter();
            }
            Keysym::Escape => return Outcome::Closed,
            Keysym::Return | Keysym::KP_Enter => {
                if let Some(target) = self.picked() {
                    let place = if mods.alt { Place::Tab } else { self.place };
                    return Outcome::Picked(target.clone(), place);
                }
            }
            Keysym::Up | Keysym::KP_Up => self.step(-1),
            Keysym::Down | Keysym::KP_Down => self.step(1),
            Keysym::k if ctrl => self.step(-1),
            Keysym::j if ctrl => self.step(1),
            // Tab and Shift+Tab go along the category chips, both ways.
            Keysym::Tab | Keysym::ISO_Left_Tab if self.chips() => {
                let back = mods.shift || sym == Keysym::ISO_Left_Tab;
                self.step_category(if back { -1 } else { 1 })
            }
            Keysym::Page_Up | Keysym::KP_Page_Up => self.step(-page),
            Keysym::Page_Down | Keysym::KP_Page_Down => self.step(page),
            _ => {}
        }
        Outcome::Open
    }

    /// Move the selection `by` matches, scrolling to keep it shown.
    fn step(&mut self, by: isize) {
        let last = self.matches.len().saturating_sub(1) as isize;
        self.selected = (self.selected as isize + by).clamp(0, last) as usize;
        self.reveal();
    }

    /// How high the list is: as its rows, `room` at most.
    fn list_height(&self) -> f64 {
        self.places.end().min(self.room as f64)
    }

    fn bounds(&self) -> Bounds {
        self.places.bounds(&self.scroll, self.list_height())
    }

    /// Let the list be `room` high at most, keeping the selection shown.
    fn set_room(&mut self, room: i32) {
        if room != self.room {
            self.room = room;
            self.reveal();
        }
    }

    /// Scroll the list as little as shows the selection whole, at once.
    fn reveal(&mut self) {
        if self.matches.is_empty() {
            return;
        }
        let span = chrome::switcher_row_span(&self.places, self.selected);
        let to = self
            .places
            .reveal(self.scroll.pos(), span, self.list_height());
        if to != self.scroll.pos() {
            self.scroll.jump(to);
        }
    }

    /// The first and last matches shown whole, if any are.
    fn shown(&self) -> Option<(usize, usize)> {
        let (pos, h) = (self.scroll.pos(), self.list_height());
        let bottom = self.places.px(pos) + h;
        let mut whole = (self.scroll.first()..self.matches.len())
            .take_while(|&i| self.places.top(i) < bottom)
            .filter(|&i| {
                let span = chrome::switcher_row_span(&self.places, i);
                self.places.shows(pos, span, h)
            });
        let first = whole.next()?;
        Some((first, whole.last().unwrap_or(first)))
    }

    /// Move the selection as little as keeps it shown whole: scrolling the
    /// list drags it along.
    fn drag(&mut self) {
        if let Some((first, last)) = self.shown() {
            self.selected = self.selected.clamp(first, last);
        }
    }

    /// The wheel or the touchpad moved the list at `now`. Nothing is
    /// under the pointer while it moves.
    fn scroll(&mut self, motion: Motion, now: Instant) {
        self.scroll.apply(motion, now, self.bounds());
        self.drag();
        if self.scroll.moving(now) {
            self.hover = None;
        }
    }

    /// Stand the list `px` pixels down, as its scrollbar's thumb held
    /// there puts it, the selection dragged along.
    fn stand_at(&mut self, px: f64) {
        let max = self.bounds().max.max(0.0);
        self.scroll.jump(self.places.pos(px).clamp(0.0, max));
        self.drag();
    }

    /// Move the list on to `now`; whether it still moves.
    fn tick(&mut self, now: Instant) -> bool {
        let moving = self.scroll.tick(now, self.bounds());
        self.drag();
        if moving {
            self.hover = None;
        }
        moving
    }

    /// The rows in view, from the first one shown at least in part down
    /// past the list's bottom.
    fn view(&self) -> SwitcherView {
        let first = self.scroll.first().min(self.matches.len());
        let top = self.places.top(first);
        let part = self.places.px(self.scroll.pos()) - top - self.scroll.overscroll();
        let reach = self.list_height() + part.max(0.0);
        let mut last: Option<Section> = None;
        let rows: Vec<SwitcherRow> = self.matches[first..]
            .iter()
            .enumerate()
            .take_while(|(k, _)| self.places.top(first + k) - top < reach)
            .map(|(_, m)| {
                let item = &self.items[m.item];
                // A heading over the first row shown and wherever the
                // section changes.
                let section = (last != Some(item.section)).then(|| item.section.label());
                last = Some(item.section);
                SwitcherRow {
                    icon: item.icon.clone(),
                    label: item.label.clone(),
                    detail: item.detail.clone(),
                    kind: item.target.kind(),
                    section,
                    label_hits: m.label.clone(),
                    detail_hits: m.detail.clone(),
                }
            })
            .collect();
        let selected = self.selected.checked_sub(first).filter(|i| *i < rows.len());
        let hover = self.hover.filter(|h| *h < rows.len());
        let hint = match (self.run, self.place) {
            (true, Place::Split) => "Command to run in a split…",
            (true, Place::Tab) => "Command to run in a new tab…",
            (false, _) => "Go to a workspace, tab, agent, project, app, folder or command…",
        };
        let categories: Vec<&'static str> = if self.chips() {
            std::iter::once("All")
                .chain(self.categories.iter().map(|c| c.chip()))
                .collect()
        } else {
            Vec::new()
        };
        let category = self
            .category
            .and_then(|c| self.categories.iter().position(|s| *s == c))
            .map_or(0, |i| i + 1);
        let hints = self.hints();
        SwitcherView {
            query: self.query.line(),
            hint,
            category_hover: self.chip_hover.filter(|h| *h < categories.len()),
            categories,
            category,
            rows,
            selected,
            hover,
            hint_hover: self.hint_hover.filter(|h| *h < hints.len()),
            hints,
            part,
            bar: (self.places.end() > self.list_height()).then(|| chrome::ListBar {
                down: self.places.px(self.scroll.pos()),
                total: self.places.end(),
            }),
        }
    }

    /// The footer's key hints: at most `chrome::SWITCHER_HINTS`.
    fn hints(&self) -> Vec<(String, String)> {
        let mut hints = vec![("Enter", if self.run { "run" } else { "open" })];
        if self.place == Place::Split {
            hints.push(("Alt+Enter", "in a new tab"));
        }
        if self.chips() {
            hints.push(("Tab", "category"));
        }
        hints.push((
            "Esc",
            if self.query.is_empty() {
                "close"
            } else {
                "clear"
            },
        ));
        hints
            .into_iter()
            .map(|(keys, what)| (keys.to_owned(), what.to_owned()))
            .collect()
    }

    /// The target of shown row `row`.
    fn row_target(&self, row: usize) -> Option<&Target> {
        let m = self.matches.get(self.scroll.first() + row)?;
        Some(&self.items[m.item].target)
    }
}

fn app_item(app: &launch::App, icons: &HashMap<String, AppIcon>) -> Item {
    let icon = app
        .icon
        .clone()
        .map_or_else(|| command_icon(&app.command, icons), ItemIcon::Picture);
    Item::new(
        icon,
        app.name.clone(),
        app.detail.clone(),
        Target::App(app.command.clone()),
    )
}

/// The icon of the installed app `command` starts, if any.
fn command_icon(command: &str, icons: &HashMap<String, AppIcon>) -> ItemIcon {
    launch::program_name(command)
        .and_then(|program| icons.get(&program).cloned())
        .map_or(ItemIcon::App, ItemIcon::Picture)
}

/// The run prompt's item for the typed `command`, to run in `place`: an
/// address, even without `https://`, opens as a web page.
fn typed_command(command: &str, icons: &HashMap<String, AppIcon>, place: Place) -> Item {
    let url = desktop::as_url(command);
    let detail = match (place, url.is_some()) {
        (Place::Split, false) => "run in a split",
        (Place::Tab, false) => "run in a new tab",
        (Place::Split, true) => "open in a split",
        (Place::Tab, true) => "open in a new tab",
    };
    let target = url.unwrap_or_else(|| command.to_owned());
    let mut item = Item::new(
        command_icon(command, icons),
        command,
        detail,
        Target::Command(target),
    );
    item.section = Section::Run;
    item
}

/// The frecency key of `target`, given the folder of the workspace it
/// names; none for what is always listed last.
fn frecency_key(
    target: &Target,
    workspace_dir: impl Fn(usize) -> Option<PathBuf>,
) -> Option<String> {
    Some(match target {
        Target::Workspace(w) => format!("workspace:{}", workspace_dir(*w)?.display()),
        Target::Tab(w, t) => format!("tab:{}:{t}", workspace_dir(*w)?.display()),
        Target::Agent(key) => format!("agent:{key}"),
        Target::NewAgent(_, command) => format!("agent-cli:{command}"),
        Target::T3(_) => "t3".to_owned(),
        Target::Project(id) => format!("project:{id}"),
        Target::Folder(dir) => format!("folder:{}", dir.display()),
        Target::App(command) | Target::Command(command) | Target::RepoPage(command) => {
            format!("app:{command}")
        }
        Target::Welcome
        | Target::Shortcuts
        | Target::Inbox
        | Target::Settings
        | Target::ProjectFile(_)
        | Target::ReloadProjects
        | Target::UpdateT3
        | Target::T3Updates
        | Target::Tasks
        | Target::ImportProfile(_) => return None,
    })
}

/// `what` a key hint does, as a tooltip says it: with a capital.
fn sentence(what: &str) -> String {
    let mut chars = what.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The other projects than `current`, the most recently used first, and a
/// row for the project menu (the welcome screen while no project is shown).
fn project_items(projects: &[Project], current: Option<&str>) -> Vec<Item> {
    let mut others: Vec<&Project> = projects
        .iter()
        .filter(|project| Some(project.id.as_str()) != current)
        .collect();
    others.sort_by_key(|project| Reverse(project.last_used));
    let mut items: Vec<Item> = others
        .into_iter()
        .map(|project| {
            let n = project.workspaces.len();
            let plural = if n == 1 { "" } else { "s" };
            Item::new(
                ItemIcon::Workspace(WorkspaceKind::Project),
                project.name.clone(),
                format!("project · {n} workspace{plural}"),
                Target::Project(project.id.clone()),
            )
        })
        .collect();
    items.push(Item::new(
        ItemIcon::Folder,
        "Projects…",
        "find, create, manage",
        Target::Welcome,
    ));
    items
}

/// The rows for the shown project's file (none while no project is shown),
/// to edit, and for reading the project files again after an edit.
fn project_file_items(current: Option<&str>, home: Option<&Path>) -> Vec<Item> {
    let mut items: Vec<Item> = current
        .map(|id| {
            Item::new(
                ItemIcon::Settings,
                "Project file",
                tilde(&crate::projects::project_file(id), home),
                Target::ProjectFile(id.to_owned()),
            )
        })
        .into_iter()
        .collect();
    items.push(Item::new(
        ItemIcon::Command,
        "Reload project file",
        "apply what was edited in it",
        Target::ReloadProjects,
    ));
    items
}

impl Gerdr {
    /// Close what the switcher opens in place of, as the other popups do
    /// when they open: one popup at a time, no key repeating into it, no
    /// menu or tooltip left over it.
    fn make_room_for_switcher(&mut self) {
        self.project_menu = None;
        self.inbox.close();
        self.scrollback = None;
        self.worktrees.close();
        self.repos.close();
        self.stop_key_repeat();
        self.close_menu();
        self.tooltip.hide();
        self.chrome.drop_tooltip();
    }

    /// Open the switcher, and ask zoxide for its folders again meanwhile.
    pub fn open_switcher(&mut self) {
        self.make_room_for_switcher();
        let mut switcher = Switcher::new(self.switcher_items(), false, Place::Split);
        switcher.set_room(self.switcher_room());
        self.switcher = Some(switcher);
        self.query_zoxide();
    }

    /// Open the switcher as the agent picker: a row per installed agent
    /// CLI, to start in a new tab of workspace `w`.
    pub fn open_agent_picker(&mut self, w: usize) {
        self.refresh_cli_agents();
        let items = self.agent_entries(w);
        if items.is_empty() {
            self.toast("No agent CLI found on PATH".into(), Level::Info);
            return;
        }
        self.make_room_for_switcher();
        let mut switcher = Switcher::new(self.weighed(items), false, Place::Tab);
        switcher.set_room(self.switcher_room());
        self.switcher = Some(switcher);
    }

    /// Open the switcher as the run prompt, for a command to launch in
    /// `place`, and look for installed apps again meanwhile.
    pub fn open_run_prompt(&mut self, place: Place) {
        self.make_room_for_switcher();
        // The commands run before there was frecency start it off, once.
        let history: Vec<String> = launch::history()
            .iter()
            .map(|command| format!("app:{command}"))
            .collect();
        if !history.is_empty() {
            self.frecency.seed(&history, SystemTime::now());
        }
        if let Some(dir) = self.workspace().map(|ws| ws.dir.clone()) {
            self.repo_pages
                .entry(dir)
                .or_insert_with_key(|dir| git::web_url(dir));
        }
        let mut switcher = Switcher::new(Vec::new(), true, place);
        switcher.icons = self.program_icons();
        switcher.set_items(self.run_items());
        switcher.set_room(self.switcher_room());
        self.switcher = Some(switcher);
        self.scan_installed_apps();
    }

    /// Bring the open run prompt's items up to date.
    pub fn refresh_run_prompt(&mut self) {
        if let Some(mut switcher) = self.switcher.take_if(|s| s.run) {
            switcher.icons = self.program_icons();
            switcher.set_items(self.run_items());
            self.switcher = Some(switcher);
        }
    }

    /// What the run prompt offers: the page of the workspace's repository,
    /// the apps named in the config file and the installed apps, the
    /// commands typed before, then the inbox, the rows importing other
    /// apps' profiles and the project file's commands.
    fn run_items(&self) -> Vec<Item> {
        let icons = self.program_icons();
        let page = self
            .workspace()
            .and_then(|ws| self.repo_pages.get(&ws.dir).cloned().flatten());
        let mut items: Vec<Item> = page
            .iter()
            .map(|url| {
                let shown = url.split_once("://").map_or(&**url, |(_, rest)| rest);
                Item::new(
                    ItemIcon::App,
                    "Repo page",
                    shown,
                    Target::RepoPage(url.clone()),
                )
            })
            .collect();
        items.extend(self.app_items());
        items.extend(self.installed_apps.iter().map(|app| app_item(app, &icons)));
        let history = launch::history()
            .into_iter()
            .filter(|command| page.as_ref() != Some(command));
        items.extend(history.map(|command| {
            Item::new(
                command_icon(&command, &icons),
                command.clone(),
                "run before",
                Target::Command(command),
            )
        }));
        items.extend(self.agent_entries(self.active_workspace));
        items.extend(self.t3_update_entries());
        items.push(self.inbox_entry());
        items.push(self.tasks_entry());
        let home = home_dir();
        if let Some(home) = &home {
            items.extend(profile_import::sources(home).into_iter().map(|source| {
                Item::new(
                    command_icon(&source.key, &icons),
                    format!("Import {} profile", source.name),
                    format!("into gerdr, from {}", tilde(&source.from, Some(home))),
                    Target::ImportProfile(source),
                )
            }));
        }
        items.extend(project_file_items(self.project.as_deref(), home.as_deref()));
        self.weighed(items)
    }

    /// A row per installed agent CLI, starting it in workspace `w`,
    /// naming it, and one for T3 Code's app once T3 Code ran here; none
    /// without the workspace.
    fn agent_entries(&self, w: usize) -> Vec<Item> {
        let Some(ws) = self.workspaces.get(w) else {
            return Vec::new();
        };
        let t3 = self.t3.home.is_some().then(|| {
            Item::new(
                ItemIcon::Command,
                "T3 Code",
                format!("its agents' threads, in {}", ws.name),
                Target::T3(w),
            )
        });
        self.cli_agents
            .iter()
            .map(|agent| {
                let icon = agent.icon.picture().map_or(ItemIcon::Command, |picture| {
                    ItemIcon::Picture(picture.clone())
                });
                Item::new(
                    icon,
                    format!("New {} agent", agent.name),
                    format!("in {}", ws.name),
                    Target::NewAgent(w, agent.command.clone()),
                )
            })
            .chain(t3)
            .collect()
    }

    /// The launcher's rows updating T3 Code, once it ran here.
    fn t3_update_entries(&self) -> Vec<crate::switcher::Item> {
        if self.t3.home.is_none() {
            return Vec::new();
        }
        let updates = &self.t3.updates;
        let running = self.t3.server.as_ref().and_then(|s| s.version.clone());
        let mut detail = running.map_or_else(|| "not running".to_owned(), |v| format!("runs {v}"));
        if let Some(pin) = updates
            .pin
            .as_ref()
            .filter(|pin| detail != format!("runs {pin}"))
        {
            detail.push_str(&format!(", {pin} next"));
        }
        if let crate::t3_agents::Source::Cli(_) = self.t3.source {
            detail.push_str(" · gerdr shows basic status only");
        }
        // Settings' `[t3] updates`, written there.
        let auto = if self.t3.auto_update {
            Item::new(
                ItemIcon::Command,
                "Turn off T3 Code updates in Settings",
                "update only when asked \u{b7} [t3] updates = false",
                Target::T3Updates,
            )
        } else {
            Item::new(
                ItemIcon::Command,
                "Turn on T3 Code updates in Settings",
                "to each newer version gerdr fully reads, checked every 6 hours \u{b7} [t3] updates",
                Target::T3Updates,
            )
        };
        vec![
            Item::new(
                ItemIcon::Command,
                "Update T3 Code",
                detail,
                Target::UpdateT3,
            ),
            auto,
        ]
    }

    /// The row opening the inbox, saying how many items its button counts.
    fn inbox_entry(&self) -> Item {
        let detail = match self.inbox_count_now() {
            0 => "agents, Git, GitHub and todos".to_owned(),
            1 => "1 needs you".to_owned(),
            n => format!("{n} need you"),
        };
        Item::new(ItemIcon::Command, "Inbox", detail, Target::Inbox)
    }

    fn tasks_entry(&self) -> Item {
        let detail = match self.tasks().len() {
            0 => "nothing runs".to_owned(),
            1 => "1 runs".to_owned(),
            n => format!("{n} run"),
        };
        Item::new(ItemIcon::Command, "Background tasks", detail, Target::Tasks)
    }

    /// `items` with their frecency.
    fn weighed(&self, mut items: Vec<Item>) -> Vec<Item> {
        let now = SystemTime::now();
        for item in &mut items {
            if let Some(key) = self.frecency_key(&item.target) {
                item.weight = self.frecency.score(&key, now);
            }
        }
        items
    }

    /// The frecency key of `target` (see `frecency`).
    fn frecency_key(&self, target: &Target) -> Option<String> {
        frecency_key(target, |w| Some(self.workspaces.get(w)?.dir.clone()))
    }

    /// The pointer moved to `pos` while the switcher is open: the row under
    /// it lights up.
    pub fn switcher_motion(&mut self, pos: Point<f64, Logical>) -> Option<Tooltip> {
        let view = self.switcher_view()?;
        let layout = chrome::switcher_geometry(&view, self.content_rect());
        let hover = chrome::switcher_row_at(&layout, pos);
        let chip = layout
            .chips
            .iter()
            .position(|chip| chip.to_f64().contains(pos));
        let hint = chrome::switcher_hint(&view, &layout, pos);
        if let Some(switcher) = self.switcher.as_mut() {
            // Rows passing under the pointer don't light up.
            let moving = switcher.scroll.moving(Instant::now());
            let hover = hover.filter(|_| !moving);
            switcher.hover = hover;
            switcher.chip_hover = chip;
            switcher.hint_hover = hint;
        }
        // A chip says what it narrows the list to, a key hint what it
        // does; Tab steps through the chips.
        if let Some(i) = chip {
            let text = match view.categories.get(i)? {
                &"All" => "Show every section".to_owned(),
                label => format!("Show only {label}"),
            };
            return Some(Tooltip::new(&text, layout.chips[i]).key("Tab"));
        }
        if let Some(i) = hint {
            let (keys, what) = view.hints.get(i)?;
            let (_, spot) = layout.hints.iter().find(|(h, _)| *h == i)?;
            return Some(Tooltip::new(&sentence(what), *spot).key(keys));
        }
        // An agent's mark says how it is doing.
        let i = hover?;
        let ItemIcon::Agent(_, status) = view.rows.get(i)?.icon else {
            return None;
        };
        let icon = chrome::switcher_icon(layout.rows[i]);
        icon.to_f64().contains(pos).then(|| {
            let text = format!("Agent {}", status_label(status));
            Tooltip::new(&text, icon)
        })
    }

    /// The installed apps' icons, by the program each starts.
    fn program_icons(&self) -> HashMap<String, AppIcon> {
        let mut icons = HashMap::new();
        for app in &self.installed_apps {
            if let (Some(icon), Some(program)) = (&app.icon, launch::program_name(&app.command)) {
                icons.entry(program).or_insert_with(|| icon.clone());
            }
        }
        icons
    }

    /// The switcher's rows, while it is open.
    pub fn switcher_view(&self) -> Option<SwitcherView> {
        Some(self.switcher.as_ref()?.view())
    }

    /// Bring the open switcher's items up to date.
    pub fn refresh_switcher(&mut self) {
        if let Some(mut switcher) = self.switcher.take_if(|s| !s.run) {
            let items = self.switcher_items();
            switcher.set_items(items);
            self.switcher = Some(switcher);
        }
    }

    /// A key pressed while the switcher is open (see `Switcher::key`):
    /// returns what Enter picked, and where to launch it.
    pub fn switcher_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<(Target, Place)> {
        let room = self.switcher_room();
        let switcher = self.switcher.as_mut()?;
        switcher.set_room(room);
        match switcher.key(mods, sym, typed) {
            Outcome::Open => None,
            Outcome::Closed => {
                self.switcher = None;
                None
            }
            Outcome::Picked(target, place) => {
                self.switcher = None;
                Some((target, place))
            }
        }
    }

    /// The wheel or the touchpad moved the switcher's list.
    pub fn switcher_scroll(&mut self, motion: Motion) {
        let room = self.switcher_room();
        if let Some(switcher) = self.switcher.as_mut() {
            switcher.set_room(room);
            switcher.scroll(motion, Instant::now());
        }
    }

    /// Stand the switcher's list `px` pixels down, as its scrollbar's
    /// thumb held there puts it.
    pub fn switcher_scroll_to(&mut self, px: f64) {
        let room = self.switcher_room();
        if let Some(switcher) = self.switcher.as_mut() {
            switcher.set_room(room);
            switcher.stand_at(px);
        }
    }

    /// Move the switcher's list on to `now` if it moves on its own; whether it
    /// still moves (see `Scroll::moving`). Run before every frame, it also
    /// fits the list to the output as it is now, resized or not.
    pub fn switcher_scroll_tick(&mut self, now: Instant) -> bool {
        if self.switcher.is_none() {
            return false;
        }
        let room = self.switcher_room();
        let Some(switcher) = self.switcher.as_mut() else {
            return false;
        };
        switcher.set_room(room);
        switcher.tick(now)
    }

    /// A button pressed while the switcher is open: on a row it switches to
    /// that row's target, on a category chip it shows that category,
    /// outside the panel it closes the switcher, but on a toast presses
    /// that.
    pub fn switcher_click(&mut self, pos: Point<f64, Logical>) {
        // A key hint: its key, pressed.
        if let Some(view) = self.switcher_view() {
            let layout = chrome::switcher_geometry(&view, self.content_rect());
            if let (Some(i), Some(switcher)) =
                (chrome::switcher_hint(&view, &layout, pos), &self.switcher)
            {
                let into = Overlay::Switcher(switcher.serial);
                return self.press_hint(into, &view.hints[i].0, pos);
            }
        }
        let Some(mut switcher) = self.switcher.take() else {
            return;
        };
        let layout = chrome::switcher_geometry(&switcher.view(), self.content_rect());
        if let Some(chip) = layout
            .chips
            .iter()
            .position(|chip| chip.to_f64().contains(pos))
        {
            switcher.set_category(chip);
            self.switcher = Some(switcher);
            return;
        }
        match chrome::switcher_row_at(&layout, pos) {
            Some(row) => {
                if let Some(target) = switcher.row_target(row).cloned() {
                    self.switch_to(target, switcher.place);
                }
            }
            None if layout.panel.to_f64().contains(pos) => self.switcher = Some(switcher),
            // A toast takes the click, and the switcher stays.
            None => {
                if let Some(hit) = self.toast_press(pos) {
                    self.switcher = Some(switcher);
                    if let ToastHit::Action(action) = hit {
                        self.menu_action(action);
                    }
                }
            }
        }
    }

    /// Switch to `target`; an app or a command launches in `place`. It
    /// counts for the frecency.
    pub fn switch_to(&mut self, target: Target, place: Place) {
        if let Some(key) = self.frecency_key(&target) {
            self.frecency.touch(&key);
            self.frecency.save();
        }
        match target {
            Target::Workspace(w) => self.switch_workspace(w),
            Target::Tab(w, t) => {
                if let Some(ws) = self.workspaces.get_mut(w)
                    && t < ws.tabs.len()
                {
                    ws.active_tab = t;
                    self.switch_workspace(w);
                }
            }
            Target::Agent(key) => self.focus_agent(&key),
            Target::Project(id) => self.switch_project(&id),
            // What Shift+P opens: the project menu while a project is
            // shown, else the welcome screen.
            Target::Welcome if self.project.is_some() => self.open_project_menu(),
            Target::Welcome => self.open_welcome(true),
            Target::Shortcuts => self.help_pinned = true,
            Target::Inbox => self.open_inbox(None),
            Target::NewAgent(w, command) => self.new_agent(w, &command),
            Target::T3(w) => self.open_t3(w, None),
            Target::UpdateT3 => self.update_t3(None),
            Target::T3Updates => self.toggle_t3_updates(),
            Target::Tasks => self.open_tasks(0),
            Target::Settings => self.open_settings(place),
            Target::ProjectFile(id) => self.open_project_file(&id),
            Target::ReloadProjects => self.reload_projects(),
            Target::Folder(dir) => match self.workspaces.iter().position(|ws| ws.dir == dir) {
                Some(w) => self.switch_workspace(w),
                None => self.open_project(dir),
            },
            Target::App(command) => self.launch(&command, place),
            Target::Command(command) => {
                launch::remember(&command);
                self.launch(&command, place);
            }
            Target::RepoPage(url) => self.open_page(self.active_workspace, url),
            Target::ImportProfile(source) => self.import_profile(source),
        }
    }

    /// Open the config file, made first when there is none: in the active
    /// workspace's Zed, shown, when it has one; else with `xdg-open`, in
    /// `place`.
    pub(crate) fn open_settings(&mut self, place: Place) {
        let path = crate::config::config_file();
        if let Err(err) = crate::config::ensure_file(&path) {
            let path = tilde(&path, home_dir().as_deref());
            return self.toast(format!("Can't make {path}: {err}"), Level::Error);
        }
        self.open_in_zed(&path, place, "the settings file");
    }

    /// Open gerdr's log file (see `crate::log_path`), as the settings file
    /// is opened.
    pub(crate) fn open_log(&mut self, place: Place) {
        let path = crate::log_path();
        self.open_in_zed(&path, place, "the log file");
    }

    /// Open project `id`'s file in a new Zed window, in a new tab of the
    /// active workspace: saved first (made if missing) so it shows what is
    /// on screen when the project is shown, and again once the tab is in
    /// it. Edits in it apply once reloaded.
    pub fn open_project_file(&mut self, id: &str) {
        if self.projects.project(id).is_none() {
            return;
        }
        let path = crate::projects::project_file(id);
        if self.project.as_deref() == Some(id) {
            self.record_tabs();
        }
        self.projects.save();
        if !path.exists() {
            let path = tilde(&path, home_dir().as_deref());
            return self.toast(format!("Can't make {path}"), Level::Error);
        }
        let zed = format!("zed -n {}", desktop::shell_quote(&path.to_string_lossy()));
        self.launch(&zed, Place::Tab);
        if self.record_tabs() {
            self.projects.save();
        }
    }

    /// Open `path` in the active workspace's Zed, shown, when it has one;
    /// else with `xdg-open`, in `place`. `what` names it in a toast.
    fn open_in_zed(&mut self, path: &Path, place: Place, what: &str) {
        let zed = self.workspace().and_then(|ws| {
            ws.tabs.iter().enumerate().find_map(|(t, tab)| {
                let pane = tab.panes.iter().find(|p| crate::is_zed(&p.window))?;
                Some((t, pane.window.clone(), ws.dir.clone()))
            })
        });
        let Some((t, window, dir)) = zed else {
            let open = format!("xdg-open {}", desktop::shell_quote(&path.to_string_lossy()));
            return self.launch(&open, place);
        };
        self.switch_tab(t);
        self.focus(Some(window));
        // `--existing`: in the Zed window used last, the one just focused.
        let mut zed = Command::new("zed");
        zed.arg("--user-data-dir")
            .arg(&self.zed_data)
            .arg("--existing")
            .arg(path);
        if self.spawn(zed, &dir, "zed").is_none() {
            self.toast(format!("Zed didn't open {what}"), Level::Error);
        }
    }

    /// How high the switcher's list may be.
    fn switcher_room(&self) -> i32 {
        chrome::switcher_room(self.content_rect())
    }

    /// The apps named in the config file, with the icons of the installed
    /// apps they start.
    fn app_items(&self) -> Vec<Item> {
        let icons = self.program_icons();
        self.apps.iter().map(|app| app_item(app, &icons)).collect()
    }

    /// Everything to switch to: the workspaces, their tabs (of those with
    /// more than one), the agents, the other projects, the apps, zoxide's
    /// folders not open yet, then gerdr's own commands: the project menu,
    /// the shortcuts, the settings file and the project file's commands.
    /// The sections sort them.
    fn switcher_items(&self) -> Vec<Item> {
        let mut items = Vec::new();
        let home = home_dir();
        for (w, (ws, (view, tabs))) in self
            .workspaces
            .iter()
            .zip(self.workspace_views())
            .enumerate()
        {
            let detail: Vec<String> = [Some(tilde(&ws.dir, home.as_deref())), view.branch]
                .into_iter()
                .flatten()
                .collect();
            let icon = match view.icon {
                Some(picture) => ItemIcon::Picture(picture),
                None => ItemIcon::Workspace(ws.kind),
            };
            items.push(Item::new(
                icon,
                ws.name.clone(),
                detail.join(" · "),
                Target::Workspace(w),
            ));
            if tabs.len() > 1 {
                for (t, tab) in tabs.into_iter().enumerate() {
                    let detail = format!("{} · tab {}", ws.name, t + 1);
                    items.push(Item::new(
                        ItemIcon::Tab,
                        tab.title,
                        detail,
                        Target::Tab(w, t),
                    ));
                }
            }
        }
        for (w, cards) in self.agent_views().into_iter().enumerate() {
            for card in cards {
                let mut detail = vec![status_label(card.status), self.workspaces[w].name.as_str()];
                detail.extend(card.title().map(str::trim).filter(|t| !t.is_empty()));
                items.push(Item::new(
                    ItemIcon::Agent(card.badge(), card.status),
                    card.label(),
                    detail.join(" · "),
                    Target::Agent(card.key.clone()),
                ));
            }
        }
        items.extend(project_items(
            &self.projects.projects,
            self.project.as_deref(),
        ));
        items.extend(self.app_items());
        let open: HashSet<&Path> = self.workspaces.iter().map(|ws| ws.dir.as_path()).collect();
        for dir in self
            .recent_dirs
            .iter()
            .filter(|dir| !open.contains(dir.as_path()))
        {
            let label = dir.file_name().map_or_else(
                || dir.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            items.push(Item::new(
                ItemIcon::Folder,
                label,
                tilde(dir, home.as_deref()),
                Target::Folder(dir.clone()),
            ));
        }
        items.push(self.inbox_entry());
        items.push(Item::new(
            ItemIcon::Command,
            "Shortcuts",
            "every key binding, kept on screen",
            Target::Shortcuts,
        ));
        items.push(Item::new(
            ItemIcon::Settings,
            "Settings file",
            tilde(&crate::config::config_file(), home.as_deref()),
            Target::Settings,
        ));
        items.extend(project_file_items(self.project.as_deref(), home.as_deref()));
        self.weighed(items)
    }

    /// Take in the folders zoxide knows, which `query_zoxide` reads off the
    /// event loop, and read them once now.
    pub fn watch_zoxide(&mut self) {
        let (tx, rx) = channel::channel();
        let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
            if let channel::Event::Msg(dirs) = event {
                state.recent_dirs = dirs;
                state.refresh_switcher();
                state.refresh_project_menu();
            }
        });
        match inserted {
            Ok(_) => {
                self.zoxide = Some(tx);
                self.query_zoxide();
            }
            Err(err) => tracing::warn!("zoxide: {err}"),
        }
    }

    /// Read zoxide's folders again, off the event loop, into `recent_dirs`.
    pub fn query_zoxide(&self) {
        let Some(tx) = self.zoxide.clone() else {
            return;
        };
        std::thread::spawn(move || {
            let _ = tx.send(zoxide_dirs());
        });
    }
}

/// The folders zoxide knows that still exist, the highest ranked first;
/// none without zoxide.
fn zoxide_dirs() -> Vec<PathBuf> {
    let out = Command::new("zoxide")
        .args(["query", "--list"])
        // zoxide leaves out the folder it runs in; "/" is no loss.
        .current_dir("/")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    match out {
        Ok(out) if out.status.success() => out
            .stdout
            .split(|b| *b == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| PathBuf::from(OsStr::from_bytes(line)))
            .collect(),
        Ok(_) => Vec::new(),
        Err(err) => {
            tracing::debug!("zoxide: {err}");
            Vec::new()
        }
    }
}

/// Tell zoxide gerdr opened `dir`, so it ranks higher, here and in the shell.
pub fn remember(dir: &Path) {
    let mut cmd = Command::new("zoxide");
    cmd.arg("add")
        .arg("--")
        .arg(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Waited for on a thread, so it doesn't linger as a zombie.
    std::thread::spawn(move || {
        let _ = cmd.status();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// How high a list of `rows` rows under one heading is.
    fn room(rows: usize) -> i32 {
        chrome::switcher_places((0..rows).map(|i| i == 0)).end() as i32
    }

    /// Move the list by `motion`, then let it come to rest, a frame at a
    /// time.
    fn settle(switcher: &mut Switcher, motion: Motion) {
        let mut now = Instant::now();
        switcher.scroll(motion, now);
        for _ in 0..250 {
            now += Duration::from_millis(8);
            switcher.tick(now);
        }
    }

    fn item(label: &str, detail: &str, w: usize) -> Item {
        Item::new(ItemIcon::Tab, label, detail, Target::Workspace(w))
    }

    fn type_in(switcher: &mut Switcher, text: &str) {
        for c in text.chars() {
            let sym = Keysym::from_char(c);
            switcher.key(&ModifiersState::default(), sym, sym);
        }
    }

    fn labels(switcher: &Switcher) -> Vec<String> {
        switcher
            .view()
            .rows
            .into_iter()
            .map(|row| row.label)
            .collect()
    }

    #[test]
    fn narrows_and_ranks() {
        let mut switcher = Switcher::new(
            vec![
                item("services-api", "~/work", 0),
                item("gerdr", "~/Personal", 1),
                item("zsh", "gerdr · tab 2", 2),
            ],
            false,
            Place::Split,
        );
        assert_eq!(labels(&switcher), ["services-api", "gerdr", "zsh"]);
        type_in(&mut switcher, "gerdr");
        assert_eq!(labels(&switcher), ["gerdr", "zsh"]);
        let view = switcher.view();
        assert_eq!(view.rows[0].label_hits, [0, 1, 2, 3, 4]);
        assert_eq!(view.rows[1].detail_hits, [0, 1, 2, 3, 4]);
        let ctrl = ModifiersState {
            ctrl: true,
            ..Default::default()
        };
        switcher.key(&ctrl, Keysym::u, Keysym::u);
        assert_eq!(labels(&switcher).len(), 3);
    }

    #[test]
    fn sections_group_the_rows_under_headings() {
        let mut switcher = Switcher::new(
            vec![
                Item::new(ItemIcon::App, "firefox", "", Target::App("firefox".into())),
                item("gerdr", "~/Personal/gerdr", 0),
                Item::new(ItemIcon::Folder, "herdr", "", Target::Folder("/h".into())),
                Item::new(ItemIcon::Command, "Shortcuts", "", Target::Shortcuts),
                item("aft", "~/Work/aft", 1),
                Item::new(ItemIcon::Folder, "Projects…", "", Target::Welcome),
            ],
            false,
            Place::Split,
        );
        let headed = |switcher: &Switcher| -> Vec<(String, &'static str, Option<&'static str>)> {
            switcher
                .view()
                .rows
                .into_iter()
                .map(|row| (row.label, row.kind, row.section))
                .collect()
        };
        assert_eq!(
            headed(&switcher),
            [
                ("gerdr".to_string(), "workspace", Some("Workspaces")),
                ("aft".to_string(), "workspace", None),
                ("firefox".to_string(), "app", Some("Apps")),
                ("herdr".to_string(), "folder", Some("Recent folders")),
                ("Shortcuts".to_string(), "command", Some("Commands")),
                ("Projects…".to_string(), "command", None),
            ]
        );
        // Scrolled into a section, the first row shown still has its heading.
        switcher.set_room(room(2));
        settle(&mut switcher, Motion::Notches(1.0));
        let rows = switcher.view().rows;
        assert_eq!(rows[0].section, Some("Workspaces"));
        assert_eq!(rows[1].section, Some("Apps"));
        // A query keeps the sections; the selection goes to the best match.
        type_in(&mut switcher, "sho");
        let view = switcher.view();
        assert_eq!(view.rows[0].section, Some("Commands"));
        assert_eq!(view.selected, Some(0));
    }

    #[test]
    fn category_chips_narrow_the_list_to_a_section() {
        let mut switcher = Switcher::new(
            vec![
                Item::new(ItemIcon::App, "firefox", "", Target::App("firefox".into())),
                item("gerdr", "~/Personal/gerdr", 0),
                Item::new(ItemIcon::Folder, "herdr", "", Target::Folder("/h".into())),
                item("aft", "~/Work/aft", 1),
            ],
            false,
            Place::Split,
        );
        // All of them at first.
        let view = switcher.view();
        assert_eq!(view.categories, ["All", "Workspaces", "Apps", "Folders"]);
        assert_eq!(view.category, 0);
        assert_eq!(labels(&switcher), ["gerdr", "aft", "firefox", "herdr"]);
        assert!(view.hints.iter().any(|(k, _)| k == "Tab"));
        // Tab moves to the next category, keeping the query.
        let none = ModifiersState::default();
        let shift = ModifiersState {
            shift: true,
            ..Default::default()
        };
        switcher.key(&none, Keysym::Tab, Keysym::Tab);
        assert_eq!(switcher.view().category, 1);
        assert_eq!(labels(&switcher), ["gerdr", "aft"]);
        type_in(&mut switcher, "af");
        assert_eq!(labels(&switcher), ["aft"]);
        assert_eq!(switcher.picked(), Some(&Target::Workspace(1)));
        // Shift+Tab goes back, and wraps around past "All".
        switcher.key(&shift, Keysym::ISO_Left_Tab, Keysym::ISO_Left_Tab);
        assert_eq!(switcher.view().category, 0);
        switcher.key(&shift, Keysym::ISO_Left_Tab, Keysym::ISO_Left_Tab);
        assert_eq!(switcher.view().category, 3);
        // A category gone from the items shows all of them again.
        switcher.set_items(vec![
            item("gerdr", "", 0),
            Item::new(ItemIcon::App, "x", "", Target::App("x".into())),
        ]);
        assert_eq!(switcher.view().category, 0);
        // With one category there are no chips, and Tab moves nothing:
        // it never walks the rows.
        switcher.set_items(vec![item("a", "", 0), item("b", "", 1)]);
        let ctrl = ModifiersState {
            ctrl: true,
            ..Default::default()
        };
        switcher.key(&ctrl, Keysym::u, Keysym::u);
        let view = switcher.view();
        assert!(view.categories.is_empty());
        assert!(view.hints.iter().all(|(k, _)| k != "Tab"));
        switcher.key(&none, Keysym::Tab, Keysym::Tab);
        assert_eq!(switcher.picked(), Some(&Target::Workspace(0)));
    }

    #[test]
    fn the_run_prompt_keeps_the_typed_command_in_any_category() {
        let mut switcher = Switcher::new(
            vec![
                Item::new(ItemIcon::App, "Files", "", Target::App("nautilus".into())),
                Item::new(
                    ItemIcon::App,
                    "fish",
                    "run before",
                    Target::Command("fish".into()),
                ),
            ],
            true,
            Place::Split,
        );
        assert_eq!(switcher.view().categories, ["All", "Apps", "Commands"]);
        switcher.set_category(2);
        type_in(&mut switcher, "f");
        assert_eq!(labels(&switcher), ["f", "fish"]);
        let hints = switcher.view().hints;
        assert_eq!(hints[0], ("Enter".into(), "run".into()));
        assert_eq!(hints.len(), chrome::SWITCHER_HINTS);
    }

    #[test]
    fn frecency_orders_a_section_and_breaks_ties() {
        let weighed = |label: &str, w: usize, weight: f32| {
            let mut item = item(label, "", w);
            item.weight = weight;
            item
        };
        let mut switcher = Switcher::new(
            vec![
                weighed("alpha", 0, 1.0),
                weighed("beta", 1, 5.0),
                weighed("gamma", 2, 2.0),
                weighed("delta", 3, 2.0),
                weighed("tablet", 4, 9.0),
                Item::new(
                    ItemIcon::App,
                    "alphabet",
                    "",
                    Target::App("alphabet".into()),
                ),
            ],
            false,
            Place::Split,
        );
        let labels = |switcher: &Switcher| -> Vec<String> {
            switcher
                .view()
                .rows
                .into_iter()
                .map(|row| row.label)
                .collect()
        };
        // Nothing typed: the weight orders, equal weights keep their order.
        assert_eq!(
            labels(&switcher),
            ["tablet", "beta", "gamma", "delta", "alpha", "alphabet"]
        );
        // "a" matches every row; the word start ("alpha") wins over the
        // "a" inside the others, which the weight then orders.
        type_in(&mut switcher, "a");
        assert_eq!(
            labels(&switcher),
            ["alpha", "tablet", "beta", "gamma", "delta", "alphabet"]
        );
        assert_eq!(switcher.view().selected, Some(0));
        // "abet" is whole in "alphabet" and scattered in "tablet": the
        // better match in the later section is selected, though the
        // heavier workspace is listed first.
        type_in(&mut switcher, "bet");
        assert_eq!(labels(&switcher), ["tablet", "alphabet"]);
        assert_eq!(switcher.picked(), Some(&Target::App("alphabet".into())));
        // Equal matches ("et" whole in "tablet", "beta" and "alphabet"):
        // the weight picks the heaviest.
        let ctrl = ModifiersState {
            ctrl: true,
            ..Default::default()
        };
        switcher.key(&ctrl, Keysym::u, Keysym::u);
        type_in(&mut switcher, "et");
        assert_eq!(labels(&switcher), ["tablet", "beta", "delta", "alphabet"]);
        assert_eq!(switcher.picked(), Some(&Target::Workspace(4)));
    }

    #[test]
    fn hover_names_a_row_shown() {
        let mut switcher = Switcher::new(
            (0..3).map(|w| item(&format!("ws{w}"), "", w)).collect(),
            false,
            Place::Split,
        );
        switcher.hover = Some(1);
        assert_eq!(switcher.view().hover, Some(1));
        switcher.hover = Some(7);
        assert_eq!(switcher.view().hover, None);
    }

    #[test]
    fn frecency_keys_name_the_target() {
        let dir = |w: usize| (w == 0).then(|| PathBuf::from("/home/x/gerdr"));
        let key = |t: &Target| frecency_key(t, dir);
        assert_eq!(
            key(&Target::Workspace(0)).as_deref(),
            Some("workspace:/home/x/gerdr")
        );
        assert_eq!(key(&Target::Workspace(3)), None);
        assert_eq!(
            key(&Target::Tab(0, 2)).as_deref(),
            Some("tab:/home/x/gerdr:2")
        );
        assert_eq!(
            key(&Target::App("imv a".into())).as_deref(),
            Some("app:imv a")
        );
        assert_eq!(
            key(&Target::Command("imv a".into())).as_deref(),
            Some("app:imv a")
        );
        assert_eq!(
            key(&Target::Folder("/tmp/x".into())).as_deref(),
            Some("folder:/tmp/x")
        );
        assert_eq!(
            key(&Target::Project("aft".into())).as_deref(),
            Some("project:aft")
        );
        assert_eq!(
            key(&Target::Agent(AgentKey::Herdr("p1".into()))).as_deref(),
            Some("agent:p1")
        );
        assert_eq!(key(&Target::Welcome), None);
        assert_eq!(key(&Target::Shortcuts), None);
        assert_eq!(key(&Target::Settings), None);
        assert_eq!(key(&Target::ProjectFile("aft".into())), None);
        assert_eq!(key(&Target::ReloadProjects), None);
    }

    #[test]
    fn selection_scrolls_and_picks() {
        let mut switcher = Switcher::new(
            (0..8).map(|w| item(&format!("ws{w}"), "", w)).collect(),
            false,
            Place::Split,
        );
        switcher.set_room(room(5));
        let none = ModifiersState::default();
        for _ in 0..6 {
            switcher.key(&none, Keysym::Down, Keysym::Down);
        }
        let view = switcher.view();
        assert_eq!(
            (view.rows[0].label.as_str(), view.selected),
            ("ws2", Some(4))
        );
        assert_eq!(switcher.row_target(0), Some(&Target::Workspace(2)));
        assert!(matches!(
            switcher.key(&none, Keysym::Return, Keysym::Return),
            Outcome::Picked(Target::Workspace(6), Place::Split)
        ));
        type_in(&mut switcher, "xyz");
        assert_eq!(switcher.view().selected, None);
        assert!(matches!(
            switcher.key(&none, Keysym::Return, Keysym::Return),
            Outcome::Open
        ));
    }

    #[test]
    fn wheel_scrolls_the_list_and_drags_the_selection() {
        let mut switcher = Switcher::new(
            (0..8).map(|w| item(&format!("ws{w}"), "", w)).collect(),
            false,
            Place::Split,
        );
        switcher.set_room(room(5));
        settle(&mut switcher, Motion::Notches(2.0));
        let view = switcher.view();
        assert_eq!(
            (view.rows[0].label.as_str(), view.selected, view.part),
            ("ws2", Some(0), 0.0)
        );
        settle(&mut switcher, Motion::Notches(10.0));
        assert_eq!(switcher.view().rows[0].label, "ws3");
        settle(&mut switcher, Motion::Notches(-10.0));
        let view = switcher.view();
        assert_eq!(
            (view.rows[0].label.as_str(), view.selected),
            ("ws0", Some(3))
        );
        // Nothing is under the pointer while the list moves.
        switcher.hover = Some(1);
        switcher.scroll(Motion::Notches(1.0), Instant::now());
        assert_eq!(switcher.view().hover, None);
        type_in(&mut switcher, "xyz");
        settle(&mut switcher, Motion::Notches(1.0));
        assert_eq!(switcher.view().selected, None);
    }

    #[test]
    fn the_scrollbar_thumb_stands_the_list_where_it_is_dragged() {
        let mut switcher = Switcher::new(
            (0..8).map(|w| item(&format!("ws{w}"), "", w)).collect(),
            false,
            Place::Split,
        );
        switcher.set_room(room(5));
        let bar = switcher.view().bar.unwrap();
        assert_eq!((bar.down, bar.total - switcher.list_height()), (0.0, 120.0));
        // Part of a row down, the selection dragged along.
        switcher.stand_at(60.0);
        let view = switcher.view();
        assert_eq!((view.rows[0].label.as_str(), view.part), ("ws1", 20.0));
        assert_eq!((view.selected, view.bar.unwrap().down), (Some(1), 60.0));
        // No further than the end.
        switcher.stand_at(1000.0);
        assert_eq!(labels(&switcher), ["ws3", "ws4", "ws5", "ws6", "ws7"]);
        // With all of them shown, there is none.
        switcher.set_room(room(9));
        assert_eq!(switcher.view().bar, None);
    }

    #[test]
    fn a_touchpad_scrolls_by_parts_of_a_row() {
        let mut switcher = Switcher::new(
            (0..8).map(|w| item(&format!("ws{w}"), "", w)).collect(),
            false,
            Place::Split,
        );
        switcher.set_room(room(5));
        let now = Instant::now();
        switcher.scroll(Motion::Finger { px: 60.0, time: 0 }, now);
        let view = switcher.view();
        assert_eq!(view.part, 20.0);
        // The row shown in part under the list too; the selection is
        // dragged to the first row shown whole.
        assert_eq!(
            labels(&switcher)[..6],
            ["ws1", "ws2", "ws3", "ws4", "ws5", "ws6"]
        );
        assert_eq!(view.selected, Some(1));
        // Pulled on past the end, the rows go up past it, and come back
        // once let go.
        switcher.scroll(Motion::Finger { px: 400.0, time: 8 }, now);
        let view = switcher.view();
        assert_eq!(view.rows[0].label, "ws3");
        assert!(view.part > 0.0 && view.part < 234.0, "{}", view.part);
        settle(&mut switcher, Motion::Lift { time: 16 });
        assert_eq!(switcher.view().part, 0.0);
        assert_eq!(labels(&switcher), ["ws3", "ws4", "ws5", "ws6", "ws7"]);
        // A key moving the list drops the part.
        switcher.scroll(
            Motion::Finger {
                px: -20.0,
                time: 3000,
            },
            now,
        );
        assert_eq!(switcher.view().part, 20.0);
        let none = ModifiersState::default();
        switcher.key(&none, Keysym::Page_Up, Keysym::Page_Up);
        assert_eq!(switcher.view().part, 0.0);
        // As does typing, which starts the list over.
        switcher.scroll(
            Motion::Finger {
                px: 20.0,
                time: 3008,
            },
            now,
        );
        assert_eq!(switcher.view().part, 20.0);
        type_in(&mut switcher, "w");
        assert_eq!(switcher.view().part, 0.0);
    }

    #[test]
    fn new_items_keep_the_selection() {
        let mut switcher = Switcher::new(
            vec![item("a", "", 0), item("b", "", 1)],
            false,
            Place::Split,
        );
        switcher.key(&ModifiersState::default(), Keysym::Down, Keysym::Down);
        switcher.set_items(vec![item("c", "", 2), item("a", "", 0), item("b", "", 1)]);
        assert_eq!(switcher.picked(), Some(&Target::Workspace(1)));
    }

    fn project(id: &str, last_used: u64, workspaces: usize) -> Project {
        let record = crate::projects::WorkspaceRecord {
            id: "w1".into(),
            herdr_id: Some("w1".into()),
            label: id.into(),
            kind: crate::projects::Kind::Project,
            dir: PathBuf::from("/tmp"),
            venv: Default::default(),
            tabs: Vec::new(),
            panes: Vec::new(),
            folded: None,
            restart_choices: Vec::new(),
        };
        Project {
            id: id.into(),
            name: id.to_uppercase(),
            session: format!("gerdr-{id}"),
            created: 0,
            last_used,
            state: Default::default(),
            workspaces: vec![record; workspaces],
            before_start: String::new(),
            after_stop: String::new(),
            tickets: Vec::new(),
        }
    }

    #[test]
    fn lists_the_other_projects_and_the_project_menu() {
        let projects = [project("a", 3, 1), project("b", 1, 2), project("c", 2, 0)];
        let items = project_items(&projects, Some("a"));
        let listed: Vec<_> = items
            .iter()
            .map(|item| (item.label.as_str(), item.detail.as_str(), &item.target))
            .collect();
        assert_eq!(
            listed,
            [
                ("C", "project · 0 workspaces", &Target::Project("c".into())),
                ("B", "project · 2 workspaces", &Target::Project("b".into())),
                ("Projects…", "find, create, manage", &Target::Welcome),
            ]
        );
        let items = project_items(&projects, None);
        assert_eq!(items[0].detail, "project · 1 workspace");
        assert_eq!(items.len(), 4);
    }

    #[test]
    fn run_prompt_opens_an_address_as_a_web_page() {
        let mut switcher = Switcher::new(Vec::new(), true, Place::Split);
        type_in(&mut switcher, "github.com/o/r");
        assert_eq!(labels(&switcher), ["github.com/o/r"]);
        let none = ModifiersState::default();
        assert!(
            matches!(switcher.key(&none, Keysym::Return, Keysym::Return), Outcome::Picked(Target::Command(c), Place::Split) if c == "https://github.com/o/r")
        );
    }

    #[test]
    fn run_prompt_offers_the_typed_command_first() {
        let command =
            |c: &str| Item::new(ItemIcon::App, c, "run before", Target::Command(c.into()));
        let app = Item::new(
            ItemIcon::App,
            "browser",
            "firefox",
            Target::App("firefox".into()),
        );
        let mut switcher = Switcher::new(
            vec![app, command("imv a.png"), command("imv")],
            true,
            Place::Split,
        );
        assert_eq!(labels(&switcher), ["browser", "imv a.png", "imv"]);
        type_in(&mut switcher, "imv");
        // "imv" run before shows once, as what's typed.
        assert_eq!(labels(&switcher), ["imv", "imv a.png"]);
        let none = ModifiersState::default();
        assert!(
            matches!(switcher.key(&none, Keysym::Return, Keysym::Return), Outcome::Picked(Target::Command(c), Place::Split) if c == "imv")
        );
        // Alt+Enter runs it in a new tab instead.
        let alt = ModifiersState {
            alt: true,
            ..Default::default()
        };
        assert!(
            matches!(switcher.key(&alt, Keysym::Return, Keysym::Return), Outcome::Picked(Target::Command(c), Place::Tab) if c == "imv")
        );
        type_in(&mut switcher, " a");
        switcher.key(&none, Keysym::Down, Keysym::Down);
        assert_eq!(labels(&switcher), ["imv a", "imv a.png"]);
        assert_eq!(
            switcher.picked(),
            Some(&Target::Command("imv a.png".into()))
        );
    }

    #[test]
    fn takes_a_paste_and_ctrl_c_never_closes() {
        let mut switcher = Switcher::new(Vec::new(), true, Place::Split);
        let ctrl = ModifiersState {
            ctrl: true,
            ..Default::default()
        };
        assert!(matches!(
            switcher.key(&ctrl, Keysym::c, Keysym::c),
            Outcome::Open
        ));
        type_in(&mut switcher, "echo ");
        let none = ModifiersState::default();
        crate::text_field::pasting("a\tb\nc\n", |sym| switcher.key(&none, sym, sym));
        assert_eq!(&*switcher.query, "echo a b c");
    }

    #[test]
    fn esc_clears_the_query_then_closes() {
        let mut switcher = Switcher::new(
            (0..3).map(|w| item(&format!("ws{w}"), "", w)).collect(),
            false,
            Place::Split,
        );
        let none = ModifiersState::default();
        let esc = |s: &Switcher| s.hints().last().map(|(_, what)| what.clone());
        type_in(&mut switcher, "ws1");
        assert_eq!(esc(&switcher).as_deref(), Some("clear"));
        assert!(matches!(
            switcher.key(&none, Keysym::Escape, Keysym::Escape),
            Outcome::Open
        ));
        assert!(switcher.query.is_empty());
        assert_eq!(labels(&switcher).len(), 3);
        assert_eq!(esc(&switcher).as_deref(), Some("close"));
        assert!(matches!(
            switcher.key(&none, Keysym::Escape, Keysym::Escape),
            Outcome::Closed
        ));
    }

    #[test]
    fn ctrl_j_and_k_move_and_ctrl_n_and_p_do_not() {
        let mut switcher = Switcher::new(
            (0..3).map(|w| item(&format!("ws{w}"), "", w)).collect(),
            false,
            Place::Split,
        );
        let ctrl = ModifiersState {
            ctrl: true,
            ..Default::default()
        };
        switcher.key(&ctrl, Keysym::j, Keysym::j);
        assert_eq!(switcher.view().selected, Some(1));
        switcher.key(&ctrl, Keysym::n, Keysym::n);
        switcher.key(&ctrl, Keysym::p, Keysym::p);
        assert_eq!(switcher.view().selected, Some(1));
        switcher.key(&ctrl, Keysym::k, Keysym::k);
        assert_eq!(switcher.view().selected, Some(0));
    }

    #[test]
    fn run_prompt_in_a_new_tab() {
        let mut switcher = Switcher::new(Vec::new(), true, Place::Tab);
        type_in(&mut switcher, "imv");
        assert_eq!(switcher.view().rows[0].detail, "run in a new tab");
        assert_eq!(switcher.view().hint, "Command to run in a new tab…");
        let none = ModifiersState::default();
        assert!(matches!(
            switcher.key(&none, Keysym::Return, Keysym::Return),
            Outcome::Picked(Target::Command(_), Place::Tab)
        ));
    }

    #[test]
    fn run_prompt_selects_a_named_app() {
        let app = |name: &str, command: &str| {
            Item::new(ItemIcon::App, name, "", Target::App(command.into()))
        };
        let mut switcher = Switcher::new(
            vec![
                app("Files", "nautilus --new-window"),
                app("Firefox", "firefox"),
            ],
            true,
            Place::Split,
        );
        type_in(&mut switcher, "fi");
        // The typed command stays first, under its own heading; the app
        // named with it is what Enter picks.
        assert_eq!(labels(&switcher), ["fi", "Files", "Firefox"]);
        let view = switcher.view();
        assert_eq!(view.rows[0].section, Some("Run"));
        assert_eq!(view.rows[1].section, Some("Apps"));
        assert_eq!(view.rows[2].section, None);
        assert_eq!(view.selected, Some(1));
        type_in(&mut switcher, "l");
        assert_eq!(
            switcher.picked(),
            Some(&Target::App("nautilus --new-window".into()))
        );
        type_in(&mut switcher, "es -x");
        assert_eq!(switcher.picked(), Some(&Target::Command("files -x".into())));
    }

    #[test]
    fn run_prompt_offers_the_project_file() {
        let home = home_dir();
        let home = home.as_deref();
        assert_eq!(
            project_file_items(None, home)
                .iter()
                .map(|item| &item.target)
                .collect::<Vec<_>>(),
            [&Target::ReloadProjects]
        );
        let items = project_file_items(Some("aft"), home);
        assert_eq!(items[0].label, "Project file");
        assert!(
            items[0]
                .detail
                .ends_with("/.local/share/gerdr/projects/aft.json")
        );
        let history = Item::new(
            ItemIcon::App,
            "imv",
            "run before",
            Target::Command("imv".into()),
        );
        let mut switcher = Switcher::new(
            std::iter::once(history).chain(items).collect(),
            true,
            Place::Split,
        );
        assert_eq!(
            labels(&switcher),
            ["imv", "Project file", "Reload project file"]
        );
        // Named, it is what Enter picks, past the typed command.
        type_in(&mut switcher, "proj");
        assert_eq!(labels(&switcher)[0], "proj");
        let none = ModifiersState::default();
        assert!(matches!(
            switcher.key(&none, Keysym::Return, Keysym::Return),
            Outcome::Picked(Target::ProjectFile(id), _) if id == "aft"
        ));
        let ctrl = ModifiersState {
            ctrl: true,
            ..Default::default()
        };
        switcher.key(&ctrl, Keysym::u, Keysym::u);
        type_in(&mut switcher, "reload");
        assert_eq!(switcher.picked(), Some(&Target::ReloadProjects));
    }
}

//! The switcher: everything gerdr can switch to (the workspaces, their tabs,
//! the coding agents, the other projects, the apps named in the config file,
//! and the folders zoxide knows), narrowed down by fuzzy search as the user
//! types. Enter
//! switches to the selected item; a folder no workspace has yet opens as a
//! project, an app launches into a split. zoxide is optional: without it
//! there are just no folders.
//!
//! The same panel is the run prompt: what is typed is a command to launch
//! into a split or a new tab, offered first (second, under an app whose name
//! starts with it), among the apps named in the config file, the commands
//! typed before and the apps installed on the system.
//!
//! Alt+Enter launches the picked app or command in a new tab, wherever
//! Enter would launch it.

use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
    ffi::OsStr,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    reexports::calloop::channel,
    utils::{Logical, Point},
};

use crate::{
    Gerdr, WorkspaceKind,
    app_icons::AppIcon,
    chrome::{self, ItemIcon, SwitcherRow, SwitcherView, status_label},
    fuzzy, home_dir,
    launch::{self, Place},
    paths::tilde,
    projects::Project,
    text_field::{Edit, TextField},
};

/// What an item switches to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Workspace(usize),
    /// A tab of a workspace.
    Tab(usize, usize),
    /// The agent in a herdr pane.
    Agent(String),
    /// Another project, by id.
    Project(String),
    /// The project menu, to find, start or manage projects there (the
    /// welcome screen while no project is shown).
    Welcome,
    /// A folder, opened as a project unless a workspace has it already.
    Folder(PathBuf),
    /// The command of an app named in the config file or installed, to
    /// launch.
    App(String),
    /// A command typed in the run prompt (now or before), to launch and
    /// remember.
    Command(String),
}

/// One thing to switch to, as listed.
pub struct Item {
    pub icon: ItemIcon,
    pub label: String,
    /// More about it, shown dimmer after the label, and searched too.
    pub detail: String,
    pub target: Target,
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
    /// The first match shown.
    scroll: usize,
    /// The wheel's turn not yet a whole row, in rows.
    wheel: f64,
    /// The installed apps' icons by program, for the typed command.
    icons: HashMap<String, AppIcon>,
}

impl Switcher {
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
            scroll: 0,
            wheel: 0.0,
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
        self.items = items;
        self.refilter();
        if let Some(i) = picked.and_then(|target| {
            self.matches
                .iter()
                .position(|m| self.items[m.item].target == target)
        }) {
            self.selected = i;
        }
    }

    /// Match the items against the query again, and select the best: in
    /// the run prompt, the typed command, unless the best is an app whose
    /// name starts with it.
    fn refilter(&mut self) {
        let command = self.query.trim().to_owned();
        let typed = self.run.then(|| Target::Command(command.clone()));
        if self.run {
            self.items[0] = typed_command(&command, &self.icons, self.place);
        }
        let mut found: Vec<(i32, Match)> = self
            .items
            .iter()
            .enumerate()
            .skip(usize::from(self.run))
            // The typed command, run before, is listed once: first.
            .filter(|(_, item)| Some(&item.target) != typed.as_ref())
            .filter_map(|(i, item)| {
                let (score, hits) =
                    fuzzy::score(&self.query, &format!("{} {}", item.label, item.detail))?;
                let n = item.label.chars().count();
                let (label, detail): (Vec<_>, Vec<_>) = hits.into_iter().partition(|h| *h < n);
                // The query has no whitespace, so nothing lands on the space between.
                let detail = detail.into_iter().map(|h| h - n - 1).collect();
                Some((
                    score,
                    Match {
                        item: i,
                        label,
                        detail,
                    },
                ))
            })
            .collect();
        // A stable sort: equal scores keep the items' order.
        found.sort_by_key(|(score, _)| Reverse(*score));
        self.matches = found.into_iter().map(|(_, m)| m).collect();
        if self.run && !command.is_empty() {
            let named = |m: &Match| {
                let item = &self.items[m.item];
                matches!(item.target, Target::App(_))
                    && item
                        .label
                        .to_lowercase()
                        .starts_with(&command.to_lowercase())
            };
            let at = usize::from(self.matches.first().is_some_and(named));
            self.matches.insert(
                at,
                Match {
                    item: 0,
                    label: Vec::new(),
                    detail: Vec::new(),
                },
            );
        }
        self.selected = 0;
        self.scroll = 0;
    }

    /// Its text field: the query.
    pub fn field_mut(&mut self) -> &mut TextField {
        &mut self.query
    }

    fn picked(&self) -> Option<&Target> {
        self.matches
            .get(self.selected)
            .map(|m| &self.items[m.item].target)
    }

    /// Handle a key: `sym` is the key's own symbol, `typed` the one with
    /// Shift and the layout applied. `page` is how many rows are shown.
    fn key(&mut self, mods: &ModifiersState, sym: Keysym, typed: Keysym, page: usize) -> Outcome {
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
        match sym {
            Keysym::Escape => return Outcome::Closed,
            Keysym::c if ctrl => return Outcome::Closed,
            Keysym::Return | Keysym::KP_Enter => {
                if let Some(target) = self.picked() {
                    let place = if mods.alt { Place::Tab } else { self.place };
                    return Outcome::Picked(target.clone(), place);
                }
            }
            Keysym::Up | Keysym::KP_Up => self.step(-1, page),
            Keysym::Down | Keysym::KP_Down => self.step(1, page),
            Keysym::Tab | Keysym::ISO_Left_Tab => self.step(if mods.shift { -1 } else { 1 }, page),
            Keysym::Page_Up => self.step(-(page as isize), page),
            Keysym::Page_Down => self.step(page as isize, page),
            Keysym::p if ctrl => self.step(-1, page),
            Keysym::n if ctrl => self.step(1, page),
            _ => {}
        }
        Outcome::Open
    }

    /// Move the selection `by` matches, scrolling to keep it shown.
    fn step(&mut self, by: isize, page: usize) {
        let last = self.matches.len().saturating_sub(1) as isize;
        self.selected = (self.selected as isize + by).clamp(0, last) as usize;
        self.scroll = scrolled(self.scroll, self.selected, page);
    }

    /// Scroll the list `rows` matches (down positive), moving the selection
    /// only as far as needed to keep it shown.
    fn scroll_by(&mut self, rows: i32, page: usize) {
        let page = page.max(1);
        let last = self.matches.len().saturating_sub(page);
        let first = scrolled(self.scroll, self.selected, page)
            .saturating_add_signed(rows as isize)
            .min(last);
        let end = (first + page).min(self.matches.len()).max(first + 1);
        self.scroll = first;
        self.selected = self.selected.clamp(first, end - 1);
    }

    /// The rows shown, `page` at most.
    fn view(&self, page: usize) -> SwitcherView {
        let scroll = scrolled(self.scroll, self.selected, page);
        let rows = self.matches[scroll.min(self.matches.len())..]
            .iter()
            .take(page)
            .map(|m| {
                let item = &self.items[m.item];
                SwitcherRow {
                    icon: item.icon.clone(),
                    label: item.label.clone(),
                    detail: item.detail.clone(),
                    label_hits: m.label.clone(),
                    detail_hits: m.detail.clone(),
                }
            })
            .collect();
        let selected = (!self.matches.is_empty()).then(|| self.selected - scroll);
        let hint = match (self.run, self.place) {
            (true, Place::Split) => "Command to run in a split (Alt+Enter: new tab)…",
            (true, Place::Tab) => "Command to run in a new tab…",
            (false, _) => "Workspace, tab, agent, project, app or folder…",
        };
        SwitcherView {
            query: self.query.line(),
            hint,
            rows,
            selected,
        }
    }

    /// The target of shown row `row`.
    fn row_target(&self, row: usize, page: usize) -> Option<&Target> {
        let m = self
            .matches
            .get(scrolled(self.scroll, self.selected, page) + row)?;
        Some(&self.items[m.item].target)
    }
}

fn app_item(app: &launch::App, icons: &HashMap<String, AppIcon>) -> Item {
    let icon = app
        .icon
        .clone()
        .map_or_else(|| command_icon(&app.command, icons), ItemIcon::Picture);
    Item {
        icon,
        label: app.name.clone(),
        detail: app.detail.clone(),
        target: Target::App(app.command.clone()),
    }
}

/// The icon of the installed app `command` starts, if any.
fn command_icon(command: &str, icons: &HashMap<String, AppIcon>) -> ItemIcon {
    launch::program_name(command)
        .and_then(|program| icons.get(&program).cloned())
        .map_or(ItemIcon::App, ItemIcon::Picture)
}

/// The run prompt's item for the typed `command`, to run in `place`.
fn typed_command(command: &str, icons: &HashMap<String, AppIcon>, place: Place) -> Item {
    let detail = match place {
        Place::Split => "run in a split",
        Place::Tab => "run in a new tab",
    };
    Item {
        icon: command_icon(command, icons),
        label: command.into(),
        detail: detail.into(),
        target: Target::Command(command.into()),
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
            Item {
                icon: ItemIcon::Workspace(WorkspaceKind::Project),
                label: project.name.clone(),
                detail: format!("project · {n} workspace{plural}"),
                target: Target::Project(project.id.clone()),
            }
        })
        .collect();
    items.push(Item {
        icon: ItemIcon::Folder,
        label: "Projects…".into(),
        detail: "find, create, manage".into(),
        target: Target::Welcome,
    });
    items
}

/// The first row to show so that `selected` is in the `page` rows shown,
/// moving as little as possible from `scroll`.
pub(crate) fn scrolled(scroll: usize, selected: usize, page: usize) -> usize {
    scroll
        .min(selected)
        .max((selected + 1).saturating_sub(page))
}

impl Gerdr {
    /// Open the switcher, and ask zoxide for its folders again meanwhile.
    pub fn open_switcher(&mut self) {
        self.project_menu = None;
        self.switcher = Some(Switcher::new(self.switcher_items(), false, Place::Split));
        self.query_zoxide();
    }

    /// Open the switcher as the run prompt, for a command to launch in
    /// `place`, and look for installed apps again meanwhile.
    pub fn open_run_prompt(&mut self, place: Place) {
        self.project_menu = None;
        let mut switcher = Switcher::new(Vec::new(), true, place);
        switcher.icons = self.program_icons();
        switcher.set_items(self.run_items());
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

    /// What the run prompt offers: the apps named in the config file, the
    /// commands typed before, then the installed apps.
    fn run_items(&self) -> Vec<Item> {
        let icons = self.program_icons();
        let mut items = self.app_items();
        items.extend(launch::history().into_iter().map(|command| Item {
            icon: command_icon(&command, &icons),
            label: command.clone(),
            detail: "run before".into(),
            target: Target::Command(command),
        }));
        items.extend(self.installed_apps.iter().map(|app| app_item(app, &icons)));
        items
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
        Some(self.switcher.as_ref()?.view(self.switcher_page()))
    }

    /// Bring the open switcher's items up to date.
    pub fn refresh_switcher(&mut self) {
        if let Some(mut switcher) = self.switcher.take_if(|s| !s.run) {
            switcher.set_items(self.switcher_items());
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
        let page = self.switcher_page();
        match self.switcher.as_mut()?.key(mods, sym, typed, page) {
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

    /// The wheel turned `rows` rows (down positive, parts of a row adding
    /// up) over the switcher.
    pub fn switcher_scroll(&mut self, rows: f64) {
        let page = self.switcher_page();
        if let Some(switcher) = self.switcher.as_mut() {
            switcher.wheel += rows;
            let whole = switcher.wheel.trunc();
            switcher.wheel -= whole;
            if whole != 0.0 {
                switcher.scroll_by(whole as i32, page);
            }
        }
    }

    /// A button pressed while the switcher is open: on a row it switches to
    /// that row's target, outside the panel it closes the switcher.
    pub fn switcher_click(&mut self, pos: Point<f64, Logical>) {
        let page = self.switcher_page();
        let Some(switcher) = self.switcher.take() else {
            return;
        };
        let shown = switcher.view(page).rows.len();
        let (panel, _, rows) = chrome::switcher_layout(shown, self.content_rect());
        match rows.iter().position(|row| row.to_f64().contains(pos)) {
            Some(row) => {
                if let Some(target) = switcher.row_target(row, page).cloned() {
                    self.switch_to(target, switcher.place);
                }
            }
            None if panel.to_f64().contains(pos) => self.switcher = Some(switcher),
            None => {}
        }
    }

    /// Switch to `target`; an app or a command launches in `place`.
    pub fn switch_to(&mut self, target: Target, place: Place) {
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
            Target::Agent(pane_id) => self.focus_agent(&pane_id),
            Target::Project(id) => self.switch_project(&id),
            Target::Welcome if self.project.is_some() => {
                if self.project_menu.is_none() {
                    self.toggle_project_menu();
                }
            }
            Target::Welcome => self.open_welcome(true),
            Target::Folder(dir) => match self.workspaces.iter().position(|ws| ws.dir == dir) {
                Some(w) => self.switch_workspace(w),
                None => self.open_project(dir),
            },
            Target::App(command) => self.launch(&command, place),
            Target::Command(command) => {
                launch::remember(&command);
                self.launch(&command, place);
            }
        }
    }

    fn switcher_page(&self) -> usize {
        chrome::switcher_page(self.content_rect())
    }

    /// The apps named in the config file, with the icons of the installed
    /// apps they start.
    fn app_items(&self) -> Vec<Item> {
        let icons = self.program_icons();
        self.apps.iter().map(|app| app_item(app, &icons)).collect()
    }

    /// Everything to switch to: each workspace followed by its tabs (when it
    /// has more than one), the agents, the other projects and the welcome
    /// screen, the apps, then zoxide's folders not open yet.
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
            let target = Target::Workspace(w);
            items.push(Item {
                icon: ItemIcon::Workspace(ws.kind),
                label: ws.name.clone(),
                detail: detail.join(" · "),
                target,
            });
            if tabs.len() > 1 {
                for (t, tab) in tabs.into_iter().enumerate() {
                    let detail = format!("{} · tab {}", ws.name, t + 1);
                    items.push(Item {
                        icon: ItemIcon::Tab,
                        label: tab.title,
                        detail,
                        target: Target::Tab(w, t),
                    });
                }
            }
        }
        for agent in &self.agents {
            let Some((w, _)) = self.herdr_pane_place(&agent.pane_id) else {
                continue;
            };
            let mut detail = vec![
                status_label(agent.agent_status),
                self.workspaces[w].name.as_str(),
            ];
            detail.extend(
                agent
                    .title
                    .as_deref()
                    .map(str::trim)
                    .filter(|t| !t.is_empty()),
            );
            items.push(Item {
                icon: ItemIcon::Agent(agent.agent_status),
                label: agent.label().into(),
                detail: detail.join(" · "),
                target: Target::Agent(agent.pane_id.clone()),
            });
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
            items.push(Item {
                icon: ItemIcon::Folder,
                label,
                detail: tilde(dir, home.as_deref()),
                target: Target::Folder(dir.clone()),
            });
        }
        items
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

    fn item(label: &str, detail: &str, w: usize) -> Item {
        Item {
            icon: ItemIcon::Tab,
            label: label.into(),
            detail: detail.into(),
            target: Target::Workspace(w),
        }
    }

    fn type_in(switcher: &mut Switcher, text: &str) {
        for c in text.chars() {
            let sym = Keysym::from_char(c);
            switcher.key(&ModifiersState::default(), sym, sym, 5);
        }
    }

    fn labels(switcher: &Switcher) -> Vec<String> {
        switcher
            .view(5)
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
        let view = switcher.view(5);
        assert_eq!(view.rows[0].label_hits, [0, 1, 2, 3, 4]);
        assert_eq!(view.rows[1].detail_hits, [0, 1, 2, 3, 4]);
        let ctrl = ModifiersState {
            ctrl: true,
            ..Default::default()
        };
        switcher.key(&ctrl, Keysym::u, Keysym::u, 5);
        assert_eq!(labels(&switcher).len(), 3);
    }

    #[test]
    fn selection_scrolls_and_picks() {
        let mut switcher = Switcher::new(
            (0..8).map(|w| item(&format!("ws{w}"), "", w)).collect(),
            false,
            Place::Split,
        );
        let none = ModifiersState::default();
        for _ in 0..6 {
            switcher.key(&none, Keysym::Down, Keysym::Down, 5);
        }
        let view = switcher.view(5);
        assert_eq!(
            (view.rows[0].label.as_str(), view.selected),
            ("ws2", Some(4))
        );
        assert_eq!(switcher.row_target(0, 5), Some(&Target::Workspace(2)));
        assert!(matches!(
            switcher.key(&none, Keysym::Return, Keysym::Return, 5),
            Outcome::Picked(Target::Workspace(6), Place::Split)
        ));
        type_in(&mut switcher, "xyz");
        assert_eq!(switcher.view(5).selected, None);
        assert!(matches!(
            switcher.key(&none, Keysym::Return, Keysym::Return, 5),
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
        switcher.scroll_by(2, 5);
        let view = switcher.view(5);
        assert_eq!(
            (view.rows[0].label.as_str(), view.selected),
            ("ws2", Some(0))
        );
        switcher.scroll_by(10, 5);
        assert_eq!(switcher.view(5).rows[0].label, "ws3");
        switcher.scroll_by(-10, 5);
        let view = switcher.view(5);
        assert_eq!(
            (view.rows[0].label.as_str(), view.selected),
            ("ws0", Some(3))
        );
        type_in(&mut switcher, "xyz");
        switcher.scroll_by(1, 5);
        assert_eq!(switcher.view(5).selected, None);
    }

    #[test]
    fn new_items_keep_the_selection() {
        let mut switcher = Switcher::new(
            vec![item("a", "", 0), item("b", "", 1)],
            false,
            Place::Split,
        );
        switcher.key(&ModifiersState::default(), Keysym::Down, Keysym::Down, 5);
        switcher.set_items(vec![item("c", "", 2), item("a", "", 0), item("b", "", 1)]);
        assert_eq!(switcher.picked(), Some(&Target::Workspace(1)));
    }

    fn project(id: &str, last_used: u64, workspaces: usize) -> Project {
        let record = crate::projects::WorkspaceRecord {
            herdr_id: "w1".into(),
            label: id.into(),
            kind: crate::projects::Kind::Project,
            dir: PathBuf::from("/tmp"),
            venv: Default::default(),
            tabs: Vec::new(),
            panes: Vec::new(),
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
    fn run_prompt_offers_the_typed_command_first() {
        let command = |c: &str| Item {
            icon: ItemIcon::App,
            label: c.into(),
            detail: "run before".into(),
            target: Target::Command(c.into()),
        };
        let app = Item {
            icon: ItemIcon::App,
            label: "browser".into(),
            detail: "firefox".into(),
            target: Target::App("firefox".into()),
        };
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
            matches!(switcher.key(&none, Keysym::Return, Keysym::Return, 5), Outcome::Picked(Target::Command(c), Place::Split) if c == "imv")
        );
        // Alt+Enter runs it in a new tab instead.
        let alt = ModifiersState {
            alt: true,
            ..Default::default()
        };
        assert!(
            matches!(switcher.key(&alt, Keysym::Return, Keysym::Return, 5), Outcome::Picked(Target::Command(c), Place::Tab) if c == "imv")
        );
        type_in(&mut switcher, " a");
        switcher.key(&none, Keysym::Down, Keysym::Down, 5);
        assert_eq!(labels(&switcher), ["imv a", "imv a.png"]);
        assert_eq!(
            switcher.picked(),
            Some(&Target::Command("imv a.png".into()))
        );
    }

    #[test]
    fn takes_a_paste_and_closes_on_ctrl_c() {
        let mut switcher = Switcher::new(Vec::new(), true, Place::Split);
        let ctrl = ModifiersState {
            ctrl: true,
            ..Default::default()
        };
        assert!(matches!(
            switcher.key(&ctrl, Keysym::c, Keysym::c, 5),
            Outcome::Closed
        ));
        type_in(&mut switcher, "echo ");
        let none = ModifiersState::default();
        crate::text_field::pasting("a\tb\nc\n", |sym| switcher.key(&none, sym, sym, 5));
        assert_eq!(&*switcher.query, "echo a b c");
    }

    #[test]
    fn run_prompt_in_a_new_tab() {
        let mut switcher = Switcher::new(Vec::new(), true, Place::Tab);
        type_in(&mut switcher, "imv");
        assert_eq!(switcher.view(5).rows[0].detail, "run in a new tab");
        assert_eq!(switcher.view(5).hint, "Command to run in a new tab…");
        let none = ModifiersState::default();
        assert!(matches!(
            switcher.key(&none, Keysym::Return, Keysym::Return, 5),
            Outcome::Picked(Target::Command(_), Place::Tab)
        ));
    }

    #[test]
    fn run_prompt_puts_a_named_app_first() {
        let app = |name: &str, command: &str| Item {
            icon: ItemIcon::App,
            label: name.into(),
            detail: String::new(),
            target: Target::App(command.into()),
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
        assert_eq!(labels(&switcher)[..2], ["Files", "fi"]);
        type_in(&mut switcher, "l");
        assert_eq!(
            switcher.picked(),
            Some(&Target::App("nautilus --new-window".into()))
        );
        type_in(&mut switcher, "es -x");
        assert_eq!(switcher.picked(), Some(&Target::Command("files -x".into())));
    }
}

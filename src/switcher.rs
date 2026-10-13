//! The switcher: everything gerdr can switch to (the workspaces, their tabs,
//! the coding agents, the apps named in the config file, and the folders
//! zoxide knows), narrowed down by fuzzy search as the user types. Enter
//! switches to the selected item; a folder no workspace has yet opens as a
//! project, an app launches into a split. zoxide is optional: without it
//! there are just no folders.
//!
//! The same panel is the run prompt: what is typed is a command to launch
//! into a split, offered first (second, under an app whose name starts with
//! it), among the apps named in the config file, the commands typed before
//! and the apps installed on the system.

use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
    ffi::OsStr,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use smithay::{
    input::keyboard::{Keycode, Keysym, ModifiersState},
    reexports::calloop::{
        channel,
        timer::{TimeoutAction, Timer},
    },
    utils::{Logical, Point},
};

use crate::{
    Gerdr, REPEAT_DELAY, REPEAT_RATE,
    chrome::{self, ItemIcon, SwitcherRow, SwitcherView, status_label},
    fuzzy, home_dir,
    icons::AppIcon,
    launch,
};

/// What an item switches to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Workspace(usize),
    /// A tab of a workspace.
    Tab(usize, usize),
    /// The agent in a herdr pane.
    Agent(String),
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
    Picked(Target),
}

pub struct Switcher {
    /// The run prompt: the first item is the query, as a command.
    run: bool,
    query: String,
    items: Vec<Item>,
    /// The items `query` matches, best first.
    matches: Vec<Match>,
    /// The selected match.
    selected: usize,
    /// The first match shown.
    scroll: usize,
    /// The installed apps' icons by program, for the typed command.
    icons: HashMap<String, AppIcon>,
}

impl Switcher {
    fn new(items: Vec<Item>, run: bool) -> Self {
        let mut switcher = Self {
            run,
            query: String::new(),
            items: Vec::new(),
            matches: Vec::new(),
            selected: 0,
            scroll: 0,
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
            items.insert(0, typed_command("", &self.icons));
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
            self.items[0] = typed_command(&command, &self.icons);
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

    fn picked(&self) -> Option<&Target> {
        self.matches
            .get(self.selected)
            .map(|m| &self.items[m.item].target)
    }

    /// Handle a key: `sym` is the key's own symbol, `typed` the one with
    /// Shift and the layout applied. `page` is how many rows are shown.
    fn key(&mut self, mods: &ModifiersState, sym: Keysym, typed: Keysym, page: usize) -> Outcome {
        let ctrl = mods.ctrl;
        match sym {
            Keysym::Escape => return Outcome::Closed,
            Keysym::Return | Keysym::KP_Enter => {
                if let Some(target) = self.picked() {
                    return Outcome::Picked(target.clone());
                }
            }
            Keysym::Up | Keysym::KP_Up => self.step(-1, page),
            Keysym::Down | Keysym::KP_Down => self.step(1, page),
            Keysym::Tab | Keysym::ISO_Left_Tab => self.step(if mods.shift { -1 } else { 1 }, page),
            Keysym::Page_Up => self.step(-(page as isize), page),
            Keysym::Page_Down => self.step(page as isize, page),
            Keysym::p if ctrl => self.step(-1, page),
            Keysym::n if ctrl => self.step(1, page),
            Keysym::BackSpace | Keysym::w if ctrl => {
                let word = self
                    .query
                    .trim_end()
                    .rfind(char::is_whitespace)
                    .map_or(0, |i| i + 1);
                self.edit(|query| query.truncate(word));
            }
            Keysym::u if ctrl => self.edit(String::clear),
            Keysym::BackSpace => self.edit(|query| {
                query.pop();
            }),
            _ => {
                if let Some(c) = typed
                    .key_char()
                    .filter(|c| !c.is_control() && !(ctrl || mods.alt || mods.logo))
                {
                    self.edit(|query| query.push(c));
                }
            }
        }
        Outcome::Open
    }

    fn edit(&mut self, change: impl FnOnce(&mut String)) {
        let before = self.query.clone();
        change(&mut self.query);
        if self.query != before {
            self.refilter();
        }
    }

    /// Move the selection `by` matches, scrolling to keep it shown.
    fn step(&mut self, by: isize, page: usize) {
        let last = self.matches.len().saturating_sub(1) as isize;
        self.selected = (self.selected as isize + by).clamp(0, last) as usize;
        self.scroll = scrolled(self.scroll, self.selected, page);
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
        let hint = if self.run {
            "Command to run in a split…"
        } else {
            "Workspace, tab, agent, app or folder…"
        };
        SwitcherView {
            query: self.query.clone(),
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

/// The run prompt's item for the typed `command`.
fn typed_command(command: &str, icons: &HashMap<String, AppIcon>) -> Item {
    Item {
        icon: command_icon(command, icons),
        label: command.into(),
        detail: "run in a split".into(),
        target: Target::Command(command.into()),
    }
}

/// The first row to show so that `selected` is in the `page` rows shown,
/// moving as little as possible from `scroll`.
fn scrolled(scroll: usize, selected: usize, page: usize) -> usize {
    scroll
        .min(selected)
        .max((selected + 1).saturating_sub(page))
}

impl Gerdr {
    /// Open the switcher, and ask zoxide for its folders again meanwhile.
    pub fn open_switcher(&mut self) {
        self.switcher = Some(Switcher::new(self.switcher_items(), false));
        self.query_zoxide();
    }

    /// Open the switcher as the run prompt, for a command to launch, and
    /// look for installed apps again meanwhile.
    pub fn open_run_prompt(&mut self) {
        let mut switcher = Switcher::new(Vec::new(), true);
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
    /// returns what Enter picked.
    pub fn switcher_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<Target> {
        let page = self.switcher_page();
        match self.switcher.as_mut()?.key(mods, sym, typed, page) {
            Outcome::Open => None,
            Outcome::Closed => {
                self.switcher = None;
                None
            }
            Outcome::Picked(target) => {
                self.switcher = None;
                Some(target)
            }
        }
    }

    /// Repeats a key held in the switcher, as long as it is held, the way the
    /// apps' keyboards do: the host sends gerdr only the press and the release.
    /// Esc and Enter, which close the switcher, don't repeat.
    pub fn repeat_switcher_key(
        &mut self,
        code: Keycode,
        mods: ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) {
        self.stop_switcher_repeat();
        if self.switcher.is_none()
            || matches!(sym, Keysym::Escape | Keysym::Return | Keysym::KP_Enter)
        {
            return;
        }
        let interval = Duration::from_millis(1000 / u64::from(REPEAT_RATE));
        let timer = Timer::from_duration(Duration::from_millis(REPEAT_DELAY.into()));
        let repeat = self.loop_handle.insert_source(timer, move |_, _, state| {
            if state.switcher.is_none() {
                state.switcher_repeat = None;
                return TimeoutAction::Drop;
            }
            state.switcher_key(&mods, sym, typed);
            TimeoutAction::ToDuration(interval)
        });
        self.switcher_repeat = repeat.ok().map(|token| (code, token));
    }

    /// Stops repeating the key held in the switcher.
    pub fn stop_switcher_repeat(&mut self) {
        if let Some((_, token)) = self.switcher_repeat.take() {
            self.loop_handle.remove(token);
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
                    self.switch_to(target);
                }
            }
            None if panel.to_f64().contains(pos) => self.switcher = Some(switcher),
            None => {}
        }
    }

    pub fn switch_to(&mut self, target: Target) {
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
            Target::Folder(dir) => match self.workspaces.iter().position(|ws| ws.dir == dir) {
                Some(w) => self.switch_workspace(w),
                None => self.open_project(dir),
            },
            Target::App(command) => self.launch(&command),
            Target::Command(command) => {
                launch::remember(&command);
                self.launch(&command);
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
    /// has more than one), the agents, the apps, then zoxide's folders not
    /// open yet.
    fn switcher_items(&self) -> Vec<Item> {
        let mut items = Vec::new();
        for (w, (ws, (view, tabs))) in self
            .workspaces
            .iter()
            .zip(self.workspace_views())
            .enumerate()
        {
            let detail: Vec<String> = [Some(tilde(&ws.dir)), view.branch]
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
                detail: tilde(dir),
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

    fn query_zoxide(&self) {
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

/// `dir` with the home folder as "~".
fn tilde(dir: &Path) -> String {
    match home_dir().and_then(|home| Some(dir.strip_prefix(home).ok()?.to_path_buf())) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => dir.display().to_string(),
    }
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
            Outcome::Picked(Target::Workspace(6))
        ));
        type_in(&mut switcher, "xyz");
        assert_eq!(switcher.view(5).selected, None);
        assert!(matches!(
            switcher.key(&none, Keysym::Return, Keysym::Return, 5),
            Outcome::Open
        ));
    }

    #[test]
    fn new_items_keep_the_selection() {
        let mut switcher = Switcher::new(vec![item("a", "", 0), item("b", "", 1)], false);
        switcher.key(&ModifiersState::default(), Keysym::Down, Keysym::Down, 5);
        switcher.set_items(vec![item("c", "", 2), item("a", "", 0), item("b", "", 1)]);
        assert_eq!(switcher.picked(), Some(&Target::Workspace(1)));
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
        let mut switcher = Switcher::new(vec![app, command("imv a.png"), command("imv")], true);
        assert_eq!(labels(&switcher), ["browser", "imv a.png", "imv"]);
        type_in(&mut switcher, "imv");
        // "imv" run before shows once, as what's typed.
        assert_eq!(labels(&switcher), ["imv", "imv a.png"]);
        let none = ModifiersState::default();
        assert!(
            matches!(switcher.key(&none, Keysym::Return, Keysym::Return, 5), Outcome::Picked(Target::Command(c)) if c == "imv")
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

//! Apps launched into a split: any command, run by `sh` in the active
//! workspace's folder. The focused pane is split along its longer side, and
//! the first window from the command's processes takes the new half. The
//! apps named in the config file are offered in the switcher and the run
//! prompt; the run prompt also offers the apps installed on the system (see
//! `desktop`) and the commands typed before, and takes any command, or a
//! URL to open in the app for it (see `desktop::url_command`). A web page
//! opens like an installed web app, in a window of its own without tabs or
//! an address bar (see `web_app`).
//!
//! Browsers and VS Code's kin keep one copy running per profile and hand new
//! windows to it, so one already open on the desktop would show the window
//! there. gerdr runs them with a profile of their own instead (see
//! `own_instance`), kept between runs: logins, bookmarks and settings there
//! are apart from the desktop's. A Firefox locks its profile, so gerdr's
//! runs under a name of its own, and the next ones hand their window to it:
//! that window is told by its app_id. A project runs one VS Code of a kind:
//! starting it again shows the one running (see `running_vs_code`).
//! Browsers, VS Codes and Zed get a profile per project, so another
//! gerdr's doesn't take their windows either (see `instance_dir`).
//!
//! Ghostty and Alacritty can open all their windows from one process.
//! gerdr keeps one of each for the project shown, apart from the desktop's
//! (see `Shared`): the first one started becomes it, and the next ones open
//! their window in it. Ghostty's take the folder, `-e` command and `--title`
//! from the command but no other options. Like Zed's, those windows, a
//! Firefox's and a web app's are told apart by app_id only, so each kind is
//! started one at a time (see `launch_next_apps`).
//!
//! Launched apps are not kept in herdr: they end with gerdr.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use smithay::reexports::calloop::channel;

use crate::{
    Expected, Gerdr, Tab, Workspace, WorkspaceKind, app_icons::AppIcon, cli_agents, desktop,
    home_dir, hooks, keyring, layout, state_dir, web_control,
};

/// The start of the class of the Ghostty gerdr keeps (see `Gerdr::shared_name`),
/// to tell its windows by.
pub const GHOSTTY_APP_ID: &str = "dev.gerdr.ghostty";

/// The class of the Alacritty gerdr keeps, to tell its windows by. Without
/// "alacritty" in it: herdr's terminals are expected by that.
pub const ALACRITTY_APP_ID: &str = "dev.gerdr.terminal";

/// The start of the app_id gerdr's Firefox windows are told by (and of the
/// name it hands windows by), followed by the browser's program.
const BROWSER_APP_ID: &str = "dev.gerdr.browser";

/// The program of Cursor, which a project keeps a workspace of its own for
/// (see `Gerdr::open_cursor`).
const CURSOR: &str = "cursor";

/// The name of the Cursor workspace.
const CURSOR_WORKSPACE: &str = "Cursor";

/// The link Cursor opens a chat by, its id following.
const CHAT_LINK: &str = "cursor://anysphere.cursor-deeplink/background-agent?bcId=";

/// How many typed commands the run prompt remembers.
const HISTORY_LEN: usize = 100;

/// Where a launched app's window goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// A split of the focused pane.
    Split,
    /// A tab of its own.
    Tab,
}

/// What `Gerdr::start_app` runs for a command (see `Gerdr::prepare`).
struct Prepared {
    /// The command, for `sh`.
    command: String,
    /// The app_id of a window it hands to another process (see `Expected`);
    /// empty when it doesn't.
    app_id: String,
    /// The folder of the profile of gerdr's it runs with, to make first.
    profile: Option<PathBuf>,
}

/// An app named in the config file, or installed on the system.
#[derive(Clone, Debug)]
pub struct App {
    pub name: String,
    pub command: String,
    /// What the switcher shows after the name.
    pub detail: String,
    pub icon: Option<AppIcon>,
}

impl Gerdr {
    /// Run `command` and show its window in a split of the focused pane,
    /// or in a new tab.
    pub fn launch(&mut self, command: &str, place: Place) {
        if command.trim().is_empty() {
            return;
        }
        if let Some((w, t, leaf)) = self.running_vs_code(command) {
            return self.show_leaf(w, t, leaf);
        }
        // Cursor goes to the project's workspace of its own.
        if program_name(command).as_deref() == Some(CURSOR) {
            return self.open_cursor(Some(command));
        }
        if self.workspaces.is_empty() {
            let ws = self.new_workspace(home_dir().unwrap_or_default(), WorkspaceKind::Terminal);
            self.workspaces.push(ws);
            self.active_workspace = 0;
        }
        let w = self.active_workspace;
        let ws = &mut self.workspaces[w];
        let before = ws.active_tab;
        // The feature panel's tab has no panes to split: a tab of its own.
        let pinned = ws.tabs.get(ws.active_tab).is_some_and(|t| t.pinned);
        if ws.tabs.is_empty() || place == Place::Tab || pinned {
            ws.tabs.push(Tab::default());
            ws.active_tab = ws.tabs.len() - 1;
        }
        let t = ws.active_tab;
        match self.launch_into(w, t, None, command) {
            Some(_) => self.show_active(),
            // The new tab is gone again: back to the one shown before.
            None if place == Place::Tab || pinned => self.switch_tab(before),
            None => {}
        }
    }

    /// Run `command` in workspace `w`'s folder and show its window in tab
    /// `t`: in `leaf`, a leaf already in the tab's layout (as in a restored
    /// tab), or else in a split of the tab's focused pane. Returns the
    /// window's leaf, or `None` if the command didn't start.
    ///
    /// The pane remembers `command` as typed, so the app comes back with the
    /// project. A new split takes focus when its window shows up; a given
    /// leaf doesn't, and nothing is shown: that is up to the caller. When
    /// the command doesn't start, a given leaf is taken out of the layout
    /// and the caller prunes, so the workspace and tab indices it holds stay
    /// put; without one, empty tabs are pruned right away.
    ///
    /// An app whose window is told by app_id (see `Expected`) is queued,
    /// and started when the one before it showed up, or never will (see
    /// `launch_next_apps`).
    pub fn launch_into(
        &mut self,
        w: usize,
        t: usize,
        leaf: Option<u32>,
        command: &str,
    ) -> Option<u32> {
        let original = command.trim();
        let ws = self.workspaces.get(w)?;
        ws.tabs.get(t)?;
        // A restored leaf for a second VS Code of the project goes (see
        // `running_vs_code`).
        if let Some(id) = leaf
            && self.running_vs_code(original).is_some()
        {
            layout::remove(&mut self.workspaces[w].tabs[t].layout, id);
            return None;
        }
        let ws = &self.workspaces[w];
        // Told apart by app_id, it starts in its turn for its leaf (see
        // `launch_next_apps`): a web page then through gerdr's extension,
        // whose windows are named after it, not the site (see `steered`).
        // Only that there is a leaf counts here, not which.
        let app_id = self
            .prepare(original, &ws.dir, Some(leaf.unwrap_or_default()))
            .app_id;
        let queued = !app_id.is_empty();
        // A restored leaf shows the app its project kept running, if any
        // (see `keep`).
        let kept = leaf
            .filter(|_| !queued && self.keeps(original))
            .and_then(|_| self.take_kept(original));
        // An app keeping its logins in the keyring waits for it to be
        // open (see `keyring`).
        let held = !queued && kept.is_none() && uses_keyring(original) && !keyring::state().ready();
        let pid = if queued || held {
            None
        } else if kept.is_some() {
            kept
        } else {
            match self.start_app(w, t, original, None) {
                Some(pid) => Some(pid),
                None => {
                    match leaf {
                        Some(id) => {
                            layout::remove(&mut self.workspaces[w].tabs[t].layout, id);
                        }
                        None => self.prune(),
                    }
                    return None;
                }
            }
        };
        let id = leaf.unwrap_or_else(|| self.new_split(w, t));
        let mut expected = Expected::new(pid, &app_id, id, leaf.is_none());
        expected.app_command = Some(original.to_owned());
        expected.kept = self.is_kept(pid);
        expected.held = held;
        self.workspaces[w].tabs[t].expected.push(expected);
        if held {
            self.wait_for_keyring();
            return Some(id);
        }
        if !queued {
            return Some(id);
        }
        let dropped = self.launch_next_apps();
        // Gone again when it didn't start: `launch_next_apps` took its leaf
        // out of the layout.
        let waiting = self.workspaces[w].tabs[t]
            .expected
            .iter()
            .any(|e| e.pane == id);
        if dropped && leaf.is_none() {
            self.prune();
        }
        waiting.then_some(id)
    }

    /// Where the shown project already runs the VS Code `command` starts
    /// (see `vs_code_of`), or waits for its window: its workspace, tab and
    /// leaf. A second one would only open an editor window of the same copy
    /// (for Cursor, not its Agents window), so a project keeps one. Another
    /// window of a kept one counts once shown (see `Pane::window_of`), after
    /// its own window.
    fn running_vs_code(&self, command: &str) -> Option<(usize, usize, u32)> {
        let key = vs_code_of(command)?;
        let same = |c: Option<&str>| c.and_then(vs_code_of).as_ref() == Some(&key);
        let find = |leaf: &dyn Fn(&Tab) -> Option<u32>| {
            self.workspaces.iter().enumerate().find_map(|(w, ws)| {
                ws.tabs
                    .iter()
                    .enumerate()
                    .find_map(|(t, tab)| Some((w, t, leaf(tab)?)))
            })
        };
        let pane = |tab: &Tab, of: &dyn Fn(&crate::Pane) -> Option<&str>| {
            tab.panes.iter().find(|p| same(of(p))).map(|p| p.id)
        };
        find(&|tab| {
            pane(tab, &|p| p.app_command.as_deref()).or_else(|| {
                tab.expected
                    .iter()
                    .find(|e| same(e.app_command.as_deref()))
                    .map(|e| e.pane)
            })
        })
        .or_else(|| find(&|tab| pane(tab, &|p| p.window_of.as_deref())))
    }

    /// Show the shown project's Cursor, else start `command` (the installed
    /// Cursor without one) in a new tab of its Cursor workspace, else in a
    /// new one called "Cursor", in the project's folder. The workspace
    /// takes splits and tabs like any other.
    pub fn open_cursor(&mut self, command: Option<&str>) {
        let command = command
            .map(str::to_owned)
            .or_else(|| self.installed_cursor().map(|app| app.command.clone()))
            .unwrap_or_else(|| CURSOR.to_owned());
        if let Some((w, t, leaf)) = self.running_vs_code(&command) {
            return self.show_leaf(w, t, leaf);
        }
        let (w, existing) = self.cursor_workspace();
        let record = self.workspaces[w].record.clone();
        let ws = &mut self.workspaces[w];
        ws.tabs.push(Tab::default());
        ws.active_tab = ws.tabs.len() - 1;
        self.active_workspace = w;
        let t = ws.active_tab;
        if self.launch_into(w, t, None, &command).is_none()
            && !existing
            && self.workspaces.iter().all(|ws| ws.record != record)
        {
            self.forget_workspace(&record);
        }
        self.show_active();
    }

    /// The project's Cursor workspace, made in the project's folder if it
    /// has none, and whether it was there already. One kept in the records
    /// (Cursor quit, or its windows haven't come back since a start) comes
    /// back in its place in the sidebar.
    fn cursor_workspace(&mut self) -> (usize, bool) {
        if let Some(w) = self
            .workspaces
            .iter()
            .position(|ws| ws.kind == WorkspaceKind::Cursor)
        {
            return (w, true);
        }
        let kept = self
            .project
            .as_deref()
            .and_then(|id| self.projects.project(id))
            .and_then(|p| p.workspaces.iter().find(|r| r.keeps_place()))
            .cloned();
        if let Some(record) = kept {
            let mut ws = Workspace::new(record.dir, WorkspaceKind::Cursor);
            ws.name = if record.label.is_empty() {
                CURSOR_WORKSPACE.to_owned()
            } else {
                record.label
            };
            ws.record = record.id;
            // Kept already: a launch that fails leaves the record be.
            return (self.insert_workspace(ws), true);
        }
        let dir = self
            .project
            .as_deref()
            .and_then(|id| self.projects.project(id))
            .map(hooks::project_dir)
            .or_else(home_dir)
            .unwrap_or_default();
        let mut ws = Workspace::new(dir, WorkspaceKind::Cursor);
        ws.name = CURSOR_WORKSPACE.to_owned();
        ws.record = self.remember_workspace(&ws.name, ws.kind, &ws.dir, None);
        self.workspaces.push(ws);
        (self.workspaces.len() - 1, false)
    }

    /// The workspace and tab for another window of the kept app `command`
    /// that no leaf waits for (see `Pane::window_of`), such as the Agents
    /// window a chat link opens: beside a window of the app shown, else,
    /// for Cursor, a new tab of its workspace (see `open_cursor`). `None`
    /// for the shown tab.
    pub(crate) fn kept_window_place(&mut self, command: &str) -> Option<(usize, usize)> {
        let program = program_name(command)?;
        let of_app = |c: Option<&str>| c.and_then(program_name).as_ref() == Some(&program);
        let own = |p: &crate::Pane| of_app(p.app_command.as_deref());
        let other = |p: &crate::Pane| of_app(p.window_of.as_deref());
        let find = |pred: &dyn Fn(&crate::Pane) -> bool, cursor: bool| {
            self.workspaces.iter().enumerate().find_map(|(w, ws)| {
                if cursor && ws.kind != WorkspaceKind::Cursor {
                    return None;
                }
                let t = ws.tabs.iter().position(|tab| tab.panes.iter().any(pred))?;
                Some((w, t))
            })
        };
        if let Some(place) = find(&own, false) {
            return Some(place);
        }
        if program != CURSOR {
            return find(&other, false);
        }
        if let Some(place) = find(&other, true) {
            return Some(place);
        }
        let (w, _) = self.cursor_workspace();
        let ws = &mut self.workspaces[w];
        ws.tabs.push(Tab::default());
        ws.active_tab = ws.tabs.len() - 1;
        Some((w, ws.active_tab))
    }

    /// Start `agent` in workspace `w`: a CLI in a new tab, T3 Code as a
    /// new thread in its app's pane.
    pub fn start_agent(&mut self, w: usize, agent: &cli_agents::CliAgent) {
        match &agent.start {
            cli_agents::Start::Command(command) => self.new_agent(w, command),
            cli_agents::Start::T3 => self.new_t3_thread(w),
        }
    }

    /// Start an agent CLI by `command` in a new tab of workspace `w`, in
    /// its folder: herdr shows it as an agent like any other.
    pub fn new_agent(&mut self, w: usize, command: &str) {
        let Some(dir) = self.workspaces.get(w).map(|ws| ws.dir.clone()) else {
            return;
        };
        self.new_tab_running(w, &dir, command.to_owned());
    }

    /// Look for the installed agent CLIs again (see `cli_agents`).
    pub fn refresh_cli_agents(&mut self) {
        self.cli_agents = cli_agents::installed(&self.agent_config);
    }

    /// Show Cursor chat `id`: the project's Cursor, started if it isn't
    /// running, and in a running one the chat, by a link Cursor opens (see
    /// `CHAT_LINK`).
    pub fn focus_cursor_chat(&mut self, id: &str) {
        let command = self
            .installed_cursor()
            .map(|app| app.command.clone())
            .unwrap_or_else(|| CURSOR.to_owned());
        let running = self
            .running_vs_code(&command)
            .and_then(|(w, t, leaf)| self.leaf_pid(w, t, leaf));
        self.open_cursor(Some(&command));
        self.cursor_opened = Some(id.to_owned());
        // One just started would take the link as its own start. To the
        // one running, by its own profile: by another, the link would
        // start a second Cursor, which gerdr doesn't keep.
        let Some(own) = running.and_then(|pid| with_profile_of(&command, pid)) else {
            return;
        };
        let link = format!("{CHAT_LINK}{id}");
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(format!("{own} --open-url {}", desktop::shell_quote(&link)))
            .env("ELECTRON_OZONE_PLATFORM_HINT", "auto");
        let dir = home_dir().unwrap_or_default();
        self.spawn(cmd, &dir, CURSOR);
    }

    /// The pid of the app shown in leaf `leaf` of tab `t` of workspace `w`,
    /// or started for it: for a window of an app kept running, the app's
    /// (its own client's is the server's client).
    fn leaf_pid(&self, w: usize, t: usize, leaf: u32) -> Option<u32> {
        let tab = self.workspaces.get(w)?.tabs.get(t)?;
        match tab.panes.iter().find(|p| p.id == leaf) {
            Some(pane) if self.is_kept_window(&pane.window) => {
                self.kept_pid(pane.app_command.as_deref().or(pane.window_of.as_deref())?)
            }
            Some(pane) => self.window_pid(&pane.window),
            None => tab.expected.iter().find(|e| e.pane == leaf)?.pid,
        }
    }

    /// The installed Cursor, if any.
    fn installed_cursor(&self) -> Option<&App> {
        self.installed_apps
            .iter()
            .find(|app| program_name(&app.command).as_deref() == Some(CURSOR))
    }

    /// The installed Cursor's icon, for its workspace's row.
    pub fn cursor_icon(&self) -> Option<AppIcon> {
        self.installed_cursor()?.icon.clone()
    }

    /// Show tab `t` of workspace `w` with the pane in `leaf` focused, or
    /// its placeholder while the window is on its way.
    pub(crate) fn show_leaf(&mut self, w: usize, t: usize, leaf: u32) {
        self.active_workspace = w;
        let ws = &mut self.workspaces[w];
        ws.active_tab = t;
        let tab = &mut ws.tabs[t];
        if let Some(window) = tab.window(leaf).cloned() {
            tab.focused = Some(window);
        }
        self.show_active();
    }

    /// Start the first queued window of each app whose windows are told by
    /// app_id (gerdr's Ghostty, Alacritty and Firefox, a web app's site),
    /// unless one is already on its way (see `launch_next`). A Zed's waits
    /// for an old Zed too (see `Gerdr::launch_next_zed`). One that doesn't
    /// start leaves its leaf, and the next is tried: whether any did, for
    /// the caller to prune.
    pub fn launch_next_apps(&mut self) -> bool {
        let mut app_ids: Vec<String> = self
            .workspaces
            .iter()
            .flat_map(|ws| &ws.tabs)
            .flat_map(|tab| &tab.expected)
            .filter(|e| e.pid.is_none() && e.app_command.is_some() && !e.app_id.is_empty())
            .map(|e| e.app_id.clone())
            .collect();
        app_ids.sort();
        app_ids.dedup();
        let mut dropped = false;
        for app_id in app_ids {
            if app_id == ZED_APP_ID {
                dropped |= self.launch_next_zed();
                continue;
            }
            dropped |= self.launch_next(&app_id, |gerdr, w, t, i| {
                let e = &gerdr.workspaces[w].tabs[t].expected[i];
                let (command, pane) = (e.app_command.clone().unwrap_or_default(), e.pane);
                gerdr.start_app(w, t, &command, Some(pane))
            });
        }
        dropped
    }

    /// What names the Ghostty and Alacritty gerdr keeps: apart from other
    /// gerdrs', and from those of the project switched away from, still on
    /// their way out (see `retire_shared`).
    fn shared_name(&self) -> String {
        let mut socket: String = self
            .socket_name
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        // No part of a class may start with a digit.
        if socket.starts_with(|c: char| c.is_ascii_digit()) {
            socket.insert(0, '_');
        }
        format!("{socket}_{}", self.shared_generation)
    }

    /// The socket of the Alacritty gerdr keeps, to hand windows to it by.
    fn alacritty_socket(&self) -> PathBuf {
        std::env::var_os("XDG_RUNTIME_DIR")
            .map_or_else(std::env::temp_dir, PathBuf::from)
            .join(format!("gerdr-alacritty-{}.sock", self.shared_name()))
    }

    /// Start a new generation of the Ghostty and Alacritty gerdr keeps (see
    /// `shared_name`): the ones running are the old project's, on their way
    /// out. The old Alacritty's socket goes, so nothing more is handed to it
    /// and it doesn't stay behind (ended by a signal, it leaves it).
    pub fn retire_shared(&mut self) {
        let socket = self.alacritty_socket();
        if let Err(err) = std::fs::remove_file(&socket)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            tracing::debug!("removing {}: {err}", socket.display());
        }
        self.shared_generation += 1;
    }

    /// `command` (trimmed) as `start_app` runs it in `dir`: in gerdr's
    /// Ghostty or Alacritty, in the app for a URL, as a web app (through
    /// gerdr's extension for `pane`, see `steered`), or with a profile of
    /// gerdr's (see the module), which for Zed is the data dir of gerdr's
    /// Zed (see `Gerdr::launch_next_zed`).
    fn prepare(&self, command: &str, dir: &Path, pane: Option<u32>) -> Prepared {
        if let Some(shared) = Shared::of(command) {
            let changed = match shared {
                Shared::Ghostty => {
                    let class = format!("{GHOSTTY_APP_ID}.{}", self.shared_name());
                    ghostty_command(command, &class, dir)
                }
                Shared::Alacritty => alacritty_command(
                    command,
                    &self.alacritty_socket(),
                    dir,
                    &self.theme().palette().alacritty_options(),
                ),
            };
            if let Some(command) = changed {
                return Prepared {
                    command,
                    app_id: shared.app_id().to_owned(),
                    profile: None,
                };
            }
        }
        // A URL opens in the app for it, like a browser; a web page in a
        // window of its own.
        let url = desktop::url_command(command);
        let web = url.as_deref().and_then(|opener| web_app(command, opener));
        let mut app_id = String::new();
        let command = match web {
            Some((web, site)) => match self.steered(&web, command, pane) {
                Some(steered) => {
                    app_id = format!("-{}_", web_control::EXTENSION_ID);
                    steered
                }
                // The extension's own pages show nothing without it: no
                // command, which doesn't start (see `steers`).
                None if site == format!("-{}_", web_control::EXTENSION_ID) => {
                    app_id = site;
                    String::new()
                }
                None => {
                    app_id = site;
                    web
                }
            },
            None => url.unwrap_or_else(|| command.to_owned()),
        };
        let bare_zed = opens_nothing_in_zed(&command);
        match own_instance(&command, Some(&self.instance_dir)) {
            Some((mut command, profile)) => {
                if let Some(profile) = &profile {
                    self.ready_profile(&command, profile);
                }
                if command.contains(BROWSER_APP_ID) {
                    app_id = BROWSER_APP_ID.to_owned();
                } else if program_name(&command).is_some_and(|key| ZEDS.contains(&key.as_str())) {
                    // The CLI hands the window to gerdr's editor and quits.
                    app_id = ZED_APP_ID.to_owned();
                    // With nothing to open, Zed brings back the windows of
                    // its last session, which gerdr takes for the ones it
                    // waits for: open the workspace's folder instead.
                    if bare_zed {
                        command.push_str(" -n ");
                        command.push_str(&desktop::shell_quote(&dir.to_string_lossy()));
                    }
                }
                Prepared {
                    command,
                    app_id,
                    profile,
                }
            }
            None => Prepared {
                command,
                app_id,
                profile: None,
            },
        }
    }

    /// The web app command `web` (see `web_app`) changed to open `url`
    /// through gerdr's extension for `pane`, written into the browser's
    /// profile of gerdr's first (see `web_control`). The app_id of its
    /// windows names the extension then, not the site. `None` for a
    /// browser not known to take the extension, or a Flatpak one, which
    /// can't run its host.
    fn steered(&self, web: &str, url: &str, pane: Option<u32>) -> Option<String> {
        let pane = pane?;
        let socket = self.web_control.socket()?;
        let (key, _, flatpak) = program(web)?;
        if flatpak || !STEERED.contains(&key.as_str()) {
            return None;
        }
        let profile = own_instance(web, Some(&self.instance_dir))?.1?;
        // Before the extension goes in, which would count as its start.
        self.ready_profile(web, &profile);
        let extension = web_control::install(&profile, socket)
            .inspect_err(|err| tracing::warn!("gerdr's extension in {}: {err}", profile.display()))
            .ok()?;
        let app = format!("--app={}", desktop::shell_quote(url.trim()));
        let boot = format!(
            "--load-extension={} --app={}",
            desktop::shell_quote(&extension.to_string_lossy()),
            desktop::shell_quote(&web_control::boot_url(pane, url.trim()))
        );
        web.contains(&app).then(|| web.replacen(&app, &boot, 1))
    }

    /// Whether `command`, run in folder `dir`, opens through gerdr's
    /// extension (see `steered`): one of its own pages needs it.
    pub(crate) fn steers(&self, command: &str, dir: &Path) -> bool {
        let app_id = self.prepare(command, dir, Some(0)).app_id;
        app_id == format!("-{}_", web_control::EXTENSION_ID)
    }

    /// Make `profile`, the one `command` runs with (see `own_instance`),
    /// ready when it is the project's: not a Flatpak app's, which sees
    /// only its own folder (see `ready_vs_code_profile` and
    /// `seed_browser_profile`).
    fn ready_profile(&self, command: &str, profile: &Path) {
        let (Some(key), Some(home)) = (program_name(command), home_dir()) else {
            return;
        };
        if !profile.starts_with(&self.instance_dir) {
            return;
        }
        if VS_CODES.contains(&key.as_str()) {
            ready_vs_code_profile(&home, &key, profile);
        } else if CHROMIUMS.contains(&key.as_str()) || FIREFOXES.contains(&key.as_str()) {
            seed_browser_profile(&home, &key, profile);
            if CHROMIUMS.contains(&key.as_str()) {
                leave_out_web_apps(profile);
            }
        }
    }

    /// Start `command` (trimmed) for tab `t` of workspace `w`, in the
    /// workspace's folder (see `prepare`), showing in `pane` when known.
    /// Its pid, or `None` if it is empty, the tab is gone, or it didn't
    /// start.
    pub(crate) fn start_app(
        &mut self,
        w: usize,
        t: usize,
        command: &str,
        pane: Option<u32>,
    ) -> Option<u32> {
        if command.is_empty() {
            return None;
        }
        let ws = self.workspaces.get(w)?;
        ws.tabs.get(t)?;
        let dir = ws.dir.clone();
        let prepared = self.prepare(command, &dir, pane);
        if prepared.command.is_empty() {
            tracing::warn!("{command}: nothing to run it with");
            return None;
        }
        if let Some(profile) = &prepared.profile
            && let Err(err) = std::fs::create_dir_all(profile)
        {
            tracing::warn!("creating {}: {err}", profile.display());
        }
        let mut cmd = Command::new("sh");
        // Electron apps would look for an X server, and there is none: nor
        // for what runs in a terminal started here.
        cmd.arg("-c")
            .arg(&prepared.command)
            .env("ELECTRON_OZONE_PLATFORM_HINT", "auto");
        let log = program_name(command).unwrap_or_else(|| "app".into());
        if log == CURSOR
            && let Some(home) = home_dir()
            && let Err(err) = crate::cursor_agents::install_hooks(&home)
        {
            tracing::warn!("adding gerdr to Cursor's hooks: {err}");
            self.toast(
                format!("Cursor's agents won't show at once: {err}"),
                crate::toast::Level::Warning,
            );
        }
        // Not one that hands its window to another process: that one's
        // would come and go with gerdr.
        if prepared.app_id.is_empty() && self.keeps(command) {
            return self.start_kept(cmd, &dir, &log, command.trim());
        }
        self.spawn(cmd, &dir, &log)
    }

    /// Take in the installed apps, which `scan_installed_apps` reads off the
    /// event loop, and read them once now.
    pub fn watch_installed_apps(&mut self) {
        let (tx, rx) = channel::channel();
        let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
            if let channel::Event::Msg(apps) = event {
                state.installed_apps = apps;
                state.refresh_run_prompt();
            }
        });
        match inserted {
            Ok(_) => {
                self.installed_apps_tx = Some(tx);
                self.scan_installed_apps();
            }
            Err(err) => tracing::warn!("installed apps: {err}"),
        }
    }

    /// Read the installed apps again, off the event loop.
    pub fn scan_installed_apps(&self) {
        let Some(tx) = self.installed_apps_tx.clone() else {
            return;
        };
        std::thread::spawn(move || {
            let _ = tx.send(desktop::installed_apps());
        });
    }
}

/// Browsers of the Firefox family, by program and Flatpak id.
const FIREFOXES: &[&str] = &[
    "firefox",
    "firefox-esr",
    "firefox-bin",
    "librewolf",
    "waterfox",
    "floorp",
    "zen",
    "zen-browser",
    "icecat",
    "mullvad-browser",
    "org.mozilla.firefox",
    "io.gitlab.librewolf-community",
    "net.waterfox.waterfox",
    "one.ablaze.floorp",
    "app.zen_browser.zen",
];

/// The browsers gerdr's extension is known to load in by the command line
/// (see `Gerdr::steered`): Chrome's own builds don't, since Chrome 137.
const STEERED: &[&str] = &[
    "chromium",
    "chromium-browser",
    "brave",
    "brave-browser",
    "brave-browser-stable",
];

/// Browsers of the Chromium family, by program and Flatpak id.
const CHROMIUMS: &[&str] = &[
    "chromium",
    "chromium-browser",
    "google-chrome",
    "google-chrome-stable",
    "brave",
    "brave-browser",
    "brave-browser-stable",
    "vivaldi",
    "vivaldi-stable",
    "microsoft-edge",
    "microsoft-edge-stable",
    "thorium-browser",
    "org.chromium.Chromium",
    "com.brave.Browser",
    "com.google.Chrome",
    "com.vivaldi.Vivaldi",
    "com.microsoft.Edge",
    "io.github.ungoogled_software.ungoogled_chromium",
];

/// `opener`, the command `desktop::url_command` gives for `url`, changed to
/// open a web page like an installed web app: in a window of its own,
/// without tabs or an address bar. With the part of the app_id its windows
/// are told by: the next ones come from the browser already running. That
/// takes a browser of the Chromium family (Firefox's have no such window):
/// the default one, or else the first installed. `None` for a URL other
/// than a web page's, or without such a browser.
fn web_app(url: &str, opener: &str) -> Option<(String, String)> {
    let url = url.trim();
    let (scheme, rest) = url.split_once("://")?;
    // A page of gerdr's extension too, as the preview (see `md_preview`).
    let own = scheme == "chrome-extension"
        && rest.starts_with(&format!("{}/", web_control::EXTENSION_ID));
    if !own && !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let quoted = desktop::shell_quote(url);
    let opener = opener.strip_suffix(&format!(" {quoted}"))?;
    let browser = match program(opener) {
        Some((key, ..)) if CHROMIUMS.contains(&key.as_str()) => opener.to_owned(),
        // Flatpak ids hold dots, programs don't.
        _ => CHROMIUMS
            .iter()
            .find(|name| !name.contains('.') && desktop::installed(name))?
            .to_string(),
    };
    // Chromium names a web app's window `<browser>-<host>_<path>-<profile>`,
    // an IPv6 host in its brackets.
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    let host = match host.find(']') {
        Some(end) if host.starts_with('[') => &host[..=end],
        _ => host.split(':').next()?,
    };
    if host.is_empty() {
        return None;
    }
    let site = format!("-{}_", host.to_ascii_lowercase());
    Some((format!("{browser} --app={quoted}"), site))
}

/// The folder of the profile of gerdr's that the browser showing `command`
/// as a web app runs with in instance folder `instance` (see `web_app` and
/// `own_instance`); `None` when `command` isn't a web page's link.
pub fn web_profile(command: &str, instance: &Path) -> Option<PathBuf> {
    let opener = desktop::url_command(command)?;
    let (web, _) = web_app(command, &opener)?;
    own_instance(&web, Some(instance))?.1
}

/// Editors built on VS Code, by program and Flatpak id.
const VS_CODES: &[&str] = &[
    "code",
    "code-insiders",
    "code-oss",
    "codium",
    "vscodium",
    "cursor",
    "windsurf",
    "com.visualstudio.code",
    "com.visualstudio.code-oss",
    "com.vscodium.codium",
];

/// Ghostty, by program and Flatpak id.
const GHOSTTYS: &[&str] = &["ghostty", "com.mitchellh.ghostty"];

/// Zed, by program (the CLI, its desktop file's name, the editor itself)
/// and Flatpak id.
const ZEDS: &[&str] = &[
    "zed",
    "zeditor",
    "zed-editor",
    "zed-preview",
    "zed-nightly",
    "dev.zed.Zed",
    "dev.zed.Zed-Preview",
    "dev.zed.Zed-Nightly",
    "dev.zed.Zed-Dev",
];

/// Whether `command` starts Zed with nothing after the program: no
/// options, no paths.
fn opens_nothing_in_zed(command: &str) -> bool {
    program(command).is_some_and(|(key, end, _)| {
        ZEDS.contains(&key.as_str()) && command[end..].trim().is_empty()
    })
}

/// The app_id a Zed window is expected by (see `Expected`).
pub const ZED_APP_ID: &str = "zed";

/// What of a Zed data dir is its instance's own; the rest links to the
/// data dir all of gerdr's Zeds share (see `zed_data_dir`). Its socket
/// (`zed-<channel>.sock`) is its own too.
const ZED_OWN: &[&str] = &["db", "logs", "hang_traces"];

/// What all of gerdr's Zeds share even before one has made it.
const ZED_SHARED: &[&str] = &["config", "extensions"];

/// The folder of the profiles of gerdr's Zed, VS Codes and browsers for
/// `instance` (the project shown, see `Gerdr::show_project`). They run one
/// process per profile and hand new windows to it: with one per project,
/// not to another gerdr's. What they keep for all projects (settings,
/// logins, extensions) comes from gerdr's shared profile of each, see
/// `zed_data_dir`, `ready_vs_code_profile` and `seed_browser_profile`.
pub fn instance_dir(home: &Path, instance: &str) -> PathBuf {
    home.join(".local/share/gerdr/instances").join(instance)
}

/// The data dir of gerdr's Zed in instance folder `instance` (see
/// `instance_dir`), made ready: all but its database, logs and socket link
/// to `~/.local/share/gerdr/zed`, which also keeps Zed apart from one on
/// the host.
pub fn zed_data_dir(home: &Path, instance: &Path) -> PathBuf {
    let dir = instance.join("zed");
    if let Err(err) = link_zed_shared(&home.join(".local/share/gerdr/zed"), &dir) {
        tracing::warn!("making Zed's data dir {}: {err}", dir.display());
    }
    dir
}

/// Link what Zed data dir `dir` shares to the one in `shared` (see
/// `zed_data_dir`), leaving what it has already. A new one starts with a
/// copy of the shared database: the folders trusted and the tabs open in
/// them are kept there.
fn link_zed_shared(shared: &Path, dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let db = dir.join("db");
    if !db.exists() && shared.join("db").is_dir() {
        copy_tree(&shared.join("db"), &db, &|_| false)?;
    }
    for name in ZED_SHARED {
        std::fs::create_dir_all(shared.join(name))?;
    }
    link_entries(shared, dir, |name| {
        !ZED_OWN.iter().any(|own| name == *own) && !name.to_string_lossy().ends_with(".sock")
    })
}

/// Link the entries of folder `shared` that `linked` picks into `dir`,
/// leaving those `dir` has already.
fn link_entries(
    shared: &Path,
    dir: &Path,
    linked: impl Fn(&std::ffi::OsStr) -> bool,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(shared)? {
        let name = entry?.file_name();
        let link = dir.join(&name);
        if linked(&name) && link.symlink_metadata().is_err() {
            std::os::unix::fs::symlink(shared.join(&name), link)?;
        }
    }
    Ok(())
}

/// What of a VS Code's `User/globalStorage/storage.json` is its process's
/// own: the windows it brings back and the workspaces it keeps unsaved
/// edits of (in its own `Backups`). Another process's would open its
/// windows, or drop its backups as missing.
const VS_CODE_OWN_STATE: &[&str] = &["windowsState", "backupWorkspaces"];

/// Make VS Code profile `dir` (see `own_instance`) share what VS Code
/// `key` keeps in gerdr's profile of it for all projects: everything in
/// `User` (settings, logins, chats, extension data) but the state of its
/// windows (see `VS_CODE_OWN_STATE`), copied when new, and the machine id.
/// The rest, Chromium's own and locked while it runs, is the profile's.
fn ready_vs_code_profile(home: &Path, key: &str, dir: &Path) {
    let shared = profile_dir(home, key);
    if let Err(err) = link_vs_code_shared(&shared, dir) {
        tracing::warn!("making the profile {}: {err}", dir.display());
    }
}

/// See `ready_vs_code_profile`.
fn link_vs_code_shared(shared: &Path, dir: &Path) -> std::io::Result<()> {
    let (shared_user, user) = (shared.join("User"), dir.join("User"));
    let (shared_global, global) = (
        shared_user.join("globalStorage"),
        user.join("globalStorage"),
    );
    std::fs::create_dir_all(&shared_global)?;
    std::fs::create_dir_all(&global)?;
    link_entries(shared, dir, |name| name == "machineid")?;
    link_entries(&shared_user, &user, |name| name != "globalStorage")?;
    // SQLite finds a database's journal by where its link leads.
    link_entries(&shared_global, &global, |name| {
        let name = name.to_string_lossy();
        name != "storage.json"
            && !["-wal", "-shm", "-journal"]
                .iter()
                .any(|end| name.ends_with(end))
    })?;
    let state = global.join("storage.json");
    if state.exists() {
        return Ok(());
    }
    let Ok(text) = std::fs::read_to_string(shared_global.join("storage.json")) else {
        return Ok(());
    };
    let Ok(serde_json::Value::Object(mut fields)) = serde_json::from_str(&text) else {
        return Ok(());
    };
    for own in VS_CODE_OWN_STATE {
        fields.remove(*own);
    }
    std::fs::write(state, serde_json::Value::Object(fields).to_string())
}

/// What a browser's new profile leaves out of the copy of gerdr's shared
/// one, by name: caches, the locks of the browser running on it, and the
/// windows and tabs it would bring back, which are another project's.
pub(crate) const BROWSER_UNCOPIED: &[&str] = &[
    // Chromium
    "SingletonLock",
    "SingletonSocket",
    "SingletonCookie",
    "DevToolsActivePort",
    "Cache",
    "Code Cache",
    "GPUCache",
    "GrShaderCache",
    "ShaderCache",
    "GraphiteDawnCache",
    "DawnGraphiteCache",
    "DawnWebGPUCache",
    "GPUPersistentCache",
    "CacheStorage",
    "ScriptCache",
    "component_crx_cache",
    "extensions_crx_cache",
    "BrowserMetrics",
    "DeferredBrowserMetrics",
    "Sessions",
    "Current Session",
    "Current Tabs",
    "Last Session",
    "Last Tabs",
    // The web apps installed in the user's own Chromium: started from the
    // copy, the browser would point their desktop shortcuts at it, and they
    // would open in gerdr (see `leave_out_web_apps`).
    WEB_APPS,
    // Firefox
    "lock",
    ".parentlock",
    "parent.lock",
    "cache2",
    "startupCache",
    "sessionstore.jsonlz4",
    "sessionstore-backups",
    "sessionCheckpoints.json",
];

/// Where a Chromium profile keeps its installed web apps (PWAs).
const WEB_APPS: &str = "Web Applications";
/// Marks a gerdr Chromium profile that has no web apps of the user's own
/// Chromium any more (see `leave_out_web_apps`).
const WEB_APPS_LEFT_OUT: &str = ".gerdr-web-apps-left-out";

/// Move aside, once, the web apps a project's Chromium `profile` has from
/// the user's own Chromium (copied before gerdr left them out, see
/// `BROWSER_UNCOPIED`), while no Chromium runs on it. Started on them,
/// the browser points the apps' desktop shortcuts at this profile, and
/// they open in gerdr. Web apps installed in the profile after that stay.
fn leave_out_web_apps(profile: &Path) {
    let marker = profile.join(WEB_APPS_LEFT_OUT);
    if marker.exists() || !profile.is_dir() || crate::profile_import::running(profile) {
        return;
    }
    // Each of the browser's profiles: `Default`, `Profile 1`…
    for entry in std::fs::read_dir(profile).into_iter().flatten().flatten() {
        let apps = entry.path().join(WEB_APPS);
        if !apps.is_dir() {
            continue;
        }
        let aside = entry.path().join(format!("{WEB_APPS}.left-out"));
        // One moved aside before, if any: this one is newer.
        let _ = std::fs::remove_dir_all(&aside);
        match std::fs::rename(&apps, &aside) {
            Ok(()) => tracing::info!(
                "left the user's web apps out of gerdr's {}: in {}",
                profile.display(),
                aside.display()
            ),
            Err(err) => {
                // Not marked: tried again at the next start.
                return tracing::warn!("leaving {} out: {err}", apps.display());
            }
        }
    }
    if let Err(err) = std::fs::write(&marker, "") {
        tracing::warn!("marking {}: {err}", marker.display());
    }
}

/// Start browser profile `dir` (see `own_instance`), when it is new, as a
/// copy of gerdr's profile of browser `key`, so the project's browser
/// begins with its logins, bookmarks and extensions. A browser locks its
/// databases while it runs, so profiles can't share them: from then on
/// each is its own. Copied beside it first, so a copy cut short isn't
/// taken for one.
fn seed_browser_profile(home: &Path, key: &str, dir: &Path) {
    let shared = profile_dir(home, key);
    if dir.exists() || !shared.is_dir() {
        return;
    }
    if let Err(err) = copy_browser_profile(&shared, dir) {
        tracing::warn!("copying {} to {}: {err}", shared.display(), dir.display());
    }
}

/// See `seed_browser_profile`.
fn copy_browser_profile(shared: &Path, dir: &Path) -> std::io::Result<()> {
    let mut new = dir.as_os_str().to_owned();
    new.push(".new");
    let new = PathBuf::from(new);
    let _ = std::fs::remove_dir_all(&new);
    let skipped = |name: &std::ffi::OsStr| BROWSER_UNCOPIED.iter().any(|skip| name == *skip);
    let copied = copy_tree(shared, &new, &skipped).and_then(|()| std::fs::rename(&new, dir));
    if copied.is_err() {
        let _ = std::fs::remove_dir_all(&new);
    }
    copied
}

/// Copy folder `from` to `to`, with all in it but the entries `skipped`
/// picks by name. Links stay links; sockets and what goes while it copies
/// (an app may be running on `from`) are left out.
pub(crate) fn copy_tree(
    from: &Path,
    to: &Path,
    skipped: &dyn Fn(&std::ffi::OsStr) -> bool,
) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        if skipped(&name) {
            continue;
        }
        let (source, target) = (entry.path(), to.join(&name));
        let copied = entry.file_type().and_then(|kind| {
            if kind.is_dir() {
                copy_tree(&source, &target, skipped)
            } else if kind.is_symlink() {
                std::fs::read_link(&source)
                    .and_then(|link| std::os::unix::fs::symlink(link, &target))
            } else if kind.is_file() {
                std::fs::copy(&source, &target).map(drop)
            } else {
                Ok(())
            }
        });
        match copied {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err),
            _ => {}
        }
    }
    Ok(())
}

/// Ghostty's option for keeping one copy running.
const SINGLE_INSTANCE: &str = "--gtk-single-instance";

/// The program `command` starts, to tell apps apart: its file name, or for
/// `flatpak run`, the app's id. With it, where it ends in `command`, and
/// whether it is a Flatpak app.
fn program(command: &str) -> Option<(String, usize, bool)> {
    let unquote = |word: &str| word.trim_matches(['\'', '"']).to_owned();
    // The words with where they start; a launcher's own words come first.
    let mut words = command
        .split_whitespace()
        .map(|word| (word.as_ptr().addr() - command.as_ptr().addr(), word))
        .skip_while(|(_, word)| matches!(*word, "exec" | "env") || word.contains('='));
    let (start, program) = words.next()?;
    let name = Path::new(&unquote(program))
        .file_name()?
        .to_str()?
        .to_owned();
    if name == "flatpak" {
        // flatpak run [options] id [args]
        words.next().filter(|(_, word)| *word == "run")?;
        let (start, id) = words.find(|(_, word)| !word.starts_with('-'))?;
        Some((unquote(id), start + id.len(), true))
    } else {
        Some((name, start + program.len(), false))
    }
}

/// A terminal gerdr keeps one process of for the project shown, to open
/// the windows of all its launches in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shared {
    Ghostty,
    Alacritty,
}

impl Shared {
    /// The one `command` opens a window of, if gerdr's can open it: not a
    /// Flatpak one (a Flatpak Ghostty may not take gerdr's D-Bus name), nor
    /// one with a class or socket of its own (an option before the `-e`
    /// command, which takes the rest), nor one of their actions.
    fn of(command: &str) -> Option<Self> {
        let (key, end, flatpak) = program(command)?;
        if flatpak {
            return None;
        }
        let mut words = command[end..].split_whitespace();
        let first = words.next().unwrap_or("");
        let is_option = |word: &str, option: &str| {
            word == option
                || word
                    .strip_prefix(option)
                    .is_some_and(|rest| rest.starts_with('='))
        };
        let own = std::iter::once(first)
            .chain(words)
            .take_while(|word| !matches!(*word, "-e" | "--command"))
            .any(|word| is_option(word, "--class") || is_option(word, "--socket"));
        if own {
            return None;
        }
        match key.as_str() {
            "ghostty" if !first.starts_with('+') => Some(Self::Ghostty),
            "alacritty" if !matches!(first, "msg" | "migrate" | "help") => Some(Self::Alacritty),
            _ => None,
        }
    }

    /// The app_id its windows are told by.
    fn app_id(self) -> &'static str {
        match self {
            Self::Ghostty => GHOSTTY_APP_ID,
            Self::Alacritty => ALACRITTY_APP_ID,
        }
    }
}

/// `command` split where its program ends, without a leading `exec`, which
/// would leave no shell to try a second way.
fn split_program(command: &str) -> Option<(&str, &str)> {
    let (_, end, _) = program(command)?;
    let (head, tail) = command.split_at(end);
    let head = head.trim_start().strip_prefix("exec ").unwrap_or(head);
    Some((head, tail))
}

/// `command`, a Ghostty gerdr's can open, changed to open its window in the
/// Ghostty of `class`, or to become it when there is none yet; in `dir`
/// unless it names a folder (a window handed to the running one would open
/// in that one's folder). `None` for any other command.
fn ghostty_command(command: &str, class: &str, dir: &Path) -> Option<String> {
    if Shared::of(command) != Some(Shared::Ghostty) {
        return None;
    }
    let (head, tail) = split_program(command)?;
    // The desktop file's `--gtk-single-instance=true` goes: gerdr picks.
    let tail = tail
        .split(' ')
        .filter(|word| !word.starts_with(SINGLE_INSTANCE))
        .collect::<Vec<_>>()
        .join(" ");
    let mut options = format!(" --class={class}");
    if !tail.contains("--working-directory") {
        let dir = desktop::shell_quote(&dir.to_string_lossy());
        options.push_str(&format!(" --working-directory={dir}"));
    }
    Some(format!(
        "{head} +new-window{options}{tail} 2>/dev/null \
         || exec {head}{options} {SINGLE_INSTANCE}=true{tail}"
    ))
}

/// `command`, an Alacritty gerdr's can open, changed to open its window in
/// the Alacritty listening on `socket`, or to become it when there is none
/// yet (a socket left by one that was killed goes first); in `dir` unless
/// it names a folder, with Alacritty's `extra` options (the look's
/// colours, see `Palette::alacritty_options`). `None` for any other
/// command.
fn alacritty_command(command: &str, socket: &Path, dir: &Path, extra: &[String]) -> Option<String> {
    if Shared::of(command) != Some(Shared::Alacritty) {
        return None;
    }
    let (head, tail) = split_program(command)?;
    let socket = desktop::shell_quote(&socket.to_string_lossy());
    let mut options = format!(" --class {ALACRITTY_APP_ID}");
    if !tail.contains("--working-directory") {
        let dir = desktop::shell_quote(&dir.to_string_lossy());
        options.push_str(&format!(" --working-directory {dir}"));
    }
    for word in extra {
        options.push(' ');
        options.push_str(&desktop::shell_quote(word));
    }
    Some(format!(
        "{head} msg --socket {socket} create-window{options}{tail} 2>/dev/null \
         || {{ rm -f {socket}; exec {head} --socket {socket}{options}{tail}; }}"
    ))
}

/// Whether `command` starts an app that keeps its logins in the keyring
/// (see `keyring`): a Chromium, which a web page opens in too (see
/// `web_app`), or a VS Code.
pub(crate) fn uses_keyring(command: &str) -> bool {
    let opener = desktop::url_command(command);
    if opener
        .as_deref()
        .is_some_and(|opener| web_app(command, opener).is_some())
    {
        return true;
    }
    program_name(opener.as_deref().unwrap_or(command))
        .is_some_and(|key| CHROMIUMS.contains(&key.as_str()) || VS_CODES.contains(&key.as_str()))
}

/// The program `command` starts (see `program`), to match commands to the
/// installed apps by.
pub fn program_name(command: &str) -> Option<String> {
    program(command).map(|(name, ..)| name)
}

/// The VS Code `command` starts (`cursor`, `code`…), by program, if any:
/// one copy runs gerdr's profile and the next ones hand it their window.
fn vs_code_of(command: &str) -> Option<String> {
    program_name(command).filter(|key| VS_CODES.contains(&key.as_str()))
}

/// VS Code `command` with the profile of the one running as `pid`, which
/// it then hands what it opens: the same `--user-data-dir`, or none for
/// the default. `None` when `pid` doesn't run (or just started).
fn with_profile_of(command: &str, pid: u32) -> Option<String> {
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let args = cmdline_args(&cmdline);
    if args.first().is_none_or(String::is_empty) {
        return None;
    }
    if command.contains("--user-data-dir") {
        return Some(command.to_owned());
    }
    let (_, end, _) = program(command)?;
    let mut command = command.to_owned();
    if let Some(dir) = user_data_dir(&args) {
        command.insert_str(
            end,
            &format!(" --user-data-dir={}", desktop::shell_quote(&dir)),
        );
    }
    Some(command)
}

/// The arguments in `cmdline`, as /proc/<pid>/cmdline holds them. Cursor
/// (Electron) writes its process title over them: one string, its words
/// joined by spaces, which are split again.
pub(crate) fn cmdline_args(cmdline: &[u8]) -> Vec<String> {
    let mut args: Vec<String> = cmdline
        .strip_suffix(&[0])
        .unwrap_or(cmdline)
        .split(|byte| *byte == 0)
        .map(|arg| String::from_utf8_lossy(arg).into_owned())
        .collect();
    if let [title] = args.as_slice()
        && title.contains(" -")
    {
        args = title.split_whitespace().map(str::to_owned).collect();
    }
    args
}

/// The `--user-data-dir` among `args`, if any.
fn user_data_dir(args: &[String]) -> Option<String> {
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if let Some(dir) = arg.strip_prefix("--user-data-dir=") {
            return Some(dir.to_owned());
        }
        if arg == "--user-data-dir" {
            return args.next().cloned();
        }
    }
    None
}

/// The profile gerdr keeps for the app of program `key`, which its
/// projects' profiles share or start from (see `instance_dir`).
pub(crate) fn profile_dir(home: &Path, key: &str) -> PathBuf {
    home.join(".local/share/gerdr/profiles").join(key)
}

/// The profile all of gerdr's Cursors share their chats in.
pub fn cursor_profile() -> Option<PathBuf> {
    Some(profile_dir(&home_dir()?, CURSOR))
}

/// `command` changed to start a copy of its own of an app that hands new
/// windows to a running one: a browser, a VS Code or a Zed with gerdr's
/// profile for it in instance folder `instance` (see `instance_dir`; a
/// Flatpak app's in its own folder), and that folder, or a Ghostty told
/// not to (without a folder); `None` when `command` doesn't start such an
/// app, already picks its profile, or needs `instance` and has none.
fn own_instance(command: &str, instance: Option<&Path>) -> Option<(String, Option<PathBuf>)> {
    let (key, end, flatpak) = program(command)?;
    if GHOSTTYS.contains(&key.as_str()) {
        let off = format!("{SINGLE_INSTANCE}=false");
        // The desktop file's `--gtk-single-instance=true` becomes `false`.
        let mut words = command.split_whitespace();
        let command = match words.find(|word| word.starts_with(SINGLE_INSTANCE)) {
            Some(word) => command.replacen(word, &off, 1),
            None => {
                let mut command = command.to_owned();
                command.insert_str(end, &format!(" {off}"));
                command
            }
        };
        return Some((command, None));
    }
    if [
        "--profile",
        " -P ",
        "--user-data-dir",
        "-no-remote",
        "--new-instance",
    ]
    .iter()
    .any(|flag| command.contains(flag))
    {
        return None;
    }
    let home = home_dir()?;
    // A Flatpak app sees only its own folder.
    let profile = if flatpak {
        home.join(".var/app").join(&key).join("gerdr-profile")
    } else if ZEDS.contains(&key.as_str()) {
        instance?.join("zed")
    } else {
        instance?.join(&key)
    };
    let quoted = desktop::shell_quote(&profile.to_string_lossy());
    let flags = if FIREFOXES.contains(&key.as_str()) {
        // Without a window option, a running one opens a tab instead.
        let window = if command.contains("-window") {
            ""
        } else {
            " --new-window"
        };
        format!(" --name {BROWSER_APP_ID}.{key} --profile {quoted}{window}")
    } else if CHROMIUMS.contains(&key.as_str())
        || VS_CODES.contains(&key.as_str())
        || ZEDS.contains(&key.as_str())
    {
        format!(" --user-data-dir={quoted}")
    } else {
        return None;
    };
    let mut command = command.to_owned();
    command.insert_str(end, &flags);
    Some((command, Some(profile)))
}

/// `pid` and the processes it descends from, nearest first, up to (not
/// including) init.
pub fn lineage(pid: u32) -> Vec<u32> {
    let mut chain = vec![pid];
    // A bound, in case /proc changes under us into a loop.
    while chain.len() < 64 {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{}/stat", chain[chain.len() - 1]))
        else {
            break;
        };
        // The command name, in parentheses, may hold spaces: the fields
        // after it are the state, then the parent.
        let parent = stat
            .rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().nth(1)?.parse().ok());
        match parent {
            Some(parent) if parent > 1 => chain.push(parent),
            _ => break,
        }
    }
    chain
}

/// The commands typed in the run prompt, the latest first.
pub fn history() -> Vec<String> {
    std::fs::read_to_string(history_file())
        .map(|text| {
            text.lines()
                .filter(|line| !line.trim().is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Put `command` first in the run prompt's history.
pub fn remember(command: &str) {
    let mut commands = history();
    commands.retain(|c| c != command);
    commands.insert(0, command.to_owned());
    commands.truncate(HISTORY_LEN);
    let path = history_file();
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&path, commands.join("\n") + "\n"));
    if let Err(err) = written {
        tracing::warn!("saving {}: {err}", path.display());
    }
}

fn history_file() -> PathBuf {
    home_dir()
        .unwrap_or_default()
        .join(".local/share/gerdr/run_history")
}

/// A log bigger than this is started over.
const LOG_MAX: u64 = 1 << 20;

/// Where what `app` (a program's name) writes to its stderr goes:
/// `~/.local/state/gerdr/launch/<app>.log`.
pub fn log_path(app: &str) -> PathBuf {
    let name: String = app
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let name = match name.trim_matches('.') {
        "" => "app",
        name => name,
    };
    state_dir().join("launch").join(format!("{name}.log"))
}

/// `app`'s log, open for appending; started over once too big. `None`
/// (and a warning) when it can't be opened.
pub fn log_file(app: &str) -> Option<std::fs::File> {
    append_log(&log_path(app))
}

/// The log at `path`, open for appending, its folder made; started over
/// once too big. `None` (and a warning) when it can't be opened.
pub fn append_log(path: &Path) -> Option<std::fs::File> {
    let open = || {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let big = std::fs::metadata(path).is_ok_and(|m| m.len() > LOG_MAX);
        std::fs::OpenOptions::new()
            .create(true)
            .append(!big)
            .write(true)
            .truncate(big)
            .open(path)
    };
    open()
        .inspect_err(|err| tracing::warn!("opening {}: {err}", path.display()))
        .ok()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        alacritty_command, cmdline_args, copy_browser_profile, ghostty_command, lineage,
        link_vs_code_shared, link_zed_shared, log_path, opens_nothing_in_zed, own_instance,
        user_data_dir, vs_code_of, web_app, with_profile_of,
    };
    use crate::home_dir;

    #[test]
    fn links_go_to_the_running_profile() {
        let args = |line: &str| line.split(' ').map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(
            user_data_dir(&args("/usr/share/cursor/cursor --user-data-dir=/p/cursor")).as_deref(),
            Some("/p/cursor")
        );
        assert_eq!(
            user_data_dir(&args("cursor --type=renderer --user-data-dir /p/x")).as_deref(),
            Some("/p/x")
        );
        assert_eq!(user_data_dir(&args("cursor .")), None);
        // Cursor's own, its title written over its arguments.
        assert_eq!(
            user_data_dir(&cmdline_args(
                b"/usr/share/cursor/cursor --user-data-dir=/p/c\0"
            ))
            .as_deref(),
            Some("/p/c")
        );
        assert_eq!(
            cmdline_args(b"cursor\0--user-data-dir\0/p/my dir\0"),
            ["cursor", "--user-data-dir", "/p/my dir"]
        );
        // This test's own process runs without one: the default profile.
        let me = std::process::id();
        assert_eq!(
            with_profile_of("/usr/share/cursor/cursor", me).as_deref(),
            Some("/usr/share/cursor/cursor")
        );
        assert_eq!(
            with_profile_of("cursor --user-data-dir=/x", me).as_deref(),
            Some("cursor --user-data-dir=/x")
        );
        assert_eq!(with_profile_of("cursor", u32::MAX), None);
    }

    #[test]
    fn vs_codes_by_program() {
        assert_eq!(
            vs_code_of("/usr/share/cursor/cursor").as_deref(),
            Some("cursor")
        );
        assert_eq!(vs_code_of("cursor --glass .").as_deref(), Some("cursor"));
        assert_eq!(
            vs_code_of("flatpak run com.visualstudio.code").as_deref(),
            Some("com.visualstudio.code")
        );
        assert_eq!(vs_code_of("zed ."), None);
        assert_eq!(vs_code_of("ghostty -e htop"), None);
    }

    #[test]
    fn lineage_goes_up() {
        let chain = lineage(std::process::id());
        assert_eq!(chain[0], std::process::id());
        if std::os::unix::process::parent_id() > 1 {
            assert_eq!(chain[1], std::os::unix::process::parent_id());
        }
    }

    #[test]
    fn web_pages_open_as_apps() {
        let url = "https://User@Example.com:8080/a?b=c&d";
        assert_eq!(
            web_app(url, &format!("/usr/bin/brave-browser-stable '{url}'")).unwrap(),
            (
                format!("/usr/bin/brave-browser-stable --app='{url}'"),
                "-example.com_".to_owned()
            )
        );
        assert_eq!(
            web_app("http://[::1]:3000", "chromium 'http://[::1]:3000'")
                .unwrap()
                .1,
            "-[::1]_"
        );
        assert_eq!(web_app("ftp://a.b", "chromium ftp://a.b"), None);
        assert_eq!(web_app("https://a.b", "xdg-open https://c.d"), None);
    }

    #[test]
    fn ghostty_opens_in_gerdrs() {
        let run =
            |command: &str| ghostty_command(command, "dev.gerdr.ghostty.w_0", Path::new("/my dir"));
        assert_eq!(
            run("/usr/bin/ghostty --gtk-single-instance=true").unwrap(),
            "/usr/bin/ghostty +new-window --class=dev.gerdr.ghostty.w_0 \
             --working-directory='/my dir' 2>/dev/null \
             || exec /usr/bin/ghostty --class=dev.gerdr.ghostty.w_0 \
             --working-directory='/my dir' --gtk-single-instance=true"
        );
        assert_eq!(
            run("exec ghostty --working-directory=/x -e htop").unwrap(),
            "ghostty +new-window --class=dev.gerdr.ghostty.w_0 --working-directory=/x -e htop \
             2>/dev/null \
             || exec ghostty --class=dev.gerdr.ghostty.w_0 --gtk-single-instance=true \
             --working-directory=/x -e htop"
        );
        // The `-e` command's own words are not Ghostty's options.
        assert!(run("ghostty -e curl https://x/--socket-docs --class").is_some());
        assert_eq!(run("ghostty --class=my.term"), None);
        assert_eq!(run("ghostty --class my.term"), None);
        assert_eq!(run("flatpak run com.mitchellh.ghostty"), None);
        assert_eq!(run("ghostty +list-fonts"), None);
        assert_eq!(run("alacritty"), None);
    }

    #[test]
    fn alacritty_takes_the_colours_in_both_ways_of_opening() {
        let colours = ["-o".to_owned(), "colors.normal.red=\"#e06c75\"".to_owned()];
        let run = alacritty_command(
            "alacritty -e htop",
            Path::new("/run/a.sock"),
            Path::new("/d"),
            &colours,
        )
        .unwrap();
        assert_eq!(
            run,
            "alacritty msg --socket /run/a.sock create-window --class dev.gerdr.terminal \
             --working-directory /d -o 'colors.normal.red=\"#e06c75\"' -e htop 2>/dev/null \
             || { rm -f /run/a.sock; exec alacritty --socket /run/a.sock \
             --class dev.gerdr.terminal --working-directory /d \
             -o 'colors.normal.red=\"#e06c75\"' -e htop; }"
        );
    }

    #[test]
    fn launch_logs_are_named_after_the_program() {
        let state = crate::state_dir();
        let under = |app: &str| {
            log_path(app)
                .strip_prefix(&state)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        };
        assert_eq!(under("zed"), "launch/zed.log");
        assert_eq!(under("firefox-esr"), "launch/firefox-esr.log");
        assert_eq!(under("a b/c"), "launch/a_b_c.log");
        assert_eq!(under(".."), "launch/app.log");
        assert_eq!(under(""), "launch/app.log");
    }

    #[test]
    fn alacritty_opens_in_gerdrs() {
        let run = |command: &str| {
            alacritty_command(command, Path::new("/run/a.sock"), Path::new("/my dir"), &[])
        };
        assert_eq!(
            run("alacritty").unwrap(),
            "alacritty msg --socket /run/a.sock create-window --class dev.gerdr.terminal \
             --working-directory '/my dir' 2>/dev/null \
             || { rm -f /run/a.sock; exec alacritty --socket /run/a.sock \
             --class dev.gerdr.terminal --working-directory '/my dir'; }"
        );
        assert_eq!(
            run("exec alacritty --working-directory /x -e htop").unwrap(),
            "alacritty msg --socket /run/a.sock create-window --class dev.gerdr.terminal \
             --working-directory /x -e htop 2>/dev/null \
             || { rm -f /run/a.sock; exec alacritty --socket /run/a.sock \
             --class dev.gerdr.terminal --working-directory /x -e htop; }"
        );
        assert_eq!(run("alacritty --class my.term"), None);
        assert_eq!(run("alacritty --socket=/s"), None);
        assert!(run("alacritty -e foo --classic").is_some());
        assert!(run("alacritty --command foo --socket /s").is_some());
        assert_eq!(run("alacritty msg create-window"), None);
        assert_eq!(run("ghostty"), None);
    }

    #[test]
    fn zed_data_dirs_share_all_but_their_own() {
        let root = std::env::temp_dir().join(format!("gerdr-zed-data-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (shared, dir) = (root.join("zed"), root.join("aft"));
        for name in ["db", "logs", "languages"] {
            std::fs::create_dir_all(shared.join(name)).unwrap();
        }
        std::fs::write(shared.join("db/db.sqlite"), "trusted").unwrap();
        std::fs::write(shared.join("zed-stable.sock"), "").unwrap();
        std::fs::create_dir_all(dir.join("node")).unwrap();
        std::fs::create_dir_all(shared.join("node")).unwrap();
        link_zed_shared(&shared, &dir).unwrap();
        // Again: what is linked stays.
        link_zed_shared(&shared, &dir).unwrap();
        let mut names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                let linked = entry.file_type().unwrap().is_symlink();
                (entry.file_name().into_string().unwrap(), linked)
            })
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                ("config".to_owned(), true),
                ("db".to_owned(), false),
                ("extensions".to_owned(), true),
                ("languages".to_owned(), true),
                ("node".to_owned(), false),
            ]
        );
        assert_eq!(
            std::fs::read_link(dir.join("config")).unwrap(),
            shared.join("config")
        );
        // Copied when new only: its own from then on.
        assert_eq!(
            std::fs::read_to_string(dir.join("db/db.sqlite")).unwrap(),
            "trusted"
        );
        std::fs::write(shared.join("db/db.sqlite"), "later").unwrap();
        link_zed_shared(&shared, &dir).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("db/db.sqlite")).unwrap(),
            "trusted"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn vs_code_profiles_share_their_user_folder_but_window_state() {
        let root = std::env::temp_dir().join(format!("gerdr-vs-code-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (shared, dir) = (root.join("cursor"), root.join("aft"));
        let global = shared.join("User/globalStorage");
        for folder in ["Backups/1", "Local Storage", "User/workspaceStorage"] {
            std::fs::create_dir_all(shared.join(folder)).unwrap();
        }
        std::fs::create_dir_all(&global).unwrap();
        std::fs::write(shared.join("machineid"), "m").unwrap();
        std::fs::write(shared.join("code.lock"), "1").unwrap();
        std::fs::write(shared.join("User/settings.json"), "{}").unwrap();
        std::fs::write(global.join("state.vscdb"), "").unwrap();
        std::fs::write(global.join("state.vscdb-wal"), "").unwrap();
        std::fs::write(
            global.join("storage.json"),
            r#"{"telemetry.machineId":"t","windowsState":{},"backupWorkspaces":{}}"#,
        )
        .unwrap();
        link_vs_code_shared(&shared, &dir).unwrap();
        // Again: what it has stays.
        std::fs::write(global.join("storage.json"), "{}").unwrap();
        link_vs_code_shared(&shared, &dir).unwrap();
        let entries = |dir: &Path| {
            let mut names: Vec<_> = std::fs::read_dir(dir)
                .unwrap()
                .map(|entry| {
                    let entry = entry.unwrap();
                    let linked = entry.file_type().unwrap().is_symlink();
                    (entry.file_name().into_string().unwrap(), linked)
                })
                .collect();
            names.sort();
            names
        };
        let owned = |names: &[(&str, bool)]| -> Vec<(String, bool)> {
            names
                .iter()
                .map(|(name, linked)| (name.to_string(), *linked))
                .collect()
        };
        assert_eq!(
            entries(&dir),
            owned(&[("User", false), ("machineid", true)])
        );
        assert_eq!(
            entries(&dir.join("User")),
            owned(&[
                ("globalStorage", false),
                ("settings.json", true),
                ("workspaceStorage", true),
            ])
        );
        assert_eq!(
            entries(&dir.join("User/globalStorage")),
            owned(&[("state.vscdb", true), ("storage.json", false)])
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("User/globalStorage/storage.json")).unwrap(),
            r#"{"telemetry.machineId":"t"}"#
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn browser_profiles_start_as_a_copy_without_caches_locks_or_sessions() {
        let root = std::env::temp_dir().join(format!("gerdr-browser-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (shared, dir) = (root.join("chromium"), root.join("aft"));
        for folder in ["Default/Cache", "Default/Sessions", "Default/Local Storage"] {
            std::fs::create_dir_all(shared.join(folder)).unwrap();
        }
        std::fs::write(shared.join("Default/Cookies"), "c").unwrap();
        std::fs::write(shared.join("Local State"), "{}").unwrap();
        std::os::unix::fs::symlink("host-1234", shared.join("SingletonLock")).unwrap();
        std::os::unix::fs::symlink("Local State", shared.join("state")).unwrap();
        copy_browser_profile(&shared, &dir).unwrap();
        let mut copied: Vec<_> = walk(&dir);
        copied.sort();
        assert_eq!(
            copied,
            [
                "Default",
                "Default/Cookies",
                "Default/Local Storage",
                "Local State",
                "state"
            ]
        );
        assert_eq!(
            std::fs::read_link(dir.join("state")).unwrap(),
            Path::new("Local State")
        );
        assert!(!root.join("aft.new").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The paths under `dir`, relative to it.
    fn walk(dir: &Path) -> Vec<String> {
        let mut paths = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().into_string().unwrap();
            if entry.file_type().unwrap().is_dir() {
                paths.extend(
                    walk(&entry.path())
                        .into_iter()
                        .map(|path| format!("{name}/{path}")),
                );
            }
            paths.push(name);
        }
        paths
    }

    #[test]
    fn chromiums_and_vs_codes_wait_for_the_keyring() {
        for command in [
            "chromium-browser --app=https://x.dev",
            "/usr/bin/brave-browser-stable",
            "cursor .",
            "code --new-window",
        ] {
            assert!(super::uses_keyring(command), "{command}");
        }
        for command in ["zed -n .", "firefox", "ghostty", "htop"] {
            assert!(!super::uses_keyring(command), "{command}");
        }
    }

    #[test]
    fn copies_of_a_chromium_profile_leave_its_web_apps_out_once() {
        let root = std::env::temp_dir().join(format!("gerdr-web-apps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (shared, copy) = (root.join("shared"), root.join("copy"));
        std::fs::create_dir_all(shared.join("Default/Web Applications/Manifest Resources"))
            .unwrap();
        std::fs::write(shared.join("Default/Preferences"), "{}").unwrap();
        super::copy_browser_profile(&shared, &copy).unwrap();
        assert!(copy.join("Default/Preferences").is_file());
        assert!(!copy.join("Default/Web Applications").exists());

        // A profile copied before: its web apps go aside, once.
        let old = root.join("old");
        std::fs::create_dir_all(old.join("Default/Web Applications/x")).unwrap();
        super::leave_out_web_apps(&old);
        assert!(!old.join("Default/Web Applications").exists());
        assert!(old.join("Default/Web Applications.left-out/x").is_dir());
        // One installed in it later stays.
        std::fs::create_dir_all(old.join("Default/Web Applications/y")).unwrap();
        super::leave_out_web_apps(&old);
        assert!(old.join("Default/Web Applications/y").is_dir());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn browsers_get_their_own_profile() {
        let home = home_dir().unwrap();
        let instance = home.join(".local/share/gerdr/instances/aft");
        let home = home.to_string_lossy().into_owned();
        let run = |command: &str| {
            own_instance(command, Some(&instance)).map(|(command, _)| command.replace(&home, "~"))
        };
        assert_eq!(
            run("/usr/share/librewolf/librewolf").unwrap(),
            "/usr/share/librewolf/librewolf --name dev.gerdr.browser.librewolf --profile ~/.local/share/gerdr/instances/aft/librewolf --new-window"
        );
        assert_eq!(
            run("exec firefox https://a.b && true").unwrap(),
            "exec firefox --name dev.gerdr.browser.firefox --profile ~/.local/share/gerdr/instances/aft/firefox --new-window https://a.b && true"
        );
        assert_eq!(
            run("brave-browser-stable").unwrap(),
            "brave-browser-stable --user-data-dir=~/.local/share/gerdr/instances/aft/brave-browser-stable"
        );
        assert_eq!(
            run("/usr/bin/flatpak run --branch=stable --command=firefox org.mozilla.firefox")
                .unwrap(),
            "/usr/bin/flatpak run --branch=stable --command=firefox org.mozilla.firefox --name dev.gerdr.browser.org.mozilla.firefox --profile ~/.var/app/org.mozilla.firefox/gerdr-profile --new-window"
        );
        assert_eq!(
            run("/usr/share/cursor/cursor --new-window").unwrap(),
            "/usr/share/cursor/cursor --user-data-dir=~/.local/share/gerdr/instances/aft/cursor --new-window"
        );
        assert_eq!(
            run("/usr/bin/ghostty --gtk-single-instance=true").unwrap(),
            "/usr/bin/ghostty --gtk-single-instance=false"
        );
        assert_eq!(
            run("ghostty -e htop").unwrap(),
            "ghostty --gtk-single-instance=false -e htop"
        );
        assert_eq!(run("code --user-data-dir=/x ."), None);
        assert_eq!(
            run("/home/me/.local/zed.app/bin/zed").unwrap(),
            "/home/me/.local/zed.app/bin/zed --user-data-dir=~/.local/share/gerdr/instances/aft/zed"
        );
        assert_eq!(
            run("zeditor -n .").unwrap(),
            "zeditor --user-data-dir=~/.local/share/gerdr/instances/aft/zed -n ."
        );
        assert_eq!(
            run("flatpak run dev.zed.Zed").unwrap(),
            "flatpak run dev.zed.Zed --user-data-dir=~/.var/app/dev.zed.Zed/gerdr-profile"
        );
        assert_eq!(run("zed --user-data-dir /x"), None);
        assert_eq!(own_instance("zed", None), None);
        assert_eq!(own_instance("cursor", None), None);
        assert_eq!(own_instance("chromium", None), None);
        assert!(opens_nothing_in_zed("/home/me/.local/zed.app/bin/zed"));
        assert!(opens_nothing_in_zed("flatpak run dev.zed.Zed "));
        assert!(!opens_nothing_in_zed("zed -n ."));
        assert!(!opens_nothing_in_zed("zed src/main.rs"));
        assert!(!opens_nothing_in_zed("code"));
        assert_eq!(
            run("librewolf --private-window").unwrap(),
            "librewolf --name dev.gerdr.browser.librewolf --profile ~/.local/share/gerdr/instances/aft/librewolf --private-window"
        );
        assert_eq!(run("firefox --profile /x"), None);
        assert_eq!(run("nautilus ."), None);
        assert_eq!(run("flatpak run md.obsidian.Obsidian"), None);
    }
}

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
//! that window is told by its app_id.
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
    Expected, Gerdr, Tab, Workspace, WorkspaceKind, app_icons::AppIcon, desktop, home_dir, layout,
    state_dir,
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
        if self.workspaces.is_empty() {
            self.workspaces.push(Workspace::new(
                home_dir().unwrap_or_default(),
                WorkspaceKind::Terminal,
            ));
            self.active_workspace = 0;
        }
        let w = self.active_workspace;
        let ws = &mut self.workspaces[w];
        let before = ws.active_tab;
        if ws.tabs.is_empty() || place == Place::Tab {
            ws.tabs.push(Tab::default());
            ws.active_tab = ws.tabs.len() - 1;
        }
        let t = ws.active_tab;
        match self.launch_into(w, t, None, command) {
            Some(_) => self.show_active(),
            // The new tab is gone again: back to the one shown before.
            None if place == Place::Tab => self.switch_tab(before),
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
        let app_id = self.prepare(original, &ws.dir).app_id;
        let queued = !app_id.is_empty();
        let pid = if queued {
            None
        } else {
            match self.start_app(w, t, original) {
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
        self.workspaces[w].tabs[t].expected.push(expected);
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
                let command = gerdr.workspaces[w].tabs[t].expected[i]
                    .app_command
                    .clone()
                    .unwrap_or_default();
                gerdr.start_app(w, t, &command)
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
    /// Ghostty or Alacritty, in the app for a URL, as a web app, or with a
    /// profile of gerdr's (see the module), which for Zed is the data dir
    /// of gerdr's Zed (see `Gerdr::launch_next_zed`).
    fn prepare(&self, command: &str, dir: &Path) -> Prepared {
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
            Some((web, site)) => {
                app_id = site;
                web
            }
            None => url.unwrap_or_else(|| command.to_owned()),
        };
        let bare_zed = opens_nothing_in_zed(&command);
        match own_instance(&command) {
            Some((mut command, profile)) => {
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

    /// Start `command` (trimmed) for tab `t` of workspace `w`, in the
    /// workspace's folder (see `prepare`). Its pid, or `None` if it is
    /// empty, the tab is gone, or it didn't start.
    pub(crate) fn start_app(&mut self, w: usize, t: usize, command: &str) -> Option<u32> {
        if command.is_empty() {
            return None;
        }
        let ws = self.workspaces.get(w)?;
        ws.tabs.get(t)?;
        let dir = ws.dir.clone();
        let prepared = self.prepare(command, &dir);
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
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
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
    // DevTools on a free port of localhost, to read back where each window
    // is (see `web_pages`); a browser already running keeps its own.
    Some((
        format!("{browser} --remote-debugging-port=0 --app={quoted}"),
        site,
    ))
}

/// The folder of the profile of gerdr's that the browser showing `command`
/// as a web app runs with (see `web_app` and `own_instance`); `None` when
/// `command` isn't a web page's link.
pub fn web_profile(command: &str) -> Option<PathBuf> {
    let opener = desktop::url_command(command)?;
    let (web, _) = web_app(command, &opener)?;
    own_instance(&web)?.1
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

/// Zed's data dir of gerdr's: with it, Zed runs its own instance here
/// instead of handing the windows to a Zed already running on the host.
/// One for all workspaces, so they run in one Zed process.
pub fn zed_data_dir(home: &Path) -> PathBuf {
    home.join(".local/share/gerdr/zed")
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

/// The program `command` starts (see `program`), to match commands to the
/// installed apps by.
pub fn program_name(command: &str) -> Option<String> {
    program(command).map(|(name, ..)| name)
}

/// `command` changed to start a copy of its own of an app that hands new
/// windows to a running one: a browser or a VS Code with gerdr's profile
/// for it, a Zed with gerdr's data dir (see `zed_data_dir`), and that
/// folder, or a Ghostty told not to (without a folder); `None` when
/// `command` doesn't start such an app or already picks its profile.
fn own_instance(command: &str) -> Option<(String, Option<PathBuf>)> {
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
        zed_data_dir(&home)
    } else {
        home.join(".local/share/gerdr/profiles").join(&key)
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
    let path = log_path(app);
    let open = || {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let big = std::fs::metadata(&path).is_ok_and(|m| m.len() > LOG_MAX);
        std::fs::OpenOptions::new()
            .create(true)
            .append(!big)
            .write(true)
            .truncate(big)
            .open(&path)
    };
    open()
        .inspect_err(|err| tracing::warn!("opening {}: {err}", path.display()))
        .ok()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        alacritty_command, ghostty_command, lineage, log_path, opens_nothing_in_zed, own_instance,
        web_app,
    };
    use crate::home_dir;

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
                format!("/usr/bin/brave-browser-stable --remote-debugging-port=0 --app='{url}'"),
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
    fn browsers_get_their_own_profile() {
        let home = home_dir().unwrap().to_string_lossy().into_owned();
        let run =
            |command: &str| own_instance(command).map(|(command, _)| command.replace(&home, "~"));
        assert_eq!(
            run("/usr/share/librewolf/librewolf").unwrap(),
            "/usr/share/librewolf/librewolf --name dev.gerdr.browser.librewolf --profile ~/.local/share/gerdr/profiles/librewolf --new-window"
        );
        assert_eq!(
            run("exec firefox https://a.b && true").unwrap(),
            "exec firefox --name dev.gerdr.browser.firefox --profile ~/.local/share/gerdr/profiles/firefox --new-window https://a.b && true"
        );
        assert_eq!(
            run("brave-browser-stable").unwrap(),
            "brave-browser-stable --user-data-dir=~/.local/share/gerdr/profiles/brave-browser-stable"
        );
        assert_eq!(
            run("/usr/bin/flatpak run --branch=stable --command=firefox org.mozilla.firefox")
                .unwrap(),
            "/usr/bin/flatpak run --branch=stable --command=firefox org.mozilla.firefox --name dev.gerdr.browser.org.mozilla.firefox --profile ~/.var/app/org.mozilla.firefox/gerdr-profile --new-window"
        );
        assert_eq!(
            run("/usr/share/cursor/cursor --new-window").unwrap(),
            "/usr/share/cursor/cursor --user-data-dir=~/.local/share/gerdr/profiles/cursor --new-window"
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
            "/home/me/.local/zed.app/bin/zed --user-data-dir=~/.local/share/gerdr/zed"
        );
        assert_eq!(
            run("zeditor -n .").unwrap(),
            "zeditor --user-data-dir=~/.local/share/gerdr/zed -n ."
        );
        assert_eq!(
            run("flatpak run dev.zed.Zed").unwrap(),
            "flatpak run dev.zed.Zed --user-data-dir=~/.var/app/dev.zed.Zed/gerdr-profile"
        );
        assert_eq!(run("zed --user-data-dir /x"), None);
        assert!(opens_nothing_in_zed("/home/me/.local/zed.app/bin/zed"));
        assert!(opens_nothing_in_zed("flatpak run dev.zed.Zed "));
        assert!(!opens_nothing_in_zed("zed -n ."));
        assert!(!opens_nothing_in_zed("zed src/main.rs"));
        assert!(!opens_nothing_in_zed("code"));
        assert_eq!(
            run("librewolf --private-window").unwrap(),
            "librewolf --name dev.gerdr.browser.librewolf --profile ~/.local/share/gerdr/profiles/librewolf --private-window"
        );
        assert_eq!(run("firefox --profile /x"), None);
        assert_eq!(run("nautilus ."), None);
        assert_eq!(run("flatpak run md.obsidian.Obsidian"), None);
    }
}

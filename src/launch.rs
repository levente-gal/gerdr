//! Apps launched into a split: any command, run by `sh` in the active
//! workspace's folder. The focused pane is split along its longer side, and
//! the first window from the command's processes takes the new half. The
//! apps named in the config file are offered in the switcher and the run
//! prompt; the run prompt also offers the apps installed on the system (see
//! `desktop`) and the commands typed before, and takes any command.
//!
//! Browsers keep one copy running per profile and hand new windows to it, so
//! one already open on the desktop would show the window there. gerdr runs
//! them with a profile of their own instead (see `own_instance`), kept
//! between runs: logins and bookmarks there are apart from the desktop's.
//!
//! Launched apps are not kept in herdr: they end with gerdr.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use smithay::reexports::calloop::channel;

use crate::{
    Expected, Gerdr, Tab, Workspace, WorkspaceKind, desktop, home_dir, icons::AppIcon, layout,
};

/// How many typed commands the run prompt remembers.
const HISTORY_LEN: usize = 100;

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
    /// Run `command` and show its window in a split of the focused pane.
    pub fn launch(&mut self, command: &str) {
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
        if ws.tabs.is_empty() {
            ws.tabs.push(Tab::default());
            ws.active_tab = 0;
        }
        let t = ws.active_tab;
        if self.launch_into(w, t, None, command).is_some() {
            self.show_active();
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
    pub fn launch_into(
        &mut self,
        w: usize,
        t: usize,
        leaf: Option<u32>,
        command: &str,
    ) -> Option<u32> {
        let original = command.trim();
        let Some(pid) = self.start_app(w, t, original) else {
            match leaf {
                Some(id) => {
                    let tab = self.workspaces.get_mut(w)?.tabs.get_mut(t)?;
                    layout::remove(&mut tab.layout, id);
                }
                None => self.prune(),
            }
            return None;
        };
        let id = match leaf {
            Some(id) => id,
            None => {
                let (id, area) = (self.new_pane_id(), self.content_rect());
                self.workspaces[w].tabs[t].split(None, id, None, area);
                id
            }
        };
        let mut expected = Expected::new(Some(pid), "", id, leaf.is_none());
        expected.app_command = Some(original.to_owned());
        self.workspaces[w].tabs[t].expected.push(expected);
        Some(id)
    }

    /// Start `command` (trimmed) for tab `t` of workspace `w`, in the
    /// workspace's folder. Its pid, or `None` if it is empty, the tab is
    /// gone, or it didn't start.
    fn start_app(&mut self, w: usize, t: usize, command: &str) -> Option<u32> {
        if command.is_empty() {
            return None;
        }
        let ws = self.workspaces.get(w)?;
        ws.tabs.get(t)?;
        let dir = ws.dir.clone();
        let command = match own_instance(command) {
            Some((command, profile)) => {
                if let Err(err) = std::fs::create_dir_all(&profile) {
                    tracing::warn!("creating {}: {err}", profile.display());
                }
                command
            }
            None => command.to_owned(),
        };
        let mut cmd = Command::new("sh");
        // Electron apps would look for an X server, and there is none.
        cmd.arg("-c")
            .arg(&command)
            .env("ELECTRON_OZONE_PLATFORM_HINT", "auto");
        self.spawn(cmd, &dir)
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

/// The program `command` starts (see `program`), to match commands to the
/// installed apps by.
pub fn program_name(command: &str) -> Option<String> {
    program(command).map(|(name, ..)| name)
}

/// `command` changed to start a browser of its own, with gerdr's profile
/// for it, and that profile's folder; `None` when `command` doesn't start a
/// known browser or already picks its profile.
fn own_instance(command: &str) -> Option<(String, PathBuf)> {
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
    let (key, end, flatpak) = program(command)?;
    let home = home_dir()?;
    // A Flatpak app sees only its own folder.
    let profile = if flatpak {
        home.join(".var/app").join(&key).join("gerdr-profile")
    } else {
        home.join(".local/share/gerdr/profiles").join(&key)
    };
    let quoted = desktop::shell_quote(&profile.to_string_lossy());
    let flags = if FIREFOXES.contains(&key.as_str()) {
        format!(" --new-instance --profile {quoted}")
    } else if CHROMIUMS.contains(&key.as_str()) {
        format!(" --user-data-dir={quoted}")
    } else {
        return None;
    };
    let mut command = command.to_owned();
    command.insert_str(end, &flags);
    Some((command, profile))
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

#[cfg(test)]
mod tests {
    use super::{lineage, own_instance};
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
    fn browsers_get_their_own_profile() {
        let home = home_dir().unwrap().to_string_lossy().into_owned();
        let run =
            |command: &str| own_instance(command).map(|(command, _)| command.replace(&home, "~"));
        assert_eq!(
            run("/usr/share/librewolf/librewolf").unwrap(),
            "/usr/share/librewolf/librewolf --new-instance --profile ~/.local/share/gerdr/profiles/librewolf"
        );
        assert_eq!(
            run("exec firefox https://a.b && true").unwrap(),
            "exec firefox --new-instance --profile ~/.local/share/gerdr/profiles/firefox https://a.b && true"
        );
        assert_eq!(
            run("brave-browser-stable").unwrap(),
            "brave-browser-stable --user-data-dir=~/.local/share/gerdr/profiles/brave-browser-stable"
        );
        assert_eq!(
            run("/usr/bin/flatpak run --branch=stable --command=firefox org.mozilla.firefox")
                .unwrap(),
            "/usr/bin/flatpak run --branch=stable --command=firefox org.mozilla.firefox --new-instance --profile ~/.var/app/org.mozilla.firefox/gerdr-profile"
        );
        assert_eq!(run("firefox --profile /x"), None);
        assert_eq!(run("nautilus ."), None);
        assert_eq!(run("flatpak run md.obsidian.Obsidian"), None);
    }
}

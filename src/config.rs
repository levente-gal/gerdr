//! gerdr's config file, `$XDG_CONFIG_HOME/gerdr/config.toml` (by default
//! `~/.config/gerdr/config.toml`): the shortcuts (see `keys`) and the apps
//! offered to launch into a split (see `launch`). Without the file, gerdr
//! uses the default keys and offers no apps.
//!
//! ```toml
//! [keys]
//! prefix = "Ctrl+A"
//!
//! [apps]
//! # Run by `sh` in the workspace folder.
//! browser = "firefox http://localhost:3000"
//! files = "nautilus ."
//!
//! [ui]
//! density = "compact"   # or "comfortable", the default
//! theme = "light"       # or "dark" or "system"; else the last choice
//! needs_you = false     # hide the sidebar's "Needs you" list; shown by default
//! motion = "reduced"    # or "off": no blinking dots or pulses; "full", the default
//! notify = false        # no desktop notifications; on by default
//!
//! [git]
//! # Branch names that are one branch across repositories, in the
//! # repositories panel's branch picker. By default main and master.
//! same_branches = [["main", "master"], ["production", "prod"]]
//! # Branches listed first in its branch lists, in this order, after the
//! # default branch; the rest go newest first. By default main, master,
//! # dev and develop.
//! top_branches = ["main", "dev", "staging"]
//!
//! [agents]
//! # The agent CLIs the AI button of a workspace starts in a new tab: the
//! # known ones on PATH (claude, codex, gemini, cursor-agent, opencode…),
//! # each run by its own command unless given another here ("" hides it),
//! # and the ones added here, by name.
//! claude = "claude --dangerously-skip-permissions"
//! my-agent = "my-agent --fast"
//!
//! [keep]
//! # Apps that keep running when gerdr quits, as terminals do, by program:
//! # gerdr shows them again when it starts. Needs wprs (wprsd and wprsc) on
//! # PATH. By default Cursor; [] keeps none.
//! apps = ["cursor"]
//!
//! [inbox]
//! # Linear's issues assigned to you show in the inbox's todos with an API
//! # key; LINEAR_API_KEY in the environment wins over this.
//! linear_api_key = "lin_api_…"
//! # Only these teams' issues, by key. By default every team's.
//! linear_teams = ["ENG"]
//! ```
//!
//! Each table is read on its own, and a key that can't be read on its
//! own too: a mistake spoils only that key (or, when the file isn't TOML at
//! all, the file), and the defaults stand in for what it spoils. Mistakes
//! go to the problems list (see `problems`) and the log, and gerdr says so
//! with an error toast. gerdr reads the file again when it is saved (see
//! `Gerdr::watch_settings`).

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use serde::{Deserialize, de::DeserializeOwned};

use smithay::reexports::calloop::timer::{TimeoutAction, Timer};

use crate::{
    Gerdr,
    anim::Motion,
    chrome::{Density, ThemeMode},
    home_dir,
    keys::{Keymap, KeysConfig},
    launch::App,
    menu::MenuAction,
    toast::Level,
};

/// The file's tables, as written; each read on its own (see `read`).
#[derive(Default)]
struct Config {
    keys: KeysConfig,
    /// Commands by name.
    apps: BTreeMap<String, String>,
    ui: UiConfig,
    git: GitConfig,
    keep: KeepConfig,
    inbox: InboxConfig,
    /// Agent CLI commands by name (see `cli_agents`).
    agents: BTreeMap<String, String>,
}

/// How a problem of the `[keys]` table starts.
const KEYS_PROBLEM: &str = "[keys] ";

/// The tables the file may have.
const TABLES: [&str; 7] = ["keys", "apps", "ui", "git", "keep", "inbox", "agents"];

impl Config {
    /// The tables of `text`, each read on its own: one that can't be read
    /// whole is read key by key (see `table`). What is wrong goes to
    /// `problems`, as "[table] key: why".
    fn read(text: &str, problems: &mut Vec<String>) -> Self {
        let mut file: toml::Table = match text.parse() {
            Ok(file) => file,
            Err(err) => {
                problems.push(err.message().to_owned() + &line_of(text, err.span()));
                return Self::default();
            }
        };
        for name in file.keys().filter(|name| !TABLES.contains(&name.as_str())) {
            problems.push(format!("[{name}] isn't a table gerdr reads"));
        }
        let mut take = |name: &str| file.remove(name);
        let (keys, apps, ui, git) = (take("keys"), take("apps"), take("ui"), take("git"));
        let (keep, inbox, agents) = (take("keep"), take("inbox"), take("agents"));
        Self {
            keys: table("keys", keys, problems),
            apps: table("apps", apps, problems),
            ui: table("ui", ui, problems),
            git: table("git", git, problems),
            keep: table("keep", keep, problems),
            inbox: table("inbox", inbox, problems),
            agents: table("agents", agents, problems),
        }
    }
}

/// " (line N)" for where `span` starts in `text`, if it says.
fn line_of(text: &str, span: Option<std::ops::Range<usize>>) -> String {
    span.map_or_else(String::new, |span| {
        let line = text[..span.start.min(text.len())].lines().count().max(1);
        format!(" (line {line})")
    })
}

/// Table `name` of the file, `value`, as `T`: whole when it reads so,
/// else key by key, each key that doesn't read on its own reported to
/// `problems` and left out; the default without it.
fn table<T: DeserializeOwned + Default>(
    name: &str,
    value: Option<toml::Value>,
    problems: &mut Vec<String>,
) -> T {
    let Some(value) = value else {
        return T::default();
    };
    if let Ok(read) = T::deserialize(value.clone()) {
        return read;
    }
    let toml::Value::Table(keys) = value else {
        problems.push(format!("[{name}] isn't a table"));
        return T::default();
    };
    let mut good = toml::Table::new();
    for (key, value) in keys {
        let alone = toml::Table::from_iter([(key.clone(), value.clone())]);
        match T::deserialize(toml::Value::Table(alone)) {
            Ok(_) => {
                good.insert(key, value);
            }
            Err(err) => problems.push(format!("[{name}] {key}: {}", err.message().trim())),
        }
    }
    T::deserialize(toml::Value::Table(good)).unwrap_or_default()
}

/// The `[ui]` table, as written.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct UiConfig {
    density: Option<String>,
    theme: Option<String>,
    needs_you: Option<bool>,
    motion: Option<String>,
    notify: Option<bool>,
}

/// The `[git]` table, as written.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct GitConfig {
    same_branches: Option<Vec<Vec<String>>>,
    top_branches: Option<Vec<String>>,
}

/// The `[keep]` table, as written.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct KeepConfig {
    apps: Option<Vec<String>>,
}

/// The programs whose apps keep running when gerdr quits, by default.
const KEEP_APPS: [&str; 1] = ["cursor"];

/// The `[inbox]` table, as written.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct InboxConfig {
    linear_api_key: Option<String>,
    linear_teams: Option<Vec<String>>,
}

/// What the inbox reads Linear with, from the `[inbox]` table.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InboxSettings {
    /// Linear's API key in the file; the environment's wins (see `linear`).
    pub linear_api_key: Option<String>,
    /// The teams whose issues to list, by key; empty for every team's.
    pub linear_teams: Vec<String>,
}

impl InboxSettings {
    /// How to read Linear, with `env_key` (`LINEAR_API_KEY`) over the
    /// file's key; `None` without either.
    pub fn linear(self, env_key: Option<String>) -> Option<Linear> {
        let key = env_key
            .map(|key| key.trim().to_owned())
            .filter(|key| !key.is_empty())
            .or(self.linear_api_key)?;
        Some(Linear {
            key,
            teams: self.linear_teams,
        })
    }
}

/// How to read the Linear issues assigned to the user.
#[derive(Clone, PartialEq, Eq)]
pub struct Linear {
    /// The API key: never logged nor put on a command line.
    pub key: String,
    /// The teams whose issues to list, by key; empty for every team's.
    pub teams: Vec<String>,
}

impl std::fmt::Debug for Linear {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Linear")
            .field("key", &"…")
            .field("teams", &self.teams)
            .finish()
    }
}

/// How git branches are seen, from the `[git]` table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Git {
    /// Groups of branch names that are one branch: a repository's `master`
    /// is another's `main`.
    pub same_branches: Vec<Vec<String>>,
    /// Branches the branch lists show first, in this order, after the
    /// default branch.
    pub top_branches: Vec<String>,
}

impl Default for Git {
    fn default() -> Self {
        Git {
            same_branches: vec![vec!["main".into(), "master".into()]],
            top_branches: ["main", "master", "dev", "develop"]
                .map(String::from)
                .into(),
        }
    }
}

/// How the chrome looks, from the `[ui]` table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ui {
    pub density: Density,
    /// The theme named in the file; `None` leaves it to the last choice.
    pub theme: Option<ThemeMode>,
    /// Whether the sidebar gathers the blocked and done agents of the
    /// other workspaces in a "Needs you" list at its top.
    pub needs_you: bool,
    /// How much the chrome moves (see `anim::Motion`).
    pub motion: Motion,
    /// Whether agents that need the user, and finished test runs, say so
    /// with a desktop notification (see `notify`).
    pub notify: bool,
}

impl Default for Ui {
    fn default() -> Self {
        Ui {
            density: Density::default(),
            theme: None,
            needs_you: true,
            motion: Motion::Full,
            notify: true,
        }
    }
}

/// Everything the config file sets, as `load` reads it.
pub struct Settings {
    pub keymap: Keymap,
    pub apps: Vec<App>,
    pub ui: Ui,
    pub git: Git,
    pub linear: Option<Linear>,
    /// The programs whose apps keep running (see `keep`).
    pub keep: Vec<String>,
    /// The agent CLI commands by name (see `cli_agents`).
    pub agents: BTreeMap<String, String>,
    /// What is wrong with the file, as sentences for the user.
    pub problems: Vec<String>,
}

/// The shortcuts, apps, look and git settings in the config file, how
/// the inbox reads Linear (`LINEAR_API_KEY` in the environment over the
/// file's key), the programs whose apps keep running (see `keep`) and the
/// agent CLI commands by name (see `cli_agents`). What is wrong with the
/// file is kept in the problems list under `"config"` (and logged), in
/// place of what was there before.
pub fn load() -> Settings {
    let path = config_file();
    let mut problems = Vec::new();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) => {
            if err.kind() != std::io::ErrorKind::NotFound {
                problems.push(format!("can't be read: {err}"));
            }
            String::new()
        }
    };
    let (keymap, apps, ui, git, inbox, found, keep, agents) = from_text(&text);
    problems.extend(found);
    // `Keymap::new` keeps its own under "keys": only the rest here.
    crate::problems::clear("config");
    for problem in problems.iter().filter(|p| !p.starts_with(KEYS_PROBLEM)) {
        crate::problems::report("config", format!("Settings: {problem}"));
    }
    let linear = inbox.linear(std::env::var("LINEAR_API_KEY").ok());
    Settings {
        keymap,
        apps,
        ui,
        git,
        linear,
        keep,
        agents,
        problems,
    }
}

/// When the config file was last written, if it is there.
pub fn modified() -> Option<SystemTime> {
    std::fs::metadata(config_file()).ok()?.modified().ok()
}

/// The shortcuts, apps, look, git and inbox settings `text` describes,
/// what's wrong with it, the programs whose apps keep running and the
/// agent CLI commands by name.
#[allow(clippy::type_complexity)]
pub fn from_text(
    text: &str,
) -> (
    Keymap,
    Vec<App>,
    Ui,
    Git,
    InboxSettings,
    Vec<String>,
    Vec<String>,
    BTreeMap<String, String>,
) {
    let mut problems = Vec::new();
    let config = Config::read(text, &mut problems);
    let (keymap, keys_problems) = Keymap::new(config.keys);
    problems.extend(
        keys_problems
            .into_iter()
            .map(|p| format!("{KEYS_PROBLEM}{p}")),
    );
    let mut apps = Vec::new();
    for (name, command) in config.apps {
        match command.trim() {
            "" => problems.push(format!("app `{name}` has no command")),
            command => apps.push(App {
                name,
                command: command.into(),
                detail: command.into(),
                icon: None,
            }),
        }
    }
    let mut ui = Ui::default();
    match config.ui.density.as_deref().map(str::trim) {
        None | Some("comfortable") => {}
        Some("compact") => ui.density = Density::Compact,
        Some(other) => problems.push(format!("unknown density `{other}`")),
    }
    match config.ui.motion.as_deref().map(str::trim) {
        None | Some("full") => {}
        Some("reduced") => ui.motion = Motion::Reduced,
        Some("off") => ui.motion = Motion::Off,
        Some(other) => problems.push(format!("unknown motion `{other}`")),
    }
    if let Some(theme) = config.ui.theme.as_deref() {
        match ThemeMode::from_name(theme) {
            Some(mode) => ui.theme = Some(mode),
            None => problems.push(format!("unknown theme `{theme}`")),
        }
    }
    if let Some(needs_you) = config.ui.needs_you {
        ui.needs_you = needs_you;
    }
    if let Some(notify) = config.ui.notify {
        ui.notify = notify;
    }
    let mut git = Git::default();
    if let Some(groups) = config.git.same_branches {
        git.same_branches = same_branches(groups, &mut problems);
    }
    if let Some(names) = config.git.top_branches {
        git.top_branches = names
            .into_iter()
            .map(|name| name.trim().to_owned())
            .filter(|name| {
                let blank = name.is_empty();
                if blank {
                    problems.push("a blank branch name in `top_branches`".into());
                }
                !blank
            })
            .collect();
    }
    let inbox = InboxSettings {
        linear_api_key: config
            .inbox
            .linear_api_key
            .map(|key| key.trim().to_owned())
            .filter(|key| !key.is_empty()),
        linear_teams: config
            .inbox
            .linear_teams
            .unwrap_or_default()
            .into_iter()
            .map(|team| team.trim().to_owned())
            .filter(|team| !team.is_empty())
            .collect(),
    };
    let keep = match config.keep.apps {
        Some(names) => names
            .into_iter()
            .map(|name| name.trim().to_owned())
            .filter(|name| {
                let blank = name.is_empty();
                if blank {
                    problems.push("a blank program name in `[keep] apps`".into());
                }
                !blank
            })
            .collect(),
        None => KEEP_APPS.map(String::from).into(),
    };
    (keymap, apps, ui, git, inbox, problems, keep, config.agents)
}

/// The groups of branch names as written, without the blank names, the
/// names already in a group and the groups left with fewer than two.
fn same_branches(groups: Vec<Vec<String>>, problems: &mut Vec<String>) -> Vec<Vec<String>> {
    let mut seen = std::collections::HashSet::new();
    let mut kept = Vec::new();
    for group in groups {
        let mut names = Vec::new();
        for name in group {
            let name = name.trim().to_owned();
            if name.is_empty() {
                problems.push("a blank branch name in `same_branches`".into());
            } else if !seen.insert(name.clone()) {
                problems.push(format!("branch `{name}` is in two `same_branches` groups"));
            } else {
                names.push(name);
            }
        }
        match names.len() {
            0 => {}
            1 => problems.push(format!(
                "branch `{}` has no other name in `same_branches`",
                names[0]
            )),
            _ => kept.push(names),
        }
    }
    kept
}

/// What a config file made for the user to edit starts with: all of it
/// commented out, so it changes nothing yet.
const TEMPLATE: &str = "\
# gerdr's settings, read again each time this file is saved. A mistake
# spoils only its own line: gerdr says what is wrong, and the default
# stands in for it.
#
# [keys]
# # A key is modifiers (Ctrl, Alt, Shift, Super) and a key name joined by
# # \"+\": \"Shift+Tab\", \"F12\", \"Down\". Letters are the same either case,
# # so \"Shift+X\" needs its Shift. A character typed with Shift (\"?\",
# # \"!\", \"@\") is Shift and the key it is on in the keyboard's layout;
# # one the layout types only with AltGr is a mistake: name the key it is
# # on instead. Ctrl still held from the prefix doesn't count: a key
# # with Ctrl in it takes pressing Ctrl again. An action given keys
# # loses its default ones; [] leaves it none. The prefix is Ctrl+B by
# # default. open-workspace was called open-project, and new-agent
# # new-cursor-agent: the old names still work, with a word that they
# # changed.
# prefix = \"Ctrl+A\"
# new-tab = \"t\"
# toggle-theme = [\"Alt+T\", \"F12\"]
# open-workspace = \"o\"
# rename-tab = [\",\", \"F2\"]
#
# [apps]
# # Run by `sh` in the workspace folder.
# browser = \"firefox http://localhost:3000\"
# files = \"nautilus .\"
#
# [ui]
# density = \"compact\"   # or \"comfortable\", the default
# theme = \"light\"       # or \"dark\" or \"system\"; else the last choice
# needs_you = false     # hide the sidebar's \"Needs you\" list; shown by default
# motion = \"reduced\"    # or \"off\": no blinking dots or pulses; \"full\", the default
# notify = false        # no desktop notifications; on by default
#
# [git]
# # Branch names that are one branch across repositories, in the
# # repositories panel's branch picker. By default main and master.
# same_branches = [[\"main\", \"master\"], [\"production\", \"prod\"]]
# # Branches listed first in its branch lists, in this order, after the
# # default branch; the rest go newest first. By default main, master,
# # dev and develop.
# top_branches = [\"main\", \"dev\", \"staging\"]
#
# [agents]
# # The agent CLIs the AI button of a workspace starts in a new tab: the
# # known ones on PATH (claude, codex, gemini, cursor-agent, opencode…),
# # each run by its own command unless given another here (\"\" hides it),
# # and the ones added here, by name. Each runs in a new terminal tab of
# # the workspace, in its folder, as typed at its shell.
# claude = \"claude --dangerously-skip-permissions\"
# my-agent = \"my-agent --fast\"
#
# [keep]
# # Apps that keep running when gerdr quits, as terminals do, by program:
# # gerdr shows them again when it starts. Needs wprs (wprsd and wprsc) on
# # PATH. By default Cursor; [] keeps none.
# apps = [\"cursor\"]
#
# [inbox]
# # Linear's issues assigned to you show in the inbox's todos with an API
# # key; LINEAR_API_KEY in the environment wins over this.
# linear_api_key = \"lin_api_…\"
# # Only these teams' issues, by key. By default every team's.
# linear_teams = [\"ENG\"]
";

/// Make the config file at `path` from `TEMPLATE`, with its folder, unless
/// there is one.
pub fn ensure_file(path: &Path) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => std::io::Write::write_all(&mut file, TEMPLATE.as_bytes()),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(err) => Err(err),
    }
}

/// How often gerdr looks whether the config file was saved.
const WATCH_INTERVAL: Duration = Duration::from_secs(2);

impl Gerdr {
    /// Read the config file again whenever it is saved (see
    /// `reload_settings`).
    pub fn watch_settings(&mut self) {
        let timer = Timer::from_duration(WATCH_INTERVAL);
        let look = |_, _: &mut (), state: &mut Gerdr| {
            let modified = modified();
            if modified != state.settings_modified {
                state.settings_modified = modified;
                state.reload_settings();
            }
            TimeoutAction::ToDuration(WATCH_INTERVAL)
        };
        if let Err(err) = self.loop_handle.insert_source(timer, look) {
            tracing::warn!("watching the settings: {err}");
        }
    }

    /// Read the config file again and use what it says now: the keys,
    /// apps, look, git, inbox, kept apps, agents and notifications. Says
    /// so, with what is wrong with it.
    pub fn reload_settings(&mut self) {
        let settings = load();
        self.keymap = settings.keymap;
        self.apps = settings.apps;
        self.density = settings.ui.density;
        self.needs_you = settings.ui.needs_you;
        crate::anim::set_motion(settings.ui.motion);
        if let Some(mode) = settings.ui.theme.filter(|mode| *mode != self.theme_mode) {
            self.set_theme_mode(mode);
        }
        self.repos.same_branches = settings.git.same_branches;
        self.repos.top_branches = settings.git.top_branches;
        self.inbox.linear = settings.linear;
        self.keep_apps = settings.keep;
        self.cli_agents = crate::cli_agents::installed(&settings.agents);
        self.agent_config = settings.agents;
        if settings.ui.notify != self.notify {
            self.notify = settings.ui.notify;
            // The bus source lets go once the notifier is gone.
            match self.notify {
                true => self.connect_notifier(),
                false => self.notifier = None,
            }
        }
        self.arrange();
        self.settings_problems(&settings.problems, true);
    }

    /// Say what is wrong with the config file, `problems`, in an error
    /// toast with a button opening it; once `reloaded`, that it was read
    /// again, either way.
    pub fn settings_problems(&mut self, problems: &[String], reloaded: bool) {
        let read = if reloaded {
            "Settings reloaded"
        } else {
            "Settings"
        };
        let text = match problems {
            [] if reloaded => return self.show_success(read.into()),
            [] => return,
            [one] => format!("{read}: {one}"),
            [first, rest @ ..] => {
                format!("{read}: {first} \u{b7} {} more in the log", rest.len())
            }
        };
        let open = ("Open settings".to_owned(), MenuAction::OpenSettings);
        self.toast_with(text, Level::Error, Some(open));
    }
}

/// gerdr's config file: `$XDG_CONFIG_HOME/gerdr/config.toml`, by default
/// `~/.config/gerdr/config.toml`.
pub fn config_file() -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| home_dir().unwrap_or_default().join(".config"));
    config.join("gerdr/config.toml")
}

#[cfg(test)]
mod tests {
    use super::{Density, Git, InboxSettings, TEMPLATE, ThemeMode, Ui, from_text};

    #[test]
    fn the_template_changes_nothing() {
        let (_, apps, ui, git, inbox, problems, ..) = from_text(TEMPLATE);
        assert!(problems.is_empty(), "{problems:?}");
        assert!(apps.is_empty());
        assert_eq!(ui, Ui::default());
        assert_eq!(git, Git::default());
        assert_eq!(inbox, InboxSettings::default());
        // Each example, uncommented, is understood.
        let examples: String = TEMPLATE
            .lines()
            .filter(|line| !line.starts_with("# ") || line.contains('=') || line.contains('['))
            .map(|line| line.strip_prefix("# ").unwrap_or(line))
            .filter(|line| !line.starts_with('#'))
            .map(|line| format!("{line}\n"))
            .collect();
        let (_, apps, ui, git, inbox, problems, _, agents) = from_text(&examples);
        assert!(problems.is_empty(), "{problems:?}\n{examples}");
        assert_eq!(agents["my-agent"], "my-agent --fast");
        assert_eq!(apps.len(), 2);
        assert_eq!(ui.density, Density::Compact);
        assert_eq!(git.same_branches.len(), 2);
        assert_eq!(git.top_branches, ["main", "dev", "staging"]);
        assert_eq!(inbox.linear_teams, ["ENG"]);
        assert!(inbox.linear_api_key.is_some());
    }

    #[test]
    fn apps() {
        let (_, apps, _, _, _, problems, ..) =
            from_text("[apps]\nfiles = \"nautilus .\"\nbrowser = \" firefox \"\nnone = \"\"\n");
        let apps: Vec<_> = apps
            .iter()
            .map(|app| (app.name.as_str(), app.command.as_str()))
            .collect();
        assert_eq!(apps, [("browser", "firefox"), ("files", "nautilus .")]);
        assert_eq!(problems, ["app `none` has no command"]);
    }

    #[test]
    fn keep_apps() {
        assert_eq!(from_text("").6, ["cursor"]);
        assert!(from_text("[keep]\napps = []\n").6.is_empty());
        let (.., problems, keep, _) = from_text("[keep]\napps = [\" code \", \"\"]\n");
        assert_eq!(keep, ["code"]);
        assert_eq!(problems, ["a blank program name in `[keep] apps`"]);
    }

    #[test]
    fn mistakes() {
        assert_eq!(from_text("[key]\n").5, ["[key] isn't a table gerdr reads"]);
        assert_eq!(from_text("[apps]\nbrowser = 1\n").5.len(), 1);
        assert_eq!(from_text("[inbox]\nlinear = 1\n").5.len(), 1);
        // Not TOML at all: where it goes wrong.
        let problems = from_text("[ui]\ndensity = \"compact\"\ntheme = \n").5;
        assert_eq!(problems.len(), 1);
        assert!(problems[0].ends_with("(line 3)"), "{problems:?}");
    }

    #[test]
    fn a_mistake_spoils_only_its_own_key() {
        let text = "[keys]\nprefix = \"Ctrl+A\"\n\
                    [apps]\nbrowser = 1\nfiles = \"nautilus .\"\n\
                    [ui]\ndensty = \"compact\"\ntheme = \"light\"\n\
                    [agents]\nmine = \"mine --fast\"\n";
        let (keymap, apps, ui, _, _, problems, _, agents) = from_text(text);
        assert_eq!(keymap.help().prefix, "Ctrl+A");
        assert_eq!(apps.len(), 1);
        assert_eq!(ui.theme, Some(ThemeMode::Light));
        assert_eq!(agents["mine"], "mine --fast");
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].starts_with("[apps] browser: "), "{problems:?}");
        assert!(problems[1].starts_with("[ui] densty: "), "{problems:?}");
    }

    fn ui(text: &str) -> (Ui, Vec<String>) {
        let (_, _, ui, _, _, problems, ..) = from_text(text);
        (ui, problems)
    }

    fn same(text: &str) -> (Vec<Vec<String>>, Vec<String>) {
        let (_, _, _, git, _, problems, ..) = from_text(text);
        (git.same_branches, problems)
    }

    #[test]
    fn main_and_master_are_one_branch_unless_the_file_says_otherwise() {
        let main = vec![vec!["main".to_owned(), "master".to_owned()]];
        assert_eq!(same(""), (main.clone(), vec![]));
        assert_eq!(same("[git]\n"), (main, vec![]));
        let (groups, problems) = same("[git]\nsame_branches = [[\"production\", \" prod \"]]\n");
        assert_eq!(groups, [["production", "prod"]]);
        assert!(problems.is_empty());
        // None at all, if the file says so.
        assert_eq!(same("[git]\nsame_branches = []\n"), (vec![], vec![]));
    }

    #[test]
    fn main_master_dev_and_develop_go_first_unless_the_file_says_otherwise() {
        let top = |text: &str| {
            let (_, _, _, git, _, problems, ..) = from_text(text);
            (git.top_branches, problems)
        };
        assert_eq!(top("").0, ["main", "master", "dev", "develop"]);
        let (names, problems) = top("[git]\ntop_branches = [\" trunk \", \"\"]\n");
        assert_eq!(names, ["trunk"]);
        assert_eq!(problems, ["a blank branch name in `top_branches`"]);
    }

    #[test]
    fn a_wrong_branch_group_is_reported_and_left_out() {
        let (groups, problems) = same(
            "[git]\nsame_branches = [[\"main\", \"master\", \"\"], [\"master\", \"trunk\"], [\"dev\"]]\n",
        );
        assert_eq!(groups, [["main", "master"]]);
        assert_eq!(
            problems,
            [
                "a blank branch name in `same_branches`",
                "branch `master` is in two `same_branches` groups",
                "branch `trunk` has no other name in `same_branches`",
                "branch `dev` has no other name in `same_branches`",
            ]
        );
    }

    #[test]
    fn density_is_comfortable_unless_the_file_says_compact() {
        assert_eq!(ui("").0, Ui::default());
        assert_eq!(Ui::default().density, Density::Comfortable);
        let (parsed, problems) = ui("[ui]\ndensity = \"compact\"\n");
        assert_eq!(parsed.density, Density::Compact);
        assert!(problems.is_empty());
        let (parsed, problems) = ui("[ui]\ndensity = \" comfortable \"\n");
        assert_eq!(parsed.density, Density::Comfortable);
        assert!(problems.is_empty());
        // The density alone leaves the theme to the last choice.
        assert_eq!(parsed.theme, None);
    }

    #[test]
    fn the_theme_names_a_mode_or_is_left_to_the_last_choice() {
        assert_eq!(ui("").0.theme, None);
        for (name, mode) in [
            ("system", ThemeMode::System),
            ("dark", ThemeMode::Dark),
            ("light", ThemeMode::Light),
        ] {
            let (parsed, problems) = ui(&format!("[ui]\ntheme = \"{name}\"\n"));
            assert_eq!(parsed.theme, Some(mode));
            assert_eq!(parsed.density, Density::Comfortable);
            assert!(problems.is_empty());
        }
        let (parsed, problems) = ui("[ui]\ndensity = \"compact\"\ntheme = \"light\"\n");
        assert_eq!(
            parsed,
            Ui {
                density: Density::Compact,
                theme: Some(ThemeMode::Light),
                needs_you: true,
                motion: crate::anim::Motion::Full,
                notify: true,
            }
        );
        assert!(problems.is_empty());
    }

    #[test]
    fn the_needs_you_list_shows_unless_the_file_hides_it() {
        assert!(ui("").0.needs_you);
        let (parsed, problems) = ui("[ui]\nneeds_you = false\n");
        assert!(!parsed.needs_you);
        assert!(problems.is_empty());
    }

    #[test]
    fn linear_is_read_with_the_environments_key_over_the_files() {
        let settings = |text: &str| from_text(text).4;
        assert_eq!(settings("").linear(None), None);
        assert_eq!(
            settings("[inbox]\nlinear_api_key = \" \"\n").linear(None),
            None
        );
        let file =
            settings("[inbox]\nlinear_api_key = \"from-file\"\nlinear_teams = [\"ENG\", \"\"]\n");
        let linear = file.clone().linear(None).expect("the file's key");
        assert_eq!(
            (linear.key.as_str(), linear.teams),
            ("from-file", vec!["ENG".to_owned()])
        );
        let linear = file.clone().linear(Some("from-env".into())).expect("a key");
        assert_eq!(linear.key, "from-env");
        // A blank one in the environment leaves the file's.
        assert_eq!(
            file.linear(Some(" ".into())).map(|l| l.key),
            Some("from-file".into())
        );
        assert_eq!(
            settings("").linear(Some("k".into())).map(|l| l.teams),
            Some(vec![])
        );
        // The key stays out of logs.
        let linear = settings("").linear(Some("secret".into())).unwrap();
        assert!(!format!("{linear:?}").contains("secret"));
    }

    #[test]
    fn a_wrong_ui_value_is_reported_and_the_default_stands() {
        let (parsed, problems) = ui("[ui]\ndensity = \"cosy\"\n");
        assert_eq!(parsed, Ui::default());
        assert_eq!(problems, ["unknown density `cosy`"]);
        let (parsed, problems) = ui("[ui]\ntheme = \"blue\"\n");
        assert_eq!(parsed, Ui::default());
        assert_eq!(problems, ["unknown theme `blue`"]);
        let (parsed, problems) = ui("[ui]\ndensity = \"cosy\"\ntheme = \"blue\"\n");
        assert_eq!(parsed, Ui::default());
        assert_eq!(problems.len(), 2);
        // A wrong type, or a key that isn't there, spoils only that key.
        let (parsed, problems) = ui("[ui]\ndensity = 1\n");
        assert_eq!(parsed, Ui::default());
        assert_eq!(problems.len(), 1);
        let (parsed, problems) = ui("[ui]\ncolour = \"red\"\ntheme = \"dark\"\n");
        assert_eq!(parsed.theme, Some(ThemeMode::Dark));
        assert_eq!(problems.len(), 1);
    }
}

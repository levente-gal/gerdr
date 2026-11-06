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
//! footer = false        # hide the footer with the background tasks; shown by default
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
//! # and the ones added here, by name. T3 Code, once it ran here, starts
//! # a new thread in its app in a web pane; t3 = "" hides it.
//! claude = "claude --dangerously-skip-permissions"
//! my-agent = "my-agent --fast"
//!
//! [machines]
//! # Other machines gerdr reaches over SSH, by name: "Connect a machine" in
//! # the launcher adds them here. gerdr connects with your SSH keys. A
//! # feature stage's page picks the machine it runs on; a project's
//! # file gives its stages one under "machines", and folders under
//! # "machine_folders".
//! build-box = "ana@10.0.0.5"
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
//!
//! [services]
//! # Output lines of the services (web servers, workers) taken for errors,
//! # as regexes, in place of the default ones (tracebacks, panics, ERROR,
//! # CRITICAL and FATAL, 5xx answers in access logs…).
//! errors = ['Traceback \(most recent call last\)', '\bERROR\b']
//! # More, besides the default ones (or those of `errors`).
//! extra_errors = ['level=error']
//! # Lines never taken for errors, though they match: never shown. To
//! # keep an error in sight but quiet, ignore it in the inbox (Ctrl+I).
//! ignore = ['DeprecationWarning']
//! # Desktop notifications of the errors; on by default ([notifications]
//! # enabled = false turns them off too).
//! notify = false
//! # Tell when a service's command stops by itself; on by default.
//! stopped = false
//!
//! [t3]
//! # Take each newer T3 Code that gerdr reads as soon as it is out
//! # (checked every 6 hours); false updates it only when asked. On by
//! # default. The launcher's row turning it on or off writes this key.
//! updates = false
//!
//! [terminal]
//! # Whether each Alacritty gerdr starts reloads its config file when it
//! # changes, which takes one of the user's inotify instances (128 by
//! # default, and Zed needs them too). With false, new terminals read the
//! # file once and take none; open ones let go of theirs when gerdr
//! # restarts. On by default. The launcher's row turning it on or off
//! # writes this key.
//! live_config_reload = false
//!
//! [notifications]
//! # Desktop notifications: of agents that need you, finished test runs and
//! # the services' errors. On by default.
//! enabled = false
//! # Seconds gerdr's desktop notifications stay on screen; 0 keeps them
//! # until dismissed, -1 leaves it to the notification daemon. By default 30.
//! timeout = 0
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
    service_errors::{DEFAULT_ERRORS, Patterns},
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
    /// Other machines' SSH targets by name (see `machines`).
    machines: BTreeMap<String, String>,
    services: ServicesConfig,
    t3: T3Config,
    terminal: TerminalConfig,
    notifications: NotificationsConfig,
}

/// The `[t3]` table, as written.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct T3Config {
    updates: Option<bool>,
}

/// How T3 Code is kept up to date, from the `[t3]` table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct T3Settings {
    /// Whether each newer version gerdr reads is taken as soon as it is
    /// out, instead of only when asked (see `t3_update`).
    pub updates: bool,
}

impl Default for T3Settings {
    fn default() -> Self {
        Self { updates: true }
    }
}

/// The `[terminal]` table, as written.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct TerminalConfig {
    live_config_reload: Option<bool>,
}

/// How the Alacrittys gerdr starts behave, from the `[terminal]` table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalSettings {
    /// Whether each reloads its config file when it changes, which takes
    /// one of the user's inotify instances (see `inotify_limit`).
    pub live_config_reload: bool,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            live_config_reload: true,
        }
    }
}

/// The `[terminal]` settings `text` describes. What is wrong with them is
/// `from_text`'s to say.
pub fn terminal_from_text(text: &str) -> TerminalSettings {
    let table = Config::read(text, &mut Vec::new()).terminal;
    TerminalSettings {
        live_config_reload: table
            .live_config_reload
            .unwrap_or(TerminalSettings::default().live_config_reload),
    }
}

/// The `[t3]` settings `text` describes. What is wrong with them is
/// `from_text`'s to say.
pub fn t3_from_text(text: &str) -> T3Settings {
    let table = Config::read(text, &mut Vec::new()).t3;
    T3Settings {
        updates: table.updates.unwrap_or(T3Settings::default().updates),
    }
}

/// Whether `text` sets `key` of table `table` itself, whatever to.
pub fn sets(text: &str, table: &str, key: &str) -> bool {
    text.parse::<toml::Table>()
        .ok()
        .and_then(|file| file.get(table)?.get(key).cloned())
        .is_some()
}

/// `text` with `key` of table `table` set to `value`: the table added at
/// the end when it isn't there, everything else (comments, other keys,
/// their order and spacing) as it was.
pub fn with_bool(text: &str, table: &str, key: &str, value: bool) -> Result<String, String> {
    with_value(text, table, key, Some(toml_edit::value(value)))
}

/// `with_bool` for any value; no value takes the key out, what else the
/// table holds and its comments left be.
fn with_value(
    text: &str,
    table: &str,
    key: &str,
    value: Option<toml_edit::Item>,
) -> Result<String, String> {
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|err: toml_edit::TomlError| {
        err.message().to_owned() + &line_of(text, err.span())
    })?;
    match doc.get(table) {
        Some(item) if !item.is_table_like() => return Err(format!("[{table}] isn't a table")),
        Some(_) => {}
        None => {
            // After what the file ends with (the template's comments), not
            // above it, a blank line between.
            let mut new = toml_edit::Table::new();
            let trailing = doc.trailing().as_str().unwrap_or_default().to_owned();
            let gap = if text.trim().is_empty() || trailing.ends_with("\n\n") {
                ""
            } else {
                "\n"
            };
            new.decor_mut().set_prefix(format!("{trailing}{gap}"));
            doc.set_trailing("");
            doc.insert(table, toml_edit::Item::Table(new));
        }
    }
    match value {
        Some(value) => doc[table][key] = value,
        None => {
            if let Some(table) = doc[table].as_table_like_mut() {
                table.remove(key);
            }
        }
    }
    Ok(doc.to_string())
}

/// Set `key` of table `table` to `value` in the config file, made from
/// the template first when there is none, keeping the rest of it as it
/// was (see `with_bool`).
pub fn write_bool(table: &str, key: &str, value: bool) -> Result<(), String> {
    let path = config_file();
    ensure_file(&path).map_err(|err| err.to_string())?;
    let text = std::fs::read_to_string(&path).map_err(|err| err.to_string())?;
    let text = with_bool(&text, table, key, value)?;
    crate::projects::write_atomic(&path, text.as_bytes()).map_err(|err| err.to_string())
}

/// Keep machine `name`'s SSH target in `[machines]` of the config file, or
/// take it out, keeping the rest of the file as it was (see `with_bool`).
pub fn write_machine(name: &str, target: Option<&str>) -> Result<(), String> {
    let path = config_file();
    ensure_file(&path).map_err(|err| err.to_string())?;
    let text = std::fs::read_to_string(&path).map_err(|err| err.to_string())?;
    let text = with_value(&text, "machines", name, target.map(toml_edit::value))?;
    crate::projects::write_atomic(&path, text.as_bytes()).map_err(|err| err.to_string())
}

/// The `[notifications]` table, as written.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct NotificationsConfig {
    enabled: Option<bool>,
    timeout: Option<i64>,
}

/// How gerdr's desktop notifications show, from the `[notifications]`
/// table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Notifications {
    /// Whether gerdr sends any: agents that need the user, finished test
    /// runs, the services' errors (see `notify`).
    pub enabled: bool,
    /// Seconds each stays on screen: 0 until dismissed, below 0 as long as
    /// the notification daemon likes.
    pub timeout: i64,
}

impl Default for Notifications {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout: 30,
        }
    }
}

impl Notifications {
    /// The timeout as the notification spec has it: milliseconds, 0 for
    /// never, -1 for the daemon's choice.
    pub fn expire_ms(self) -> i32 {
        if self.timeout < 0 {
            -1
        } else {
            i32::try_from(self.timeout.saturating_mul(1000)).unwrap_or(i32::MAX)
        }
    }
}

/// How gerdr's desktop notifications show, as the config file says.
pub fn load_notifications() -> Notifications {
    let text = std::fs::read_to_string(config_file()).unwrap_or_default();
    notifications_from_text(&text)
}

/// The `[notifications]` settings `text` describes; the rest of the file is
/// `from_text`'s, which tells its mistakes.
pub fn notifications_from_text(text: &str) -> Notifications {
    let table = Config::read(text, &mut Vec::new()).notifications;
    let mut settings = Notifications::default();
    if let Some(enabled) = table.enabled {
        settings.enabled = enabled;
    }
    if let Some(timeout) = table.timeout {
        settings.timeout = timeout;
    }
    settings
}

/// The `[services]` table, as written.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ServicesConfig {
    errors: Option<Vec<String>>,
    extra_errors: Option<Vec<String>>,
    ignore: Option<Vec<String>>,
    notify: Option<bool>,
    stopped: Option<bool>,
}

/// How the services' errors are found and told, from the `[services]`
/// table.
#[derive(Clone, Debug)]
pub struct Services {
    /// Which output lines are errors.
    pub patterns: Patterns,
    /// Whether errors get desktop notifications.
    pub notify: bool,
    /// Whether a service's command stopping by itself is told.
    pub stopped: bool,
}

impl Default for Services {
    fn default() -> Self {
        Self {
            patterns: Patterns::default(),
            notify: true,
            stopped: true,
        }
    }
}

/// The `[services]` settings `text` describes, and what's wrong with them
/// (the defaults stand in for patterns that spoil them). The rest of the
/// file is `from_text`'s.
pub fn services_from_text(text: &str) -> (Services, Vec<String>) {
    // What else is wrong with the file is `from_text`'s to say.
    let table = Config::read(text, &mut Vec::new()).services;
    let mut problems = Vec::new();
    let mut services = Services::default();
    if table.errors.is_some() || table.extra_errors.is_some() || table.ignore.is_some() {
        let mut errors: Vec<String> = match table.errors {
            Some(errors) => errors,
            None => DEFAULT_ERRORS.iter().map(|p| (*p).to_owned()).collect(),
        };
        errors.extend(table.extra_errors.unwrap_or_default());
        errors.retain(|p| !p.trim().is_empty());
        let ignore: Vec<String> = table
            .ignore
            .unwrap_or_default()
            .into_iter()
            .filter(|p| !p.trim().is_empty())
            .collect();
        match Patterns::new(&errors, &ignore) {
            Ok(patterns) => services.patterns = patterns,
            Err(err) => problems.push(format!("[services]: {err}")),
        }
    }
    services.notify = table.notify.unwrap_or(true);
    services.stopped = table.stopped.unwrap_or(true);
    (services, problems)
}

/// How a problem of the `[keys]` table starts.
const KEYS_PROBLEM: &str = "[keys] ";

/// The tables the file may have.
const TABLES: [&str; 12] = [
    "keys",
    "apps",
    "ui",
    "git",
    "keep",
    "inbox",
    "agents",
    "machines",
    "services",
    "t3",
    "terminal",
    "notifications",
];

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
        let (services, t3, notifications) = (take("services"), take("t3"), take("notifications"));
        let (terminal, machines) = (take("terminal"), take("machines"));
        Self {
            keys: table("keys", keys, problems),
            apps: table("apps", apps, problems),
            ui: table("ui", ui, problems),
            git: table("git", git, problems),
            keep: table("keep", keep, problems),
            inbox: table("inbox", inbox, problems),
            agents: table("agents", agents, problems),
            machines: table("machines", machines, problems),
            services: table("services", services, problems),
            t3: table("t3", t3, problems),
            terminal: table("terminal", terminal, problems),
            notifications: table("notifications", notifications, problems),
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
    footer: Option<bool>,
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
    /// Whether the footer with the background tasks shows.
    pub footer: bool,
}

impl Default for Ui {
    fn default() -> Self {
        Ui {
            density: Density::default(),
            theme: None,
            needs_you: true,
            motion: Motion::Full,
            footer: true,
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
    /// Other machines' SSH targets by name (see `machines`).
    pub machines: BTreeMap<String, String>,
    /// How the services' errors are found and told (see `services`).
    pub services: Services,
    /// How T3 Code is kept up to date.
    pub t3: T3Settings,
    /// How the Alacrittys gerdr starts behave.
    pub terminal: TerminalSettings,
    /// Whether and how desktop notifications show.
    pub notifications: Notifications,
    /// What is wrong with the file, as sentences for the user.
    pub problems: Vec<String>,
}

/// The shortcuts, apps, look and git settings in the config file, how
/// the inbox reads Linear (`LINEAR_API_KEY` in the environment over the
/// file's key), the programs whose apps keep running (see `keep`) and the
/// agent CLI commands by name (see `cli_agents`), how the services'
/// errors are found (see `services`) and whether T3 Code updates itself.
/// What is wrong with the file is kept in the problems list under
/// `"config"` (and logged), in place of what was there before.
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
    let (services, service_problems) = services_from_text(&text);
    problems.extend(service_problems);
    let t3 = t3_from_text(&text);
    let terminal = terminal_from_text(&text);
    let notifications = notifications_from_text(&text);
    let machines = Config::read(&text, &mut Vec::new()).machines;
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
        machines,
        services,
        t3,
        terminal,
        notifications,
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
    if let Some(footer) = config.ui.footer {
        ui.footer = footer;
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
# footer = false        # hide the footer with the background tasks; shown by default
# motion = \"reduced\"    # or \"off\": no blinking dots or pulses; \"full\", the default
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
# # the workspace, in its folder, as typed at its shell. T3 Code, once it
# # ran here, starts a new thread in its app in a web pane; t3 = \"\"
# # hides it.
# claude = \"claude --dangerously-skip-permissions\"
# my-agent = \"my-agent --fast\"
#
# [machines]
# # Other machines gerdr reaches over SSH, by name: \"Connect a machine\" in
# # the launcher adds them here. gerdr connects with your SSH keys. A
# # feature stage's page picks the machine it runs on; a project's
# # file gives its stages one under \"machines\", and folders under
# # \"machine_folders\".
# build-box = \"ana@10.0.0.5\"
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
#
# [services]
# # Output lines of the services (web servers, workers) taken for errors,
# # as regexes, in place of the default ones (tracebacks, panics, ERROR,
# # CRITICAL and FATAL, 5xx answers in access logs…).
# errors = ['Traceback \\(most recent call last\\)', '\\bERROR\\b']
# # More, besides the default ones (or those of `errors`).
# extra_errors = ['level=error']
# # Lines never taken for errors, though they match: never shown. To
# # keep an error in sight but quiet, ignore it in the inbox (Ctrl+I).
# ignore = ['DeprecationWarning']
# # Desktop notifications of the errors; on by default ([notifications]
# # enabled = false turns them off too).
# notify = false
# # Tell when a service's command stops by itself; on by default.
# stopped = false
#
# [t3]
# # Take each newer T3 Code that gerdr reads as soon as it is out
# # (checked every 6 hours); false updates it only when asked. On by
# # default. The launcher's row turning it on or off writes this key.
# updates = false
#
# [terminal]
# # Whether each Alacritty gerdr starts reloads its config file when it
# # changes, which takes one of the user's inotify instances (128 by
# # default, and Zed needs them too). With false, new terminals read the
# # file once and take none; open ones let go of theirs when gerdr
# # restarts. On by default. The launcher's row turning it on or off
# # writes this key.
# live_config_reload = false
#
# [notifications]
# # Desktop notifications: of agents that need you, finished test runs and
# # the services' errors. On by default.
# enabled = false
# # Seconds gerdr's desktop notifications stay on screen; 0 keeps them
# # until dismissed, -1 leaves it to the notification daemon. By default 30.
# timeout = 0
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

    /// Read the config file again and use what it says now (see
    /// `apply_settings`). Says so, with what is wrong with it.
    pub fn reload_settings(&mut self) {
        let problems = self.apply_settings();
        self.settings_problems(&problems, true);
    }

    /// Read the config file again and use what it says now: the keys,
    /// apps, look, git, inbox, kept apps, agents, notifications, how the
    /// services' errors are found, whether T3 Code updates itself and
    /// whether new terminals reload their config.
    /// What is wrong with it.
    pub fn apply_settings(&mut self) -> Vec<String> {
        self.settings_modified = modified();
        let settings = load();
        self.keymap = settings.keymap;
        self.apps = settings.apps;
        self.density = settings.ui.density;
        self.needs_you = settings.ui.needs_you;
        // The panes take the footer's room, or give it back (`arrange`).
        self.show_footer = settings.ui.footer;
        if !self.show_footer {
            self.footer_hover = None;
            self.tasks_panel = None;
        }
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
        self.set_saved_machines(settings.machines);
        if let Some(notifier) = &mut self.notifier {
            notifier.set_settings(settings.notifications);
        }
        if settings.notifications.enabled != self.notify {
            self.notify = settings.notifications.enabled;
            // The bus source lets go once the notifier is gone.
            match self.notify {
                true => self.connect_notifier(),
                false => self.notifier = None,
            }
        }
        self.set_service_settings(settings.services);
        self.t3.auto_update = settings.t3.updates;
        // Terminals read it when they start: only new ones change.
        self.live_config_reload = settings.terminal.live_config_reload;
        self.arrange();
        settings.problems
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
        let (services, problems) = super::services_from_text(&examples);
        assert!(problems.is_empty(), "{problems:?}");
        assert!(!services.notify && !services.stopped);
        assert!(services.patterns.is_error("x level=error"));
        assert!(services.patterns.is_error("an ERROR"));
        assert!(!services.patterns.is_error("ERROR DeprecationWarning"));
        assert!(!services.patterns.is_error("a 500 FATAL"));
        assert!(!super::t3_from_text(&examples).updates);
        assert!(super::t3_from_text(TEMPLATE).updates);
    }

    #[test]
    fn terminals_reload_their_config_unless_the_file_turns_it_off() {
        let reload = |text: &str| super::terminal_from_text(text).live_config_reload;
        assert!(reload(""));
        assert!(reload(TEMPLATE));
        assert!(!reload("[terminal]\nlive_config_reload = false\n"));
        let (.., problems, _, _) = from_text("[terminal]\nlive_config_reload = false\n");
        assert!(problems.is_empty(), "{problems:?}");
        let (.., problems, _, _) = from_text("[terminal]\nlive_reload = false\n");
        assert_eq!(problems.len(), 1, "{problems:?}");
    }

    #[test]
    fn t3_updates_are_on_unless_the_file_turns_them_off() {
        use super::{T3Settings, t3_from_text};
        assert_eq!(t3_from_text(""), T3Settings { updates: true });
        assert!(t3_from_text("[t3]\n").updates);
        assert!(t3_from_text("[t3]\nupdates = true\n").updates);
        assert!(!t3_from_text("[t3]\nupdates = false\n").updates);
        // A wrong value is told, and the default stands.
        let text = "[t3]\nupdates = \"no\"\n";
        assert!(t3_from_text(text).updates);
        assert_eq!(from_text(text).5.len(), 1, "{:?}", from_text(text).5);
        assert_eq!(from_text("[t3]\nupdate = false\n").5.len(), 1);
        assert!(from_text("[t3]\nupdates = false\n").5.is_empty());
    }

    #[test]
    fn writing_a_key_keeps_the_rest_of_the_file() {
        use super::{sets, with_bool};
        // An existing table: its other keys and every comment stay.
        let text = "\
# My settings.
[ui]
density = \"compact\"  # tight

[t3]
# Off while on the train.
updates = true
other = 1
";
        let out = with_bool(text, "t3", "updates", false).unwrap();
        assert_eq!(out, text.replace("updates = true", "updates = false"));
        // A missing table: added at the end, after the comments the file
        // ends with.
        let text =
            "# My settings.\n[ui]\ndensity = \"compact\" # tight\n\n# [t3]\n# updates = false\n";
        let out = with_bool(text, "t3", "updates", false).unwrap();
        assert_eq!(out, format!("{text}\n[t3]\nupdates = false\n"));
        assert!(!sets(text, "t3", "updates") && sets(&out, "t3", "updates"));
        assert!(!super::t3_from_text(&out).updates);
        assert_eq!(from_text(&out).2.density, Density::Compact);
        // The template: still all its comments, then the key.
        let out = with_bool(TEMPLATE, "t3", "updates", true).unwrap();
        assert!(out.starts_with(TEMPLATE), "{out}");
        assert!(out.ends_with("\n[t3]\nupdates = true\n"), "{out}");
        assert!(from_text(&out).5.is_empty());
        // An empty file, one of tables only.
        assert_eq!(
            with_bool("", "t3", "updates", false).unwrap(),
            "[t3]\nupdates = false\n"
        );
        let out = with_bool("[ui]\nneeds_you = false\n", "t3", "updates", true).unwrap();
        assert_eq!(out, "[ui]\nneeds_you = false\n\n[t3]\nupdates = true\n");
        // A table without the key, written inline: the key joins it.
        let out = with_bool("t3 = { }\n", "t3", "updates", false).unwrap();
        assert!(!super::t3_from_text(&out).updates, "{out}");
        // What can't be written says why.
        assert!(with_bool("t3 = 1\n", "t3", "updates", false).is_err());
        assert!(with_bool("[t3\n", "t3", "updates", false).is_err());
    }

    #[test]
    fn services() {
        let (services, problems) = super::services_from_text("");
        assert!(problems.is_empty());
        assert!(services.notify && services.stopped);
        assert!(services.patterns.is_error("FATAL boom"));
        let (services, problems) =
            super::services_from_text("[services]\nextra_errors = ['level=error', '(']\n");
        assert_eq!(problems.len(), 1, "{problems:?}");
        // The defaults stand in.
        assert!(services.patterns.is_error("FATAL boom"));
        assert!(!services.patterns.is_error("level=error"));
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
                footer: true,
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
    fn the_footer_shows_unless_the_file_hides_it() {
        assert!(ui("").0.footer);
        let (parsed, problems) = ui("[ui]\nfooter = false\n");
        assert!(!parsed.footer);
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

    #[test]
    fn notifications_last_30_seconds_unless_told() {
        use super::{Notifications, notifications_from_text};
        assert_eq!(notifications_from_text(""), Notifications::default());
        assert!(Notifications::default().enabled);
        assert!(!notifications_from_text("[notifications]\nenabled = false\n").enabled);
        assert_eq!(notifications_from_text("").expire_ms(), 30_000);
        let never = notifications_from_text("[notifications]\ntimeout = 0\n");
        assert_eq!(never.expire_ms(), 0);
        let daemon = notifications_from_text("[notifications]\ntimeout = -1\n");
        assert_eq!(daemon.expire_ms(), -1);
        let long = notifications_from_text("[notifications]\ntimeout = 99999999\n");
        assert_eq!(long.expire_ms(), i32::MAX);
    }
}

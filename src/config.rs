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
//! Mistakes are logged, and the defaults stand in for what they spoil.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::{
    chrome::{Density, ThemeMode},
    home_dir,
    keys::{Keymap, KeysConfig},
    launch::App,
};

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Config {
    keys: KeysConfig,
    /// Commands by name.
    apps: BTreeMap<String, String>,
    ui: UiConfig,
    git: GitConfig,
    keep: KeepConfig,
    inbox: InboxConfig,
}

/// The `[ui]` table, as written.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct UiConfig {
    density: Option<String>,
    theme: Option<String>,
    needs_you: Option<bool>,
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
}

impl Default for Ui {
    fn default() -> Self {
        Ui {
            density: Density::default(),
            theme: None,
            needs_you: true,
        }
    }
}

/// The shortcuts, apps, look and git settings in the config file, how
/// the inbox reads Linear (`LINEAR_API_KEY` in the environment over the
/// file's key), and the programs whose apps keep running (see `keep`).
pub fn load() -> (Keymap, Vec<App>, Ui, Git, Option<Linear>, Vec<String>) {
    let path = config_file();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) => {
            if err.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("can't read {}: {err}", path.display());
            }
            String::new()
        }
    };
    let (keymap, apps, ui, git, inbox, problems, keep) = from_text(&text);
    for problem in problems {
        tracing::warn!("{}: {problem}", path.display());
    }
    let linear = inbox.linear(std::env::var("LINEAR_API_KEY").ok());
    (keymap, apps, ui, git, linear, keep)
}

/// The shortcuts, apps, look, git and inbox settings `text` describes,
/// what's wrong with it, and the programs whose apps keep running.
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
) {
    let mut problems = Vec::new();
    let config: Config = toml::from_str(text).unwrap_or_else(|err| {
        problems.push(err.to_string());
        Config::default()
    });
    let (keymap, keys_problems) = Keymap::new(config.keys);
    problems.extend(keys_problems);
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
    if let Some(theme) = config.ui.theme.as_deref() {
        match ThemeMode::from_name(theme) {
            Some(mode) => ui.theme = Some(mode),
            None => problems.push(format!("unknown theme `{theme}`")),
        }
    }
    if let Some(needs_you) = config.ui.needs_you {
        ui.needs_you = needs_you;
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
    (keymap, apps, ui, git, inbox, problems, keep)
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
# gerdr's settings, read when gerdr starts.
#
# [keys]
# prefix = \"Ctrl+A\"
#
# [apps]
# # Run by `sh` in the workspace folder.
# browser = \"firefox http://localhost:3000\"
# files = \"nautilus .\"
#
# [ui]
# density = \"compact\"   # or \"comfortable\", the default
# theme = \"light\"       # or \"dark\" or \"system\"; else the last choice
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
        let (_, apps, ui, git, inbox, problems, _) = from_text(TEMPLATE);
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
        let (_, apps, ui, git, inbox, problems, _) = from_text(&examples);
        assert!(problems.is_empty(), "{problems:?}\n{examples}");
        assert_eq!(apps.len(), 2);
        assert_eq!(ui.density, Density::Compact);
        assert_eq!(git.same_branches.len(), 2);
        assert_eq!(git.top_branches, ["main", "dev", "staging"]);
        assert_eq!(inbox.linear_teams, ["ENG"]);
        assert!(inbox.linear_api_key.is_some());
    }

    #[test]
    fn apps() {
        let (_, apps, _, _, _, problems, _) =
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
        let (.., problems, keep) = from_text("[keep]\napps = [\" code \", \"\"]\n");
        assert_eq!(keep, ["code"]);
        assert_eq!(problems, ["a blank program name in `[keep] apps`"]);
    }

    #[test]
    fn mistakes() {
        assert_eq!(from_text("[key]\n").5.len(), 1);
        assert_eq!(from_text("[apps]\nbrowser = 1\n").5.len(), 1);
        assert_eq!(from_text("[inbox]\nlinear = 1\n").5.len(), 1);
    }

    fn ui(text: &str) -> (Ui, Vec<String>) {
        let (_, _, ui, _, _, problems, _) = from_text(text);
        (ui, problems)
    }

    fn same(text: &str) -> (Vec<Vec<String>>, Vec<String>) {
        let (_, _, _, git, _, problems, _) = from_text(text);
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
            let (_, _, _, git, _, problems, _) = from_text(text);
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
        // A wrong type, or a key that isn't there, spoils the whole file.
        let (parsed, problems) = ui("[ui]\ndensity = 1\n");
        assert_eq!(parsed, Ui::default());
        assert_eq!(problems.len(), 1);
        let (parsed, problems) = ui("[ui]\ncolour = \"red\"\n");
        assert_eq!(parsed, Ui::default());
        assert_eq!(problems.len(), 1);
    }
}

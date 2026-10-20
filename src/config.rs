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
//!
//! [git]
//! # Branch names that are one branch across repositories, in the
//! # repositories panel's branch picker. By default main and master.
//! same_branches = [["main", "master"], ["production", "prod"]]
//! # Branches listed first in its branch lists, in this order, after the
//! # default branch; the rest go newest first. By default main, master,
//! # dev and develop.
//! top_branches = ["main", "dev", "staging"]
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
}

/// The `[ui]` table, as written.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct UiConfig {
    density: Option<String>,
    theme: Option<String>,
}

/// The `[git]` table, as written.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct GitConfig {
    same_branches: Option<Vec<Vec<String>>>,
    top_branches: Option<Vec<String>>,
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ui {
    pub density: Density,
    /// The theme named in the file; `None` leaves it to the last choice.
    pub theme: Option<ThemeMode>,
}

/// The shortcuts, apps, look and git settings in the config file.
pub fn load() -> (Keymap, Vec<App>, Ui, Git) {
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
    let (keymap, apps, ui, git, problems) = from_text(&text);
    for problem in problems {
        tracing::warn!("{}: {problem}", path.display());
    }
    (keymap, apps, ui, git)
}

/// The shortcuts, apps, look and git settings `text` describes, and what's
/// wrong with it.
pub fn from_text(text: &str) -> (Keymap, Vec<App>, Ui, Git, Vec<String>) {
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
    (keymap, apps, ui, git, problems)
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
    use super::{Density, Git, TEMPLATE, ThemeMode, Ui, from_text};

    #[test]
    fn the_template_changes_nothing() {
        let (_, apps, ui, git, problems) = from_text(TEMPLATE);
        assert!(problems.is_empty(), "{problems:?}");
        assert!(apps.is_empty());
        assert_eq!(ui, Ui::default());
        assert_eq!(git, Git::default());
        // Each example, uncommented, is understood.
        let examples: String = TEMPLATE
            .lines()
            .filter(|line| !line.starts_with("# ") || line.contains('=') || line.contains('['))
            .map(|line| line.strip_prefix("# ").unwrap_or(line))
            .filter(|line| !line.starts_with('#'))
            .map(|line| format!("{line}\n"))
            .collect();
        let (_, apps, ui, git, problems) = from_text(&examples);
        assert!(problems.is_empty(), "{problems:?}\n{examples}");
        assert_eq!(apps.len(), 2);
        assert_eq!(ui.density, Density::Compact);
        assert_eq!(git.same_branches.len(), 2);
        assert_eq!(git.top_branches, ["main", "dev", "staging"]);
    }

    #[test]
    fn apps() {
        let (_, apps, _, _, problems) =
            from_text("[apps]\nfiles = \"nautilus .\"\nbrowser = \" firefox \"\nnone = \"\"\n");
        let apps: Vec<_> = apps
            .iter()
            .map(|app| (app.name.as_str(), app.command.as_str()))
            .collect();
        assert_eq!(apps, [("browser", "firefox"), ("files", "nautilus .")]);
        assert_eq!(problems, ["app `none` has no command"]);
    }

    #[test]
    fn mistakes() {
        assert_eq!(from_text("[key]\n").4.len(), 1);
        assert_eq!(from_text("[apps]\nbrowser = 1\n").4.len(), 1);
    }

    fn ui(text: &str) -> (Ui, Vec<String>) {
        let (_, _, ui, _, problems) = from_text(text);
        (ui, problems)
    }

    fn same(text: &str) -> (Vec<Vec<String>>, Vec<String>) {
        let (_, _, _, git, problems) = from_text(text);
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
            let (_, _, _, git, problems) = from_text(text);
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
                theme: Some(ThemeMode::Light)
            }
        );
        assert!(problems.is_empty());
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

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
//! ```
//!
//! Mistakes are logged, and the defaults stand in for what they spoil.

use std::{collections::BTreeMap, path::PathBuf};

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
}

/// The `[ui]` table, as written.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct UiConfig {
    density: Option<String>,
    theme: Option<String>,
}

/// How the chrome looks, from the `[ui]` table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ui {
    pub density: Density,
    /// The theme named in the file; `None` leaves it to the last choice.
    pub theme: Option<ThemeMode>,
}

/// The shortcuts, apps and look in the config file.
pub fn load() -> (Keymap, Vec<App>, Ui) {
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
    let (keymap, apps, ui, problems) = from_text(&text);
    for problem in problems {
        tracing::warn!("{}: {problem}", path.display());
    }
    (keymap, apps, ui)
}

/// The shortcuts, apps and look `text` describes, and what's wrong with it.
pub fn from_text(text: &str) -> (Keymap, Vec<App>, Ui, Vec<String>) {
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
    (keymap, apps, ui, problems)
}

fn config_file() -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| home_dir().unwrap_or_default().join(".config"));
    config.join("gerdr/config.toml")
}

#[cfg(test)]
mod tests {
    use super::{Density, ThemeMode, Ui, from_text};

    #[test]
    fn apps() {
        let (_, apps, _, problems) =
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
        assert_eq!(from_text("[key]\n").3.len(), 1);
        assert_eq!(from_text("[apps]\nbrowser = 1\n").3.len(), 1);
    }

    #[test]
    fn ui() {
        let (_, _, ui, problems) = from_text("[ui]\ndensity = \"compact\"\ntheme = \"light\"\n");
        assert_eq!(ui.density, Density::Compact);
        assert_eq!(ui.theme, Some(ThemeMode::Light));
        assert!(problems.is_empty());
        let (_, _, ui, problems) = from_text("");
        assert_eq!(ui, Ui::default());
        assert_eq!(ui.density, Density::Comfortable);
        assert!(problems.is_empty());
        let (_, _, ui, problems) = from_text("[ui]\ndensity = \"cosy\"\ntheme = \"blue\"\n");
        assert_eq!(ui, Ui::default());
        assert_eq!(problems.len(), 2);
    }
}

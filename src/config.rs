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
    use super::{Density, TEMPLATE, ThemeMode, Ui, from_text};

    #[test]
    fn the_template_changes_nothing() {
        let (_, apps, ui, problems) = from_text(TEMPLATE);
        assert!(problems.is_empty(), "{problems:?}");
        assert!(apps.is_empty());
        assert_eq!(ui, Ui::default());
        // Each example, uncommented, is understood.
        let examples: String = TEMPLATE
            .lines()
            .filter(|line| !line.starts_with("# ") || line.contains('=') || line.contains('['))
            .map(|line| line.strip_prefix("# ").unwrap_or(line))
            .filter(|line| !line.starts_with('#'))
            .map(|line| format!("{line}\n"))
            .collect();
        let (_, apps, ui, problems) = from_text(&examples);
        assert!(problems.is_empty(), "{problems:?}\n{examples}");
        assert_eq!(apps.len(), 2);
        assert_eq!(ui.density, Density::Compact);
    }

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

    fn ui(text: &str) -> (Ui, Vec<String>) {
        let (_, _, ui, problems) = from_text(text);
        (ui, problems)
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

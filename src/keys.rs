//! gerdr's shortcuts: the prefix, and the key that runs each action after it.
//! Both can be changed in the config file (see `config`):
//!
//! ```toml
//! [keys]
//! prefix = "Ctrl+A"
//! new-tab = "t"                       # in place of c
//! toggle-theme = ["Shift+L", "F12"]   # in place of t
//! close-pane = []                     # no key
//! ```
//!
//! An action given keys loses its default ones; the others keep theirs,
//! except a default key that a configured action took. A key is modifiers
//! (Ctrl, Alt, Shift, Super) and a key name joined by "+", as in "Shift+Tab",
//! "-" or "Down"; letters are the same in either case, so "Shift+X" needs its
//! Shift. Ctrl held from the prefix doesn't matter unless the key names it.
//! Mistakes are logged, and the defaults stand in for what they spoil.

use std::{collections::BTreeMap, sync::Arc};

use serde::Deserialize;
use smithay::input::keyboard::{Keysym, ModifiersState, xkb};

use crate::{
    Action,
    chrome::Help,
    layout::{Dir, Side},
};

const DEFAULT_PREFIX: &str = "Ctrl+B";

/// Every action as (name in the config, action, what the help panel calls
/// it, default key).
const ACTIONS: &[(&str, Action, &str, &str)] = &[
    ("new-tab", Action::NewTab, "new tab", "c"),
    ("next-tab", Action::NextTab, "next tab", "n"),
    ("previous-tab", Action::PrevTab, "previous tab", "p"),
    ("go-to-tab-1", Action::GotoTab(0), "go to tab 1", "1"),
    ("go-to-tab-2", Action::GotoTab(1), "go to tab 2", "2"),
    ("go-to-tab-3", Action::GotoTab(2), "go to tab 3", "3"),
    ("go-to-tab-4", Action::GotoTab(3), "go to tab 4", "4"),
    ("go-to-tab-5", Action::GotoTab(4), "go to tab 5", "5"),
    ("go-to-tab-6", Action::GotoTab(5), "go to tab 6", "6"),
    ("go-to-tab-7", Action::GotoTab(6), "go to tab 7", "7"),
    ("go-to-tab-8", Action::GotoTab(7), "go to tab 8", "8"),
    ("go-to-tab-9", Action::GotoTab(8), "go to tab 9", "9"),
    ("close-tab", Action::CloseTab, "close tab", "Shift+X"),
    (
        "open-project",
        Action::NewProject,
        "open project",
        "Shift+N",
    ),
    (
        "new-terminal",
        Action::NewTerminal,
        "terminal workspace in home",
        "Shift+T",
    ),
    (
        "close-workspace",
        Action::CloseWorkspace,
        "close workspace",
        "Shift+D",
    ),
    (
        "next-workspace",
        Action::NextWorkspace,
        "next workspace",
        "Down",
    ),
    (
        "previous-workspace",
        Action::PrevWorkspace,
        "previous workspace",
        "Up",
    ),
    (
        "split-right",
        Action::Split(Dir::Right),
        "split pane right",
        "v",
    ),
    (
        "split-down",
        Action::Split(Dir::Down),
        "split pane down",
        "-",
    ),
    ("close-pane", Action::ClosePane, "close pane", "x"),
    (
        "restart-pane",
        Action::RestartPane,
        "restart the pane's command",
        "a",
    ),
    (
        "stop-pane",
        Action::StopPane,
        "stop the pane's command",
        "s",
    ),
    (
        "focus-left",
        Action::FocusSide(Side::Left),
        "focus pane left",
        "h",
    ),
    (
        "focus-down",
        Action::FocusSide(Side::Down),
        "focus pane down",
        "j",
    ),
    (
        "focus-up",
        Action::FocusSide(Side::Up),
        "focus pane up",
        "k",
    ),
    (
        "focus-right",
        Action::FocusSide(Side::Right),
        "focus pane right",
        "l",
    ),
    ("next-pane", Action::CyclePane(1), "next pane", "Tab"),
    (
        "previous-pane",
        Action::CyclePane(-1),
        "previous pane",
        "Shift+Tab",
    ),
    (
        "toggle-theme",
        Action::ToggleTheme,
        "light / dark theme",
        "t",
    ),
    ("switcher", Action::Switcher, "find and switch", "f"),
    ("run", Action::Run, "run an app in a split", "r"),
    (
        "run-tab",
        Action::RunTab,
        "run an app in a new tab",
        "Shift+R",
    ),
    ("projects", Action::Projects, "projects", "Shift+P"),
    (
        "last-project",
        Action::LastProject,
        "last project",
        "Shift+L",
    ),
    (
        "toggle-restart",
        Action::ToggleRestart,
        "restart after reboot",
        "Shift+S",
    ),
    ("quit", Action::Quit, "quit…", "Shift+Q"),
];

/// The help panel's lines, in order: actions that share a line when each
/// has exactly one key (else each gets its own), under one label.
const HELP_LINES: &[(&str, &[&str])] = &[
    ("new tab", &["new-tab"]),
    ("next / previous tab", &["next-tab", "previous-tab"]),
    (
        "go to tab",
        &[
            "go-to-tab-1",
            "go-to-tab-2",
            "go-to-tab-3",
            "go-to-tab-4",
            "go-to-tab-5",
            "go-to-tab-6",
            "go-to-tab-7",
            "go-to-tab-8",
            "go-to-tab-9",
        ],
    ),
    ("close tab", &["close-tab"]),
    ("open project", &["open-project"]),
    ("terminal workspace in home", &["new-terminal"]),
    ("close workspace", &["close-workspace"]),
    (
        "next / previous workspace",
        &["next-workspace", "previous-workspace"],
    ),
    ("split pane right / down", &["split-right", "split-down"]),
    ("close pane", &["close-pane"]),
    (
        "restart / stop the pane's command",
        &["restart-pane", "stop-pane"],
    ),
    (
        "focus pane left / down / up / right",
        &["focus-left", "focus-down", "focus-up", "focus-right"],
    ),
    ("next / previous pane", &["next-pane", "previous-pane"]),
    ("light / dark theme", &["toggle-theme"]),
    ("find and switch", &["switcher"]),
    ("run an app in a split", &["run"]),
    ("run an app in a new tab", &["run-tab"]),
    ("projects", &["projects"]),
    ("last project", &["last-project"]),
    ("restart after reboot", &["toggle-restart"]),
    ("quit…", &["quit"]),
];

/// The modifiers a key needs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Mods {
    ctrl: bool,
    alt: bool,
    shift: bool,
    logo: bool,
}

/// A key with its modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Chord {
    mods: Mods,
    sym: Keysym,
}

impl Chord {
    /// The key pressed: `sym` is the key's own symbol, without Shift applied.
    fn pressed(mods: &ModifiersState, sym: Keysym) -> Self {
        let mods = Mods {
            ctrl: mods.ctrl,
            alt: mods.alt,
            shift: mods.shift,
            logo: mods.logo,
        };
        // Some layouts give Tab another symbol under Shift.
        let sym = if sym == Keysym::ISO_Left_Tab {
            Keysym::Tab
        } else {
            sym
        };
        Self { mods, sym }
    }

    /// Read a key as written in the config, like "Ctrl+Shift+X".
    fn parse(spec: &str) -> Result<Self, String> {
        let spec = spec.trim();
        // "+" is a key too: "+" or "Ctrl++".
        let (mods_spec, key) = match spec.strip_suffix("++") {
            Some(rest) => (rest, "+"),
            None if spec == "+" => ("", "+"),
            None => spec.rsplit_once('+').unwrap_or(("", spec)),
        };
        let mut mods = Mods::default();
        for name in mods_spec.split('+').filter(|name| !name.is_empty()) {
            let flag = match name.trim().to_ascii_lowercase().as_str() {
                "ctrl" | "control" => &mut mods.ctrl,
                "alt" => &mut mods.alt,
                "shift" => &mut mods.shift,
                "super" | "logo" | "win" => &mut mods.logo,
                _ => return Err(format!("unknown modifier `{name}` in `{spec}`")),
            };
            *flag = true;
        }
        let sym = key_sym(key.trim()).ok_or_else(|| format!("unknown key `{key}` in `{spec}`"))?;
        Ok(Self { mods, sym })
    }

    /// The key as the help panel shows it, like "Shift+X".
    fn label(&self) -> String {
        let Mods {
            ctrl,
            alt,
            shift,
            logo,
        } = self.mods;
        let mods = [
            (ctrl, "Ctrl+"),
            (alt, "Alt+"),
            (shift, "Shift+"),
            (logo, "Super+"),
        ];
        let mut label: String = mods
            .iter()
            .filter(|(on, _)| *on)
            .map(|(_, name)| *name)
            .collect();
        label.push_str(&key_label(self.sym));
        label
    }
}

/// The symbol for a key name: a character, or an xkb name like "Down" or
/// "F12" in any case.
fn key_sym(name: &str) -> Option<Keysym> {
    let mut chars = name.chars();
    let sym = match (chars.next(), chars.next()) {
        (Some(c), None) => xkb::utf32_to_keysym(c.to_lowercase().next().unwrap_or(c) as u32),
        _ => {
            let name = match name.to_ascii_lowercase().as_str() {
                "esc" => "Escape",
                "enter" => "Return",
                _ => name,
            };
            match xkb::keysym_from_name(name, xkb::KEYSYM_NO_FLAGS) {
                Keysym::NoSymbol => xkb::keysym_from_name(name, xkb::KEYSYM_CASE_INSENSITIVE),
                sym => sym,
            }
        }
    };
    (sym != Keysym::NoSymbol).then_some(sym)
}

/// How the help panel names a key: what it types (letters upper case), else
/// its xkb name.
fn key_label(sym: Keysym) -> String {
    match sym {
        Keysym::Escape => return "Esc".into(),
        Keysym::Return => return "Enter".into(),
        Keysym::space => return "Space".into(),
        _ => {}
    }
    match char::from_u32(xkb::keysym_to_utf32(sym))
        .filter(|c| !c.is_control() && !c.is_whitespace())
    {
        Some(c) => c.to_uppercase().collect(),
        None => {
            let name = xkb::keysym_get_name(sym);
            let mut chars = name.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or(name)
        }
    }
}

/// The config file's `[keys]`.
#[derive(Default, Deserialize)]
#[serde(default)]
pub struct KeysConfig {
    prefix: Option<String>,
    /// Keys by action name.
    #[serde(flatten)]
    actions: BTreeMap<String, Keys>,
}

/// One key or several, or none.
#[derive(Deserialize)]
#[serde(untagged)]
enum Keys {
    One(String),
    Many(Vec<String>),
}

/// The prefix and what the key after it runs.
pub struct Keymap {
    prefix: Chord,
    bindings: Vec<(Chord, Action)>,
    help: Arc<Help>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::new(KeysConfig::default()).0
    }
}

impl Keymap {
    /// The keymap `config` describes, and what's wrong with it.
    pub fn new(config: KeysConfig) -> (Self, Vec<String>) {
        let mut problems = Vec::new();
        let default_prefix = Chord::parse(DEFAULT_PREFIX).expect("default prefix");
        let prefix = config
            .prefix
            .map_or(Ok(default_prefix), |spec| Chord::parse(&spec))
            .unwrap_or_else(|err| {
                problems.push(err);
                default_prefix
            });

        // The configured keys first, so they win over the defaults.
        let mut bindings: Vec<(Chord, usize)> = Vec::new();
        let mut configured = vec![false; ACTIONS.len()];
        for (name, keys) in config.actions {
            let Some(idx) = ACTIONS.iter().position(|(n, ..)| *n == name) else {
                problems.push(format!("unknown action `{name}`"));
                continue;
            };
            configured[idx] = true;
            let keys = match keys {
                Keys::One(key) => vec![key],
                Keys::Many(keys) => keys,
            };
            for spec in keys {
                match Chord::parse(&spec) {
                    Ok(chord) => match bindings.iter().find(|(c, _)| *c == chord) {
                        Some((_, other)) => problems
                            .push(format!("`{spec}` is already for `{}`", ACTIONS[*other].0)),
                        None => bindings.push((chord, idx)),
                    },
                    Err(err) => problems.push(err),
                }
            }
        }
        for (idx, (_, _, _, spec)) in ACTIONS.iter().enumerate() {
            let chord = Chord::parse(spec).expect("default key");
            if !configured[idx] && !bindings.iter().any(|(c, _)| *c == chord) {
                bindings.push((chord, idx));
            }
        }

        let help = Arc::new(help(prefix, &bindings));
        let bindings = bindings
            .into_iter()
            .map(|(chord, idx)| (chord, ACTIONS[idx].1))
            .collect();
        (
            Self {
                prefix,
                bindings,
                help,
            },
            problems,
        )
    }

    pub fn is_prefix(&self, mods: &ModifiersState, sym: Keysym) -> bool {
        Chord::pressed(mods, sym) == self.prefix
    }

    /// The action for the key pressed after the prefix.
    pub fn action(&self, mods: &ModifiersState, sym: Keysym) -> Option<Action> {
        let chord = Chord::pressed(mods, sym);
        let find = |chord: Chord| {
            self.bindings
                .iter()
                .find(|(c, _)| *c == chord)
                .map(|(_, action)| *action)
        };
        // Ctrl may still be held from the prefix.
        find(chord).or_else(|| {
            find(Chord {
                mods: Mods {
                    ctrl: false,
                    ..chord.mods
                },
                ..chord
            })
        })
    }

    /// What the help panel lists while the prefix is armed.
    pub fn help(&self) -> Arc<Help> {
        self.help.clone()
    }
}

/// The help panel for these bindings, as (keys, label) lines.
fn help(prefix: Chord, bindings: &[(Chord, usize)]) -> Help {
    let keys = |name: &str| -> Vec<String> {
        let idx = ACTIONS
            .iter()
            .position(|(n, ..)| *n == name)
            .expect("action in HELP_LINES");
        bindings
            .iter()
            .filter(|(_, i)| *i == idx)
            .map(|(chord, _)| chord.label())
            .collect()
    };
    let mut rows = Vec::new();
    for (label, names) in HELP_LINES {
        let all: Vec<Vec<String>> = names.iter().map(|name| keys(name)).collect();
        if names.len() == 1 || all.iter().any(|keys| keys.len() != 1) {
            for (name, keys) in names.iter().zip(all) {
                if !keys.is_empty() {
                    let label = if names.len() == 1 {
                        label
                    } else {
                        ACTIONS.iter().find(|(n, ..)| n == name).unwrap().2
                    };
                    rows.push((keys.join(" / "), label.to_string()));
                }
            }
        } else if names.len() > 4 {
            // A run, like the tab numbers: its ends.
            rows.push((
                format!("{}–{}", all[0][0], all[all.len() - 1][0]),
                label.to_string(),
            ));
        } else {
            rows.push((
                all.iter()
                    .map(|keys| keys[0].as_str())
                    .collect::<Vec<_>>()
                    .join(" / "),
                label.to_string(),
            ));
        }
    }
    let prefix = prefix.label();
    rows.push((prefix.clone(), format!("send {prefix} to the app")));
    rows.push(("Esc".into(), "cancel".into()));
    Help { prefix, rows }
}

#[cfg(test)]
mod tests {
    use smithay::input::keyboard::{Keysym, ModifiersState};

    use super::{ACTIONS, Chord, Keymap};
    use crate::{
        Action, config,
        layout::{Dir, Side},
    };

    fn from_config(text: &str) -> (Keymap, Vec<String>) {
        let (keymap, _, problems) = config::from_text(text);
        (keymap, problems)
    }

    fn mods(ctrl: bool, shift: bool) -> ModifiersState {
        ModifiersState {
            ctrl,
            shift,
            ..Default::default()
        }
    }

    #[test]
    fn default_help() {
        let help = Keymap::default().help();
        let rows: Vec<_> = help
            .rows
            .iter()
            .map(|(keys, label)| format!("{keys}: {label}"))
            .collect();
        assert_eq!(help.prefix, "Ctrl+B");
        assert_eq!(
            rows,
            [
                "C: new tab",
                "N / P: next / previous tab",
                "1–9: go to tab",
                "Shift+X: close tab",
                "Shift+N: open project",
                "Shift+T: terminal workspace in home",
                "Shift+D: close workspace",
                "Down / Up: next / previous workspace",
                "V / -: split pane right / down",
                "X: close pane",
                "A / S: restart / stop the pane's command",
                "H / J / K / L: focus pane left / down / up / right",
                "Tab / Shift+Tab: next / previous pane",
                "T: light / dark theme",
                "F: find and switch",
                "R: run an app in a split",
                "Shift+R: run an app in a new tab",
                "Shift+P: projects",
                "Shift+L: last project",
                "Shift+S: restart after reboot",
                "Shift+Q: quit…",
                "Ctrl+B: send Ctrl+B to the app",
                "Esc: cancel",
            ]
        );
    }

    #[test]
    fn default_actions() {
        let keymap = Keymap::default();
        assert!(keymap.is_prefix(&mods(true, false), Keysym::b));
        assert!(!keymap.is_prefix(&mods(false, false), Keysym::b));
        // Ctrl still held from the prefix.
        assert_eq!(
            keymap.action(&mods(true, false), Keysym::_3),
            Some(Action::GotoTab(2))
        );
        assert_eq!(
            keymap.action(&mods(false, false), Keysym::minus),
            Some(Action::Split(Dir::Down))
        );
        assert_eq!(
            keymap.action(&mods(false, true), Keysym::ISO_Left_Tab),
            Some(Action::CyclePane(-1))
        );
        assert_eq!(keymap.action(&mods(false, true), Keysym::c), None);
        // Shift tells the project keys from the tab and run keys.
        assert_eq!(
            keymap.action(&mods(false, true), Keysym::p),
            Some(Action::Projects)
        );
        assert_eq!(
            keymap.action(&mods(false, false), Keysym::p),
            Some(Action::PrevTab)
        );
        assert_eq!(
            keymap.action(&mods(false, true), Keysym::r),
            Some(Action::RunTab)
        );
        assert_eq!(
            keymap.action(&mods(false, true), Keysym::s),
            Some(Action::ToggleRestart)
        );
        assert_eq!(
            keymap.action(&mods(false, false), Keysym::s),
            Some(Action::StopPane)
        );
        assert_eq!(
            keymap.action(&mods(false, false), Keysym::a),
            Some(Action::RestartPane)
        );
        assert_eq!(
            keymap.action(&mods(false, false), Keysym::r),
            Some(Action::Run)
        );
        assert_eq!(
            keymap.action(&mods(false, true), Keysym::q),
            Some(Action::Quit)
        );
        assert_eq!(keymap.action(&mods(false, false), Keysym::q), None);
    }

    #[test]
    fn last_project_is_shift_l() {
        let keymap = Keymap::default();
        assert_eq!(
            keymap.action(&mods(false, true), Keysym::l),
            Some(Action::LastProject)
        );
        // Plain l still focuses right.
        assert_eq!(
            keymap.action(&mods(false, false), Keysym::l),
            Some(Action::FocusSide(Side::Right))
        );
    }

    #[test]
    fn default_keys_are_all_different() {
        let chords: Vec<Chord> = ACTIONS
            .iter()
            .map(|(_, _, _, spec)| Chord::parse(spec).expect("default key"))
            .collect();
        for (i, chord) in chords.iter().enumerate() {
            assert!(
                !chords[..i].contains(chord),
                "`{}` is the default of two actions",
                ACTIONS[i].3
            );
        }
    }

    #[test]
    fn config_rebinds() {
        let config = "[keys]\nprefix = \"Ctrl+Space\"\nnew-tab = \"t\"\nclose-pane = []\nsplit-down = [\"Shift+minus\", \"s\"]\n";
        let (keymap, problems) = from_config(config);
        assert!(problems.is_empty(), "{problems:?}");
        assert!(keymap.is_prefix(&mods(true, false), Keysym::space));
        assert_eq!(
            keymap.action(&mods(false, false), Keysym::t),
            Some(Action::NewTab)
        );
        // c lost new-tab, x lost close-pane, and toggle-theme lost t to new-tab.
        assert_eq!(keymap.action(&mods(false, false), Keysym::c), None);
        assert_eq!(keymap.action(&mods(false, false), Keysym::x), None);
        assert_eq!(keymap.action(&mods(false, false), Keysym::minus), None);
        assert_eq!(
            keymap.action(&mods(false, true), Keysym::minus),
            Some(Action::Split(Dir::Down))
        );
        let help = keymap.help();
        assert!(
            help.rows
                .contains(&("Shift+- / S".into(), "split pane down".into()))
        );
        assert!(help.rows.contains(&("V".into(), "split pane right".into())));
        assert!(
            !help
                .rows
                .iter()
                .any(|(_, label)| label == "light / dark theme")
        );
        assert_eq!(help.rows[help.rows.len() - 2].0, "Ctrl+Space");
    }

    #[test]
    fn config_mistakes() {
        let (keymap, problems) = from_config(
            "[keys]\nprefix = \"Hyper+B\"\nfly = \"f\"\nnew-tab = [\"Ctrl+\", \"n\"]\n",
        );
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(keymap.is_prefix(&mods(true, false), Keysym::b));
        assert_eq!(
            keymap.action(&mods(false, false), Keysym::n),
            Some(Action::NewTab)
        );
        assert_eq!(
            Chord::parse("Ctrl++").map(|c| c.label()),
            Ok("Ctrl++".into())
        );
        assert_eq!(
            Chord::parse("shift+f12").map(|c| c.label()),
            Ok("Shift+F12".into())
        );
    }
}

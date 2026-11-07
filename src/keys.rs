//! gerdr's shortcuts: the prefix, and the key that runs each action after it.
//! Both can be changed in the config file (see `config`):
//!
//! ```toml
//! [keys]
//! prefix = "Ctrl+A"
//! new-tab = "t"                       # in place of c
//! toggle-theme = ["Alt+T", "F12"]     # in place of t
//! close-pane = []                     # no key
//! ```
//!
//! An action given keys loses its default ones; the others keep theirs,
//! except a default key that a configured action took. A key is modifiers
//! (Ctrl, Alt, Shift, Super) and a key name joined by "+", as in "Shift+Tab",
//! "-" or "Down"; letters are the same in either case, so "Shift+X" needs its
//! Shift; a character typed with Shift ("?", "!", "@", "{") is Shift and the
//! key it is on in the keyboard's layout, and one the layout types only
//! with AltGr, or on no key, is a mistake. Ctrl still held from the prefix
//! doesn't count: a key with Ctrl in it takes pressing Ctrl again (see
//! `input`). Mistakes, a default key taken and an
//! action's old name are reported (see `problems`), and the defaults stand
//! in for what they spoil.

use std::{collections::BTreeMap, sync::Arc};

use serde::Deserialize;
use smithay::input::keyboard::{Keysym, ModifiersState, xkb};

use crate::{
    Action,
    chrome::{Help, HelpPress},
    layout::{Dir, Side},
};

const DEFAULT_PREFIX: &str = "Ctrl+B";

/// Every action as (name in the config, action, what the help panel calls
/// it, default key). A label starts with a verb; it ends in "…" when more
/// input follows (a dialog or a picker), not when a panel opens; a toggle
/// says so.
const ACTIONS: &[(&str, Action, &str, &str)] = &[
    ("new-tab", Action::NewTab, "Open a new tab", "c"),
    ("new-agent", Action::NewAgent, "Start an agent…", "Shift+A"),
    ("next-tab", Action::NextTab, "Go to the next tab", "n"),
    (
        "previous-tab",
        Action::PrevTab,
        "Go to the previous tab",
        "p",
    ),
    ("go-to-tab-1", Action::GotoTab(0), "Go to tab 1", "1"),
    ("go-to-tab-2", Action::GotoTab(1), "Go to tab 2", "2"),
    ("go-to-tab-3", Action::GotoTab(2), "Go to tab 3", "3"),
    ("go-to-tab-4", Action::GotoTab(3), "Go to tab 4", "4"),
    ("go-to-tab-5", Action::GotoTab(4), "Go to tab 5", "5"),
    ("go-to-tab-6", Action::GotoTab(5), "Go to tab 6", "6"),
    ("go-to-tab-7", Action::GotoTab(6), "Go to tab 7", "7"),
    ("go-to-tab-8", Action::GotoTab(7), "Go to tab 8", "8"),
    ("go-to-tab-9", Action::GotoTab(8), "Go to tab 9", "9"),
    ("close-tab", Action::CloseTab, "Close tab", "Shift+X"),
    // tmux's rename-window.
    ("rename-tab", Action::RenameTab, "Rename tab…", ","),
    // "<" and ">" on a US layout, by the key: some layouts have "<" on a
    // key of its own as well, which a US keyboard lacks.
    (
        "move-tab-left",
        Action::MoveTab(-1),
        "Move tab left",
        "Shift+comma",
    ),
    (
        "move-tab-right",
        Action::MoveTab(1),
        "Move tab right",
        "Shift+period",
    ),
    // A folder of the project, opened as a Zed + terminal workspace.
    (
        "open-workspace",
        Action::NewProject,
        "Open a folder as a workspace…",
        "Shift+N",
    ),
    (
        "new-terminal",
        Action::NewTerminal,
        "Open a terminal workspace in home",
        "Shift+T",
    ),
    (
        "close-workspace",
        Action::CloseWorkspace,
        "Close workspace",
        "Shift+D",
    ),
    (
        "next-workspace",
        Action::NextWorkspace,
        "Go to the next workspace",
        "Down",
    ),
    // What a right click on its sidebar row offers.
    (
        "workspace-menu",
        Action::WorkspaceMenu,
        "Open the workspace's menu…",
        "Shift+M",
    ),
    (
        "previous-workspace",
        Action::PrevWorkspace,
        "Go to the previous workspace",
        "Up",
    ),
    (
        "go-back",
        Action::GoBack,
        "Go back to the tab shown before",
        "Left",
    ),
    (
        "go-forward",
        Action::GoForward,
        "Go forward to the tab gone back from",
        "Right",
    ),
    (
        "split-right",
        Action::Split(Dir::Right),
        "Split pane right",
        "v",
    ),
    (
        "split-down",
        Action::Split(Dir::Down),
        "Split pane down",
        "-",
    ),
    ("close-pane", Action::ClosePane, "Close pane", "x"),
    (
        "restart-pane",
        Action::RestartPane,
        "Restart the pane's command",
        "a",
    ),
    (
        "stop-pane",
        Action::StopPane,
        "Stop the pane's command",
        "s",
    ),
    (
        "send-prompt",
        Action::SendPrompt,
        "Send the waiting agent its stage's prompt",
        "Enter",
    ),
    (
        "focus-left",
        Action::FocusSide(Side::Left),
        "Focus pane left",
        "h",
    ),
    (
        "focus-down",
        Action::FocusSide(Side::Down),
        "Focus pane down",
        "j",
    ),
    (
        "focus-up",
        Action::FocusSide(Side::Up),
        "Focus pane up",
        "k",
    ),
    (
        "focus-right",
        Action::FocusSide(Side::Right),
        "Focus pane right",
        "l",
    ),
    (
        "move-left",
        Action::MovePane(Side::Left),
        "Move pane left",
        "Shift+H",
    ),
    (
        "move-down",
        Action::MovePane(Side::Down),
        "Move pane down",
        "Shift+J",
    ),
    (
        "move-up",
        Action::MovePane(Side::Up),
        "Move pane up",
        "Shift+K",
    ),
    (
        "move-right",
        Action::MovePane(Side::Right),
        "Move pane right",
        "Shift+L",
    ),
    (
        "swap-left",
        Action::SwapPane(Side::Left),
        "Swap pane left",
        "Alt+H",
    ),
    (
        "swap-down",
        Action::SwapPane(Side::Down),
        "Swap pane down",
        "Alt+J",
    ),
    (
        "swap-up",
        Action::SwapPane(Side::Up),
        "Swap pane up",
        "Alt+K",
    ),
    (
        "swap-right",
        Action::SwapPane(Side::Right),
        "Swap pane right",
        "Alt+L",
    ),
    (
        "flip-pane",
        Action::FlipPane,
        "Flip the pane's split",
        "Space",
    ),
    // herdr's and tmux's zoom.
    (
        "zoom-pane",
        Action::ZoomPane,
        "Zoom pane to fill the tab, or unzoom",
        "z",
    ),
    // tmux's break-pane.
    (
        "pane-to-new-tab",
        Action::PaneToNewTab,
        "Move pane to a new tab",
        "!",
    ),
    // What a right click on its bar offers.
    ("pane-menu", Action::PaneMenu, "Open the pane's menu…", "m"),
    (
        "next-pane",
        Action::CyclePane(1),
        "Go to the next pane",
        "Tab",
    ),
    (
        "previous-pane",
        Action::CyclePane(-1),
        "Go to the previous pane",
        "Shift+Tab",
    ),
    // Light or dark for good: it no longer follows the system's choice.
    (
        "toggle-theme",
        Action::ToggleTheme,
        "Toggle light / dark theme (not system)",
        "t",
    ),
    ("shortcuts", Action::Shortcuts, "Keep this help open", "?"),
    ("switcher", Action::Switcher, "Find and switch…", "f"),
    ("run", Action::Run, "Run an app in a split…", "r"),
    (
        "run-tab",
        Action::RunTab,
        "Run an app in a new tab…",
        "Shift+R",
    ),
    ("projects", Action::Projects, "Choose a project…", "Shift+P"),
    (
        "last-project",
        Action::LastProject,
        "Go to the last project",
        "Alt+P",
    ),
    (
        "toggle-restart",
        Action::ToggleRestart,
        "Toggle running again after a reboot",
        "Shift+S",
    ),
    (
        "toggle-venv",
        Action::ToggleVenv,
        "Toggle python venv",
        "Shift+V",
    ),
    (
        "pick-venv",
        Action::PickVenv,
        "Pick a python venv…",
        "Alt+V",
    ),
    (
        "run-start-commands",
        Action::RunStartCommands,
        "Run the project's start commands",
        "Shift+B",
    ),
    ("worktrees", Action::Worktrees, "Open worktrees", "w"),
    ("repos", Action::Repos, "Open repositories", "g"),
    ("inbox", Action::Inbox, "Open inbox", "i"),
    ("tasks", Action::Tasks, "Open background tasks", "b"),
    ("features", Action::Features, "Open features", "Shift+F"),
    ("ai-stats", Action::AiStats, "Open the AI work stats", "u"),
    (
        "criteria",
        Action::Criteria,
        "Try the criteria over the sandbox's app",
        "Shift+C",
    ),
    // tmux's copy-mode.
    (
        "scrollback",
        Action::Scrollback,
        "Select and copy the scrollback",
        "[",
    ),
    (
        "reload-projects",
        Action::ReloadProjects,
        "Reload the project files",
        "Alt+R",
    ),
    ("quit", Action::Quit, "Quit…", "Shift+Q"),
];

/// Names actions went by before, and what they are called now: still
/// read, with a word that they were renamed.
const OLD_NAMES: &[(&str, &str)] = &[
    // While it started only Cursor's agent.
    ("new-cursor-agent", "new-agent"),
    // It opens a folder of the project as a workspace, not a project.
    ("open-project", "open-workspace"),
];

/// The help panel's lines, in order: actions that share a line when each
/// has exactly one key (else each gets its own), under one label.
const HELP_LINES: &[(&str, &[&str])] = &[
    ("Open a new tab", &["new-tab"]),
    ("Start an agent…", &["new-agent"]),
    (
        "Go to the next / previous tab",
        &["next-tab", "previous-tab"],
    ),
    (
        "Go to tab",
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
    ("Close tab", &["close-tab"]),
    ("Rename tab…", &["rename-tab"]),
    (
        "Move tab left / right",
        &["move-tab-left", "move-tab-right"],
    ),
    ("Open a folder as a workspace…", &["open-workspace"]),
    ("Open a terminal workspace in home", &["new-terminal"]),
    ("Close workspace", &["close-workspace"]),
    (
        "Go to the next / previous workspace",
        &["next-workspace", "previous-workspace"],
    ),
    (
        "Go back / forward through the tabs shown",
        &["go-back", "go-forward"],
    ),
    (
        "Open the pane's / workspace's menu…",
        &["pane-menu", "workspace-menu"],
    ),
    ("Split pane right / down", &["split-right", "split-down"]),
    ("Close pane", &["close-pane"]),
    (
        "Restart / stop the pane's command",
        &["restart-pane", "stop-pane"],
    ),
    (
        "Focus pane left / down / up / right",
        &["focus-left", "focus-down", "focus-up", "focus-right"],
    ),
    (
        "Move pane left / down / up / right",
        &["move-left", "move-down", "move-up", "move-right"],
    ),
    (
        "Swap pane left / down / up / right",
        &["swap-left", "swap-down", "swap-up", "swap-right"],
    ),
    ("Flip the pane's split", &["flip-pane"]),
    ("Zoom pane to fill the tab, or unzoom", &["zoom-pane"]),
    ("Move pane to a new tab", &["pane-to-new-tab"]),
    (
        "Go to the next / previous pane",
        &["next-pane", "previous-pane"],
    ),
    ("Toggle light / dark theme (not system)", &["toggle-theme"]),
    ("Keep this help open", &["shortcuts"]),
    ("Find and switch…", &["switcher"]),
    ("Run an app in a split…", &["run"]),
    ("Run an app in a new tab…", &["run-tab"]),
    ("Choose a project…", &["projects"]),
    ("Go to the last project", &["last-project"]),
    ("Toggle running again after a reboot", &["toggle-restart"]),
    (
        "Toggle python venv, pick one…",
        &["toggle-venv", "pick-venv"],
    ),
    ("Run the project's start commands", &["run-start-commands"]),
    ("Open worktrees", &["worktrees"]),
    ("Open repositories", &["repos"]),
    ("Open inbox", &["inbox"]),
    ("Open background tasks", &["tasks"]),
    ("Open features, or start one", &["features"]),
    ("Open the AI work stats", &["ai-stats"]),
    ("Try the criteria over the sandbox's app", &["criteria"]),
    ("Select and copy the scrollback", &["scrollback"]),
    ("Reload the project files", &["reload-projects"]),
    ("Quit…", &["quit"]),
];

/// What the help panel called its lines before they started with a verb,
/// and what it calls them now, so a lookup by the old label still finds
/// the key (see `Help::key_for`).
const OLD_LABELS: &[(&str, &str)] = &[
    ("new tab", "Open a new tab"),
    ("new agent", "Start an agent…"),
    ("next tab", "Go to the next tab"),
    ("previous tab", "Go to the previous tab"),
    ("next / previous tab", "Go to the next / previous tab"),
    ("go to tab", "Go to tab"),
    ("open project", "Open a folder as a workspace…"),
    (
        "terminal workspace in home",
        "Open a terminal workspace in home",
    ),
    ("next workspace", "Go to the next workspace"),
    ("previous workspace", "Go to the previous workspace"),
    (
        "next / previous workspace",
        "Go to the next / previous workspace",
    ),
    ("next pane", "Go to the next pane"),
    ("previous pane", "Go to the previous pane"),
    ("next / previous pane", "Go to the next / previous pane"),
    (
        "light / dark theme",
        "Toggle light / dark theme (not system)",
    ),
    ("find and switch", "Find and switch…"),
    ("run an app in a split", "Run an app in a split…"),
    ("run an app in a new tab", "Run an app in a new tab…"),
    ("projects", "Choose a project…"),
    ("last project", "Go to the last project"),
    (
        "restart after reboot",
        "Toggle running again after a reboot",
    ),
    (
        "toggle restart after reboot",
        "Toggle running again after a reboot",
    ),
    ("python venv on / off", "Toggle python venv"),
    ("pick python venv", "Pick a python venv…"),
    (
        "python venv on / off, pick",
        "Toggle python venv, pick one…",
    ),
    ("worktrees", "Open worktrees"),
    ("repositories", "Open repositories"),
    ("inbox", "Open inbox"),
    ("reload the project file", "Reload the project files"),
    ("quit…", "Quit…"),
];

/// The help panel's label for what it used to call `label`; any other
/// label as it is. Case does not matter.
pub fn help_label(label: &str) -> &str {
    OLD_LABELS
        .iter()
        .find(|(old, _)| old.eq_ignore_ascii_case(label))
        .map_or(label, |(_, new)| new)
}

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

    /// Read a key as written in the config, like "Ctrl+Shift+X": a
    /// character typed with Shift ("@", "?") is matched as Shift and the
    /// key it is on in the keyboard's layout. One the layout types on no
    /// key, or only with AltGr, is a mistake.
    fn parse(spec: &str) -> Result<Self, String> {
        Self::parse_in(spec, layout(), true)
    }

    /// Read a key gerdr itself names (a default, a hint's): a character
    /// the layout has no key for is taken as on a US layout.
    fn parse_lenient(spec: &str) -> Result<Self, String> {
        Self::parse_in(spec, layout(), false)
    }

    /// Read `spec` against `layout` (none when the layout can't be read:
    /// then as on a US layout); `strict` turns a character it has no key
    /// for into a mistake.
    fn parse_in(spec: &str, layout: Option<&Layout>, strict: bool) -> Result<Self, String> {
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
        let key = key.trim();
        let sym = key_sym(key).ok_or_else(|| format!("unknown key `{key}` in `{spec}`"))?;
        if types_char(sym) {
            match layout.map(|layout| layout.typing(sym)) {
                Some(Some((base, shift))) => {
                    mods.shift |= shift;
                    return Ok(Self { mods, sym: base });
                }
                Some(None) if strict => {
                    return Err(format!(
                        "no key types `{key}` (in `{spec}`) without AltGr on this keyboard \
                         layout; name the key it is on instead"
                    ));
                }
                _ => {}
            }
        }
        // As on a US layout: "?" is Shift and the slash key. Its xkb
        // name, "question", is the same key.
        let shifted = SHIFTED
            .iter()
            .find(|(c, _)| key_sym(c) == Some(sym))
            .and_then(|(_, base)| key_sym(base));
        if let Some(base) = shifted {
            mods.shift = true;
            return Ok(Self { mods, sym: base });
        }
        Ok(Self { mods, sym })
    }

    /// The key as the help panel shows it, like "Shift+X".
    fn label(&self) -> String {
        self.label_in(layout())
    }

    /// The key as the help panel shows it with `layout`: Shift and a key
    /// typing a character other than its own upper case is that
    /// character ("Shift+/" is "?").
    fn label_in(&self, layout: Option<&Layout>) -> String {
        let Mods {
            ctrl,
            alt,
            shift,
            logo,
        } = self.mods;
        let key = key_label(self.sym);
        let typed = match layout {
            Some(layout) => layout.shifted(self.sym).map(char_label),
            None => SHIFTED
                .iter()
                .find(|(_, base)| key_sym(base) == Some(self.sym))
                .map(|(c, _)| (*c).to_owned()),
        };
        let (key, shift) = match typed {
            Some(c) if shift && c != key => (c, false),
            _ => (key, shift),
        };
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
        label.push_str(&key);
        label
    }
}

/// The key a key hint shows ("Ctrl+M", "Shift+Del", "Esc") as pressed:
/// its modifiers and the key's own symbol. `None` for anything else, like
/// keys to choose from ("Up / Down").
pub fn hint_key(keys: &str) -> Option<(ModifiersState, Keysym)> {
    let Chord { mods, sym } = Chord::parse_lenient(keys).ok()?;
    let mods = ModifiersState {
        ctrl: mods.ctrl,
        alt: mods.alt,
        shift: mods.shift,
        logo: mods.logo,
        ..Default::default()
    };
    Some((mods, sym))
}

/// Characters typed with Shift on the key named after them, as on a US
/// layout: what a key written as one of them is when the keyboard's
/// layout can't be read.
const SHIFTED: &[(&str, &str)] = &[("?", "/"), ("!", "1"), ("<", ","), (">", ".")];

/// Whether `sym` types a character that sits on a key of a layout, to
/// be found there: not a letter's case, a keypad key, space or a control.
fn types_char(sym: Keysym) -> bool {
    !sym.is_keypad_key()
        && sym
            .key_char()
            .is_some_and(|c| !c.is_control() && !c.is_whitespace())
}

/// What the keyboard's layout types: per key and layout, the key's own
/// symbol (as gerdr is handed it on a press, see `Chord::pressed`) and
/// what it types with Shift.
#[derive(Debug, Default)]
struct Layout {
    keys: Vec<(Keysym, Keysym)>,
}

impl Layout {
    /// The layout xkb compiles from `layout` (and the environment's
    /// `XKB_DEFAULT_*` when `env`), as the seat's keyboard does with no
    /// names; `None` without xkb's data.
    fn compile(layout: &str, env: bool) -> Option<Self> {
        let flags = match env {
            true => xkb::CONTEXT_NO_FLAGS,
            false => xkb::CONTEXT_NO_ENVIRONMENT_NAMES,
        };
        let context = xkb::Context::new(flags);
        let keymap = xkb::Keymap::new_from_names(
            &context,
            "",
            "",
            layout,
            "",
            None,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )?;
        let shift = 1 << keymap.mod_get_index(xkb::MOD_NAME_SHIFT);
        let mut codes = Vec::new();
        keymap.key_for_each(|_, code| codes.push(code));
        let layouts = keymap.num_layouts();
        // The key's own symbol in `group`; one typing something other than
        // ASCII is taken from another group typing ASCII there, as the
        // seat does for a key pressed.
        let own = |code, group| -> Option<Keysym> {
            let sym = *keymap.key_get_syms_by_level(code, group, 0).first()?;
            if sym.key_char().is_none_or(|c| c.is_ascii()) {
                return Some(sym);
            }
            let ascii = (0..layouts).filter(|g| *g != group).find_map(|g| {
                let other = *keymap.key_get_syms_by_level(code, g, 0).first()?;
                other
                    .key_char()
                    .is_some_and(|c| c.is_ascii() && !c.is_ascii_control())
                    .then_some(other)
            });
            Some(ascii.unwrap_or(sym))
        };
        let mut keys = Vec::new();
        for group in 0..layouts {
            let mut state = xkb::State::new(&keymap);
            state.update_mask(shift, 0, 0, 0, 0, group);
            for &code in &codes {
                if let Some(sym) = own(code, group) {
                    keys.push((sym, state.key_get_one_sym(code)));
                }
            }
        }
        Some(Self { keys })
    }

    /// The key typing `sym`, and whether it takes Shift: on a key of its
    /// own first, else with Shift.
    fn typing(&self, sym: Keysym) -> Option<(Keysym, bool)> {
        let lower = key_sym(&char_label(sym).to_lowercase()).unwrap_or(sym);
        self.keys
            .iter()
            .find(|(own, _)| *own == sym || *own == lower)
            .map(|(own, _)| (*own, false))
            .or_else(|| {
                self.keys
                    .iter()
                    .find(|(_, shifted)| *shifted == sym)
                    .map(|(own, _)| (*own, true))
            })
    }

    /// What Shift and the key `own` type, when it is a character other
    /// than the key's own in upper case: "?" for the slash key.
    fn shifted(&self, own: Keysym) -> Option<Keysym> {
        let (_, shifted) = self.keys.iter().find(|(o, _)| *o == own)?;
        let upper = char_label(own);
        (types_char(*shifted) && char_label(*shifted) != upper).then_some(*shifted)
    }
}

/// The keyboard's layout, as the seat's keyboard compiles it; `None`
/// when xkb can't (no data installed). Tests read a US layout.
fn layout() -> Option<&'static Layout> {
    static LAYOUT: std::sync::OnceLock<Option<Layout>> = std::sync::OnceLock::new();
    LAYOUT
        .get_or_init(|| match cfg!(test) {
            true => Layout::compile("us", false),
            false => Layout::compile("", true),
        })
        .as_ref()
}

/// How a key that types a character is shown: the character, upper case.
fn char_label(sym: Keysym) -> String {
    match sym.key_char() {
        Some(c) => c.to_uppercase().collect(),
        None => key_label(sym),
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
                "del" => "Delete",
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
/// its xkb name, short where every panel shortens it ("Esc", "Del").
fn key_label(sym: Keysym) -> String {
    match sym {
        Keysym::Escape => return "Esc".into(),
        Keysym::Return => return "Enter".into(),
        Keysym::space => return "Space".into(),
        Keysym::Delete => return "Del".into(),
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

/// Where the prefix is pressed: what its keys can run there.
#[derive(Clone, Copy)]
pub struct Reach<'a> {
    /// Whether the action works there.
    pub works: &'a dyn Fn(Action) -> bool,
    /// Whether the prefix pressed again goes to the app: only with
    /// nothing of gerdr's taking the keys.
    pub to_app: bool,
}

impl Reach<'_> {
    /// Over the apps, where every key works.
    const APPS: Reach<'static> = Reach {
        works: &|_| true,
        to_app: true,
    };
}

impl Default for Keymap {
    fn default() -> Self {
        Self::build(KeysConfig::default()).0
    }
}

impl Keymap {
    /// The keymap `config` describes, and what's wrong with it. What's
    /// wrong is also kept in `problems` (as "keys"), in place of what an
    /// earlier config had wrong.
    pub fn new(config: KeysConfig) -> (Self, Vec<String>) {
        let (keymap, problems) = Self::build(config);
        crate::problems::clear("keys");
        for problem in &problems {
            crate::problems::report("keys", problem.clone());
        }
        (keymap, problems)
    }

    /// The keymap `config` describes, and what's wrong with it.
    fn build(config: KeysConfig) -> (Self, Vec<String>) {
        let mut problems = Vec::new();
        let default_prefix = Chord::parse_lenient(DEFAULT_PREFIX).expect("default prefix");
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
            let name = match OLD_NAMES.iter().find(|(old, _)| *old == name) {
                Some((old, new)) => {
                    problems.push(format!("`{old}` is now called `{new}`; it still works"));
                    (*new).to_owned()
                }
                None => name,
            };
            let Some(idx) = ACTIONS.iter().position(|(n, ..)| *n == name) else {
                problems.push(format!("unknown action `{name}`"));
                continue;
            };
            let keys = match keys {
                Keys::One(key) => vec![key],
                Keys::Many(keys) => keys,
            };
            // No keys at all is meant; keys that are all mistakes leave
            // the default standing in.
            configured[idx] |= keys.is_empty();
            for spec in keys {
                if let Some(problem) = upper_case_letter(&spec) {
                    problems.push(problem);
                }
                match Chord::parse(&spec) {
                    Ok(chord) => {
                        configured[idx] = true;
                        match bindings.iter().find(|(c, _)| *c == chord) {
                            Some((_, other)) => problems
                                .push(format!("`{spec}` is already for `{}`", ACTIONS[*other].0)),
                            None => bindings.push((chord, idx)),
                        }
                    }
                    Err(err) => problems.push(err),
                }
            }
        }
        for (idx, (name, _, _, spec)) in ACTIONS.iter().enumerate() {
            let chord = Chord::parse_lenient(spec).expect("default key");
            if configured[idx] {
                continue;
            }
            match bindings.iter().find(|(c, _)| *c == chord) {
                // Its default went to another action: say so, or it just
                // drops out of the help.
                Some((_, other)) => problems.push(format!(
                    "`{name}` has no key: its default `{}` is now for `{}`",
                    chord.label(),
                    ACTIONS[*other].0
                )),
                None => bindings.push((chord, idx)),
            }
        }
        problems.extend(ctrl_clashes(prefix, &bindings));

        let help = Arc::new(help(prefix, &bindings, Reach::APPS));
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

    /// What the help panel lists while the prefix is armed at `reach`:
    /// the keys that don't work there greyed out (`HelpPress::NotHere`).
    pub fn help_at(&self, reach: Reach) -> Help {
        let bindings: Vec<(Chord, usize)> = self
            .bindings
            .iter()
            .filter_map(|(chord, action)| {
                let idx = ACTIONS.iter().position(|(_, a, ..)| a == action)?;
                Some((*chord, idx))
            })
            .collect();
        help(self.prefix, &bindings, reach)
    }
}

/// How the help panel names the key pressed with `mods` held, like
/// "Shift+Q" or "?" (Shift and the slash key).
pub fn chord_label(mods: &ModifiersState, sym: Keysym) -> String {
    Chord::pressed(mods, sym).label()
}

/// What's wrong with `spec`, a single upper case letter with no
/// modifiers: it is the letter's key without Shift, which it may not
/// look like.
fn upper_case_letter(spec: &str) -> Option<String> {
    let mut chars = spec.trim().chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_uppercase() => Some(format!(
            "`{c}` is the {} key without Shift; write `Shift+{c}` if Shift is meant",
            c.to_lowercase()
        )),
        _ => None,
    }
}

/// Keys that run different actions only by whether Ctrl is held, after a
/// prefix with Ctrl in it: Ctrl still held from the prefix picks the one
/// without it, and only pressing Ctrl again the one with it, which may
/// surprise.
fn ctrl_clashes(prefix: Chord, bindings: &[(Chord, usize)]) -> Vec<String> {
    if !prefix.mods.ctrl {
        return Vec::new();
    }
    bindings
        .iter()
        .filter(|(chord, _)| chord.mods.ctrl)
        .filter_map(|(chord, idx)| {
            let plain = Chord {
                mods: Mods {
                    ctrl: false,
                    ..chord.mods
                },
                ..*chord
            };
            let (_, other) = bindings.iter().find(|(c, i)| *c == plain && i != idx)?;
            Some(format!(
                "`{}` (`{}`) and `{}` (`{}`) differ only by Ctrl: Ctrl still held from \
                 the prefix `{}` picks `{}`; press Ctrl again for `{}`",
                chord.label(),
                ACTIONS[*idx].0,
                plain.label(),
                ACTIONS[*other].0,
                prefix.label(),
                plain.label(),
                chord.label()
            ))
        })
        .collect()
}

/// `keys` with the modifiers they all have written on the first only, as in
/// "Alt+H / J / K / L": the help's key column is as wide as its widest
/// line. `Help::key_for` reads the others without them.
fn shared_mods<'a>(keys: &[&'a str]) -> Vec<&'a str> {
    // Up to the key: "Ctrl++" is Ctrl and "+".
    let mods = |key: &'a str| match key.strip_suffix('+') {
        Some(rest) if rest.is_empty() || rest.ends_with('+') => rest,
        _ => key.rfind('+').map_or("", |i| &key[..=i]),
    };
    let first = keys.first().map_or("", |key| mods(key));
    if first.is_empty() || keys.iter().any(|key| mods(key) != first) {
        return keys.to_vec();
    }
    let rest = keys[1..].iter().map(|key| &key[first.len()..]);
    keys[..1].iter().copied().chain(rest).collect()
}

/// The help panel for these bindings pressed at `reach`, as (keys,
/// label) lines.
fn help(prefix: Chord, bindings: &[(Chord, usize)], reach: Reach) -> Help {
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
    let action = |name: &str| {
        ACTIONS
            .iter()
            .find(|(n, ..)| *n == name)
            .map(|(_, a, ..)| *a)
    };
    // What a click on a key of the action named `name` does.
    let press = |name: &str| {
        let action = action(name)?;
        Some(match (reach.works)(action) {
            true => HelpPress::Action(action),
            false => HelpPress::NotHere,
        })
    };
    let (mut rows, mut presses) = (Vec::new(), Vec::new());
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
                    presses.push(vec![press(name); keys.len()]);
                    rows.push((keys.join(" / "), label.to_string()));
                }
            }
        } else if names.len() > 4 {
            // A run, like the tab numbers: its ends, standing for them all.
            let works = names
                .iter()
                .filter_map(|name| action(name))
                .any(reach.works);
            presses.push(vec![(!works).then_some(HelpPress::NotHere)]);
            rows.push((
                format!("{}–{}", all[0][0], all[all.len() - 1][0]),
                label.to_string(),
            ));
        } else {
            let keys: Vec<&str> = all.iter().map(|keys| keys[0].as_str()).collect();
            presses.push(names.iter().map(|name| press(name)).collect());
            rows.push((shared_mods(&keys).join(" / "), label.to_string()));
        }
    }
    let prefix = prefix.label();
    // The prefix goes to the app only as a key.
    presses.push(vec![(!reach.to_app).then_some(HelpPress::NotHere)]);
    rows.push((prefix.clone(), format!("Send {prefix} to the app")));
    presses.push(vec![Some(HelpPress::Close)]);
    rows.push(("Esc".into(), "Cancel".into()));
    Help {
        prefix,
        rows,
        presses,
    }
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
        let (keymap, _, _, _, _, problems, ..) = config::from_text(text);
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
                "C: Open a new tab",
                "Shift+A: Start an agent…",
                "N / P: Go to the next / previous tab",
                "1–9: Go to tab",
                "Shift+X: Close tab",
                ",: Rename tab…",
                "< / >: Move tab left / right",
                "Shift+N: Open a folder as a workspace…",
                "Shift+T: Open a terminal workspace in home",
                "Shift+D: Close workspace",
                "Down / Up: Go to the next / previous workspace",
                "Left / Right: Go back / forward through the tabs shown",
                "M / Shift+M: Open the pane's / workspace's menu…",
                "V / -: Split pane right / down",
                "X: Close pane",
                "A / S: Restart / stop the pane's command",
                "H / J / K / L: Focus pane left / down / up / right",
                "Shift+H / J / K / L: Move pane left / down / up / right",
                "Alt+H / J / K / L: Swap pane left / down / up / right",
                "Space: Flip the pane's split",
                "Z: Zoom pane to fill the tab, or unzoom",
                "!: Move pane to a new tab",
                "Tab / Shift+Tab: Go to the next / previous pane",
                "T: Toggle light / dark theme (not system)",
                "?: Keep this help open",
                "F: Find and switch…",
                "R: Run an app in a split…",
                "Shift+R: Run an app in a new tab…",
                "Shift+P: Choose a project…",
                "Alt+P: Go to the last project",
                "Shift+S: Toggle running again after a reboot",
                "Shift+V / Alt+V: Toggle python venv, pick one…",
                "Shift+B: Run the project's start commands",
                "W: Open worktrees",
                "G: Open repositories",
                "I: Open inbox",
                "B: Open background tasks",
                "Shift+F: Open features, or start one",
                "U: Open the AI work stats",
                "Shift+C: Try the criteria over the sandbox's app",
                "[: Select and copy the scrollback",
                "Alt+R: Reload the project files",
                "Shift+Q: Quit…",
                "Ctrl+B: Send Ctrl+B to the app",
                "Esc: Cancel",
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
        assert_eq!(
            keymap.action(&mods(false, true), Keysym::c),
            Some(Action::Criteria)
        );
        assert_eq!(
            keymap.action(&mods(false, false), Keysym::u),
            Some(Action::AiStats)
        );
        assert_eq!(
            keymap.action(&mods(false, false), Keysym::bracketleft),
            Some(Action::Scrollback)
        );
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
        // "?" is Shift and the slash key.
        assert_eq!(
            keymap.action(&mods(false, true), Keysym::slash),
            Some(Action::Shortcuts)
        );
        assert_eq!(keymap.action(&mods(false, false), Keysym::slash), None);
    }

    #[test]
    fn tabs_and_menus_have_keys() {
        let keymap = Keymap::default();
        assert_eq!(
            keymap.action(&mods(false, false), Keysym::comma),
            Some(Action::RenameTab)
        );
        // "<" and ">" are Shift and the comma and period keys.
        assert_eq!(
            keymap.action(&mods(false, true), Keysym::comma),
            Some(Action::MoveTab(-1))
        );
        assert_eq!(
            keymap.action(&mods(false, true), Keysym::period),
            Some(Action::MoveTab(1))
        );
        assert_eq!(
            keymap.action(&mods(false, false), Keysym::m),
            Some(Action::PaneMenu)
        );
        assert_eq!(
            keymap.action(&mods(false, true), Keysym::m),
            Some(Action::WorkspaceMenu)
        );
        // F2, the panels' rename key, can be added.
        let (keymap, problems) = from_config("[keys]\nrename-tab = [\",\", \"F2\"]\n");
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            keymap.action(&mods(false, false), Keysym::F2),
            Some(Action::RenameTab)
        );
    }

    #[test]
    fn what_does_not_work_where_the_prefix_is_pressed_is_greyed() {
        use crate::chrome::HelpPress;
        let keymap = Keymap::default();
        let only_theme = |action| action == Action::ToggleTheme;
        let help = keymap.help_at(super::Reach {
            works: &only_theme,
            to_app: false,
        });
        let row = |label: &str| {
            help.rows
                .iter()
                .position(|(_, l)| l.starts_with(label))
                .unwrap_or_else(|| panic!("no row {label}"))
        };
        assert!(!help.greyed(row("Toggle light / dark")));
        assert_eq!(
            help.presses[row("Toggle light / dark")],
            [Some(HelpPress::Action(Action::ToggleTheme))]
        );
        assert!(help.greyed(row("Open a new tab")));
        assert!(help.greyed(row("Go to tab")));
        assert!(help.greyed(row("Send Ctrl+B")));
        assert!(!help.greyed(row("Cancel")));
        // The same rows, keys and labels as everywhere.
        let all = keymap.help();
        assert_eq!(help.rows, all.rows);
        assert!((0..all.rows.len()).all(|i| !all.greyed(i)));
    }

    #[test]
    fn a_pressed_key_is_named_as_the_help_names_it() {
        let shift = mods(false, true);
        assert_eq!(super::chord_label(&shift, Keysym::q), "Shift+Q");
        assert_eq!(super::chord_label(&mods(true, false), Keysym::b), "Ctrl+B");
        assert_eq!(
            super::chord_label(&mods(false, false), Keysym::Escape),
            "Esc"
        );
        if super::layout().is_some() {
            assert_eq!(super::chord_label(&shift, Keysym::slash), "?");
        }
    }

    #[test]
    fn old_names_still_bind_and_say_they_were_renamed() {
        let (keymap, problems) =
            from_config("[keys]\nnew-cursor-agent = \"Alt+G\"\nopen-project = \"o\"\n");
        let alt = ModifiersState {
            alt: true,
            ..Default::default()
        };
        assert_eq!(keymap.action(&alt, Keysym::g), Some(Action::NewAgent));
        assert_eq!(
            keymap.action(&mods(false, false), Keysym::o),
            Some(Action::NewProject)
        );
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].contains("`new-agent`"), "{problems:?}");
        assert!(problems[1].contains("`open-workspace`"), "{problems:?}");
    }

    #[test]
    fn shifted_characters_follow_the_layout() {
        // US: "@" is Shift and 2, "{" Shift and [, and back again.
        if super::layout().is_none() {
            return;
        }
        let (keymap, problems) =
            from_config("[keys]\nscrollback = \"@\"\nzoom-pane = \"braceleft\"\n");
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            keymap.action(&mods(false, true), Keysym::_2),
            Some(Action::Scrollback)
        );
        assert_eq!(
            keymap.action(&mods(false, false), Keysym::_2),
            Some(Action::GotoTab(1))
        );
        assert_eq!(
            keymap.action(&mods(false, true), Keysym::bracketleft),
            Some(Action::ZoomPane)
        );
        assert_eq!(Chord::parse("Shift+2").map(|c| c.label()), Ok("@".into()));
        assert_eq!(
            Chord::parse("Shift+minus").map(|c| c.label()),
            Ok("_".into())
        );
        assert_eq!(
            Chord::parse("Shift+H").map(|c| c.label()),
            Ok("Shift+H".into())
        );
        assert_eq!(
            Chord::parse("Shift+Tab").map(|c| c.label()),
            Ok("Shift+Tab".into())
        );
        // Nothing types "é" without AltGr: a mistake, not a key never pressed.
        let (keymap, problems) = from_config("[keys]\nscrollback = \"é\"\n");
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("AltGr"), "{problems:?}");
        assert_eq!(
            keymap.action(&mods(false, false), Keysym::bracketleft),
            Some(Action::Scrollback)
        );
    }

    #[test]
    fn a_layout_puts_shifted_characters_where_it_has_them() {
        // On a German layout "?" is Shift and the ß key, "/" Shift and 7,
        // and "[" only with AltGr.
        let Some(de) = super::Layout::compile("de", false) else {
            return;
        };
        let parse = |spec: &str| Chord::parse_in(spec, Some(&de), true);
        let question = parse("?").unwrap();
        assert_eq!((question.mods.shift, question.sym), (true, Keysym::ssharp));
        assert_eq!(question.label_in(Some(&de)), "?");
        let slash = parse("/").unwrap();
        assert_eq!((slash.mods.shift, slash.sym), (true, Keysym::_7));
        assert!(parse("[").is_err());
        // A default gerdr names is kept as on a US layout instead.
        let lenient = Chord::parse_in("[", Some(&de), false).unwrap();
        assert_eq!(lenient.sym, Keysym::bracketleft);
    }

    #[test]
    fn what_may_surprise_is_reported() {
        // An upper case letter is still the plain key.
        let (keymap, problems) = from_config("[keys]\nnew-tab = \"Y\"\n");
        assert_eq!(
            keymap.action(&mods(false, false), Keysym::y),
            Some(Action::NewTab)
        );
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("Shift+Y"), "{problems:?}");
        // Taking another action's default leaves that one without a key.
        let (_, problems) = from_config("[keys]\nnew-tab = \"t\"\n");
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].contains("`toggle-theme` has no key"),
            "{problems:?}"
        );
        // After a Ctrl prefix, Ctrl still held picks X, and Ctrl+X takes
        // pressing Ctrl again: said, as it may surprise.
        let (_, problems) = from_config("[keys]\nnew-tab = \"Ctrl+X\"\n");
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("only by Ctrl"), "{problems:?}");
        assert!(
            problems[0].contains("picks `X`; press Ctrl again for `Ctrl+X`"),
            "{problems:?}"
        );
        let (_, problems) = from_config("[keys]\nprefix = \"Alt+B\"\nnew-tab = \"Ctrl+X\"\n");
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(Chord::parse("Delete").map(|c| c.label()), Ok("Del".into()));
    }

    #[test]
    fn last_project_is_alt_p() {
        let keymap = Keymap::default();
        let alt = ModifiersState {
            alt: true,
            ..Default::default()
        };
        assert_eq!(keymap.action(&alt, Keysym::p), Some(Action::LastProject));
        // Plain l still focuses right.
        assert_eq!(
            keymap.action(&mods(false, false), Keysym::l),
            Some(Action::FocusSide(Side::Right))
        );
    }

    #[test]
    fn pane_moves_have_keys() {
        let keymap = Keymap::default();
        let alt = ModifiersState {
            alt: true,
            ..Default::default()
        };
        assert_eq!(
            keymap.action(&mods(false, true), Keysym::h),
            Some(Action::MovePane(Side::Left))
        );
        assert_eq!(
            keymap.action(&mods(true, true), Keysym::l),
            Some(Action::MovePane(Side::Right))
        );
        assert_eq!(
            keymap.action(&alt, Keysym::j),
            Some(Action::SwapPane(Side::Down))
        );
        assert_eq!(
            keymap.action(&alt, Keysym::l),
            Some(Action::SwapPane(Side::Right))
        );
        assert_eq!(
            keymap.action(&mods(false, false), Keysym::space),
            Some(Action::FlipPane)
        );
        assert_eq!(
            keymap.action(&mods(false, false), Keysym::z),
            Some(Action::ZoomPane)
        );
        assert_eq!(
            keymap.action(&mods(false, false), Keysym::Return),
            Some(Action::SendPrompt)
        );
        // "!" is Shift and the 1 key, which alone still goes to tab 1.
        assert_eq!(
            keymap.action(&mods(false, true), Keysym::_1),
            Some(Action::PaneToNewTab)
        );
        assert_eq!(
            keymap.action(&mods(false, false), Keysym::_1),
            Some(Action::GotoTab(0))
        );
    }

    #[test]
    fn shifted_characters_by_name() {
        let (keymap, problems) = from_config("[keys]\npane-to-new-tab = \"exclam\"\n");
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            keymap.action(&mods(false, true), Keysym::_1),
            Some(Action::PaneToNewTab)
        );
        assert_eq!(Chord::parse("!"), Chord::parse("Shift+1"));
        assert_eq!(Chord::parse("question"), Chord::parse("?"));
        assert_eq!(Chord::parse("exclam").map(|c| c.label()), Ok("!".into()));
    }

    #[test]
    fn shared_modifiers_are_written_once() {
        use super::shared_mods;
        assert_eq!(
            shared_mods(&["Alt+H", "Alt+J", "Alt+K", "Alt+L"]),
            ["Alt+H", "J", "K", "L"]
        );
        assert_eq!(shared_mods(&["Ctrl++", "Ctrl+-"]), ["Ctrl++", "-"]);
        assert_eq!(shared_mods(&["V", "-"]), ["V", "-"]);
        assert_eq!(shared_mods(&["+", "-"]), ["+", "-"]);
        assert_eq!(shared_mods(&["Tab", "Shift+Tab"]), ["Tab", "Shift+Tab"]);
        assert_eq!(shared_mods(&["Shift+V", "Alt+V"]), ["Shift+V", "Alt+V"]);
        assert_eq!(
            shared_mods(&["Ctrl+Up", "Ctrl+Alt+Up"]),
            ["Ctrl+Up", "Ctrl+Alt+Up"]
        );
    }

    #[test]
    fn default_keys_are_all_different() {
        let chords: Vec<Chord> = ACTIONS
            .iter()
            .map(|(_, _, _, spec)| Chord::parse_lenient(spec).expect("default key"))
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
        // The theme's and stop's keys were taken.
        assert_eq!(problems.len(), 2, "{problems:?}");
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
        let shift_minus = Chord::parse("Shift+minus").unwrap().label();
        assert!(
            help.rows
                .contains(&(format!("{shift_minus} / S"), "Split pane down".into()))
        );
        assert!(help.rows.contains(&("V".into(), "Split pane right".into())));
        assert!(!help.rows.iter().any(|(_, label)| label.contains("theme")));
        assert_eq!(help.rows[help.rows.len() - 2].0, "Ctrl+Space");
    }

    #[test]
    fn config_mistakes() {
        let (keymap, problems) = from_config(
            "[keys]\nprefix = \"Hyper+B\"\nfly = \"f\"\nnew-tab = [\"Ctrl+\", \"n\"]\n",
        );
        // Three mistakes, and next-tab lost its n to new-tab.
        assert_eq!(problems.len(), 4, "{problems:?}");
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

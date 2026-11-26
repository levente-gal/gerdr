//! The menu Codex, OpenCode, pi and Antigravity open on `/model`, read off
//! the pane's screen so the chat and the phone show it as gerdr's own: a
//! row picked there is picked in the agent's menu with its arrow keys, and
//! the other keys go to it as typed (its search, its effort). Claude Code
//! and Cursor take the model on the line instead (`model`).

use super::menu::Menu;
use crate::{ai_work::AgentKind, scrollback::Styled};

/// The line that opens it.
pub const LINE: &str = "/model";

// shortcut: the menus' words and marks are as each CLI drew them when its
// `screen.rs` says; recheck them with its modes when one updates.
/// The model menu agent `kind`'s screen `rows` show at their bottom, as a
/// menu picked with the arrows (`Menu::arrows`); none when they show none.
pub fn read(kind: AgentKind, rows: &[(String, Styled)]) -> Option<Menu> {
    let mut m = crate::integrations::of(kind).model_menu(rows)?;
    m.arrows = true;
    for (i, choice) in m.choices.iter_mut().enumerate() {
        choice.n = u8::try_from(i + 1).unwrap_or(u8::MAX);
    }
    Some(m)
}

/// Whether `screen` (as herdr reads it, plain or with its escapes) shows
/// any agent's model menu: the line that opened it was taken, and an Enter
/// now would pick a model.
pub fn shows(screen: &str) -> bool {
    let rows = crate::scrollback::parse_ansi(screen);
    (crate::integrations::ALL.iter()).any(|agent| read(agent.kind(), &rows).is_some())
}

/// The keys that pick the row labelled `label` in menu `m`, as herdr names
/// them: the arrow moved onto it, then Enter. None when it shows no such
/// row, or no arrow.
pub fn keys_to(m: &Menu, label: &str) -> Option<Vec<&'static str>> {
    let at = m.choices.iter().position(|c| c.picked)?;
    let to = m.choices.iter().position(|c| c.label == label)?;
    let way = if to > at { "Down" } else { "Up" };
    let mut keys = vec![way; to.abs_diff(at)];
    keys.push("Enter");
    Some(keys)
}

/// The model agent `kind`'s screen says it is on, under its reply box
/// (`rows` its last ones); none when it says none (a menu covers it).
pub fn now(kind: AgentKind, rows: &[&str]) -> Option<String> {
    let rows: Vec<&str> = (rows.iter().rev())
        .map(|row| row.trim())
        .filter(|row| !row.is_empty())
        .take(6)
        .collect();
    let said = crate::integrations::of(kind).model_on_screen(&rows)?;
    Some(said.trim().to_owned()).filter(|said| !said.is_empty())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::scrollback::parse_ansi;

    pub(crate) fn screen(ansi: &str) -> Vec<(String, Styled)> {
        parse_ansi(ansi)
    }

    pub(crate) fn labels(m: &Menu) -> Vec<&str> {
        m.choices.iter().map(|c| c.label.as_str()).collect()
    }

    pub(crate) fn picked(m: &Menu) -> &str {
        (m.choices.iter().find(|c| c.picked)).map_or("", |c| c.label.as_str())
    }

    #[test]
    fn a_menu_on_the_plain_screen_shows_and_none_when_idle() {
        let plain =
            |ansi: &str| -> String { screen(ansi).into_iter().map(|(t, _)| t + "\n").collect() };
        for ansi in [
            include_str!("fixtures/model_menu/codex.ansi"),
            include_str!("fixtures/model_menu/oc.ansi"),
            include_str!("fixtures/model_menu/pi-few.ansi"),
            include_str!("fixtures/model_menu/agy-search.ansi"),
        ] {
            assert!(shows(&plain(ansi)));
        }
        for ansi in [
            include_str!("fixtures/model_menu/codex-idle.ansi"),
            include_str!("fixtures/model_menu/oc-idle.ansi"),
            include_str!("fixtures/model_menu/pi-idle.ansi"),
            include_str!("fixtures/model_menu/agy-idle.ansi"),
        ] {
            assert!(!shows(&plain(ansi)));
        }
    }

    #[test]
    fn the_model_under_each_reply_box_reads() {
        let rows =
            |ansi: &str| -> Vec<String> { screen(ansi).into_iter().map(|(t, _)| t).collect() };
        let now_in = |kind, ansi: &str| {
            let rows = rows(ansi);
            let texts: Vec<&str> = rows.iter().map(String::as_str).collect();
            now(kind, &texts)
        };
        let codex = include_str!("fixtures/model_menu/codex-idle.ansi");
        assert_eq!(
            now_in(AgentKind::Codex, codex).as_deref(),
            Some("GPT-6.2-Astra")
        );
        let oc = include_str!("fixtures/model_menu/oc-idle.ansi");
        assert_eq!(
            now_in(AgentKind::OpenCode, oc).as_deref(),
            Some("GPT-5.6 Sol OpenAI")
        );
        let pi = include_str!("fixtures/model_menu/pi-idle.ansi");
        assert_eq!(
            now_in(AgentKind::Pi, pi).as_deref(),
            Some("claude-opus-5-5")
        );
        let agy = include_str!("fixtures/model_menu/agy-idle.ansi");
        assert_eq!(
            now_in(AgentKind::Antigravity, agy).as_deref(),
            Some("Gemini 4 Hydrogen")
        );
        assert_eq!(
            now(AgentKind::Antigravity, &["  Gemini 4 Hydrogen · high"]).as_deref(),
            Some("Gemini 4 Hydrogen")
        );
        // A model without efforts; another mode.
        let agy = |row: &str| now(AgentKind::Antigravity, &[row]);
        let sonnet = "? for shortcuts      auto · Claude Sonnet 4.6 (Thinking)";
        assert_eq!(agy(sonnet).as_deref(), Some("Claude Sonnet 4.6 (Thinking)"));
        let plan = "  plan · GPT-OSS 120B (Medium) · low";
        assert_eq!(agy(plan).as_deref(), Some("GPT-OSS 120B (Medium)"));
        assert_eq!(agy("? for shortcuts"), None);
        assert_eq!(now(AgentKind::Claude, &["anything"]), None);
    }
}

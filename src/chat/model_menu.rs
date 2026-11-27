//! The menu Codex, OpenCode, pi and Antigravity open on `/model` (and on
//! their effort's line: `Integration::effort_line`), read off
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
    let way = match (to > at, m.across) {
        (true, false) => "Down",
        (false, false) => "Up",
        (true, true) => "Right",
        (false, true) => "Left",
    };
    let mut keys = vec![way; to.abs_diff(at)];
    keys.push("Enter");
    Some(keys)
}

/// The model agent `kind`'s screen says it is on, under its reply box
/// (`rows` its last ones); none when it says none (a menu covers it).
pub fn now(kind: AgentKind, rows: &[&str]) -> Option<String> {
    said(rows, |rows| {
        crate::integrations::of(kind).model_on_screen(rows)
    })
}

/// The effort agent `kind`'s screen says it is on, as `now`.
pub fn effort_now(kind: AgentKind, rows: &[&str]) -> Option<String> {
    said(rows, |rows| {
        crate::integrations::of(kind).effort_on_screen(rows)
    })
}

/// What `read` finds in `rows`' last ones that aren't empty, trimmed, last
/// first.
fn said<'a>(rows: &[&'a str], read: impl Fn(&[&'a str]) -> Option<&'a str>) -> Option<String> {
    let rows: Vec<&str> = (rows.iter().rev())
        .map(|row| row.trim())
        .filter(|row| !row.is_empty())
        .take(6)
        .collect();
    let said = read(&rows)?;
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
    fn each_agents_effort_menu_reads_as_its_model_menu_does() {
        let pi = read(
            AgentKind::Pi,
            &screen(include_str!("fixtures/model_menu/pi-thinking.ansi")),
        );
        let pi = pi.expect("pi's /thinking");
        assert_eq!(pi.ask[0].0, "Thinking level");
        assert_eq!(
            labels(&pi),
            ["low", "medium (current)", "high", "xhigh", "max"]
        );
        assert_eq!(pi.choices[0].detail, "Light reasoning (~2k tokens)");
        assert_eq!(picked(&pi), "medium (current)");
        assert_eq!(
            keys_to(&pi, "max"),
            Some(vec!["Down", "Down", "Down", "Enter"])
        );

        let oc = screen(include_str!("fixtures/model_menu/oc-variants.ansi"));
        let oc = read(AgentKind::OpenCode, &oc).expect("OpenCode's /variants");
        assert_eq!(oc.ask[0].0, "Select variant");
        assert_eq!(
            labels(&oc),
            [
                "Default",
                "none",
                "low",
                "medium",
                "high (current)",
                "xhigh",
                "max"
            ]
        );

        let agy = screen(include_str!("fixtures/model_menu/agy-effort.ansi"));
        let agy = read(AgentKind::Antigravity, &agy).expect("Antigravity's /effort");
        assert_eq!(agy.ask[0].0, "Set effort");
        assert!(agy.across && agy.sideways);
        assert_eq!(labels(&agy), ["low", "medium", "high (current)", "xhigh"]);
        assert_eq!(picked(&agy), "high (current)");
        assert_eq!(keys_to(&agy, "low"), Some(vec!["Left", "Left", "Enter"]));
        assert_eq!(keys_to(&agy, "xhigh"), Some(vec!["Right", "Enter"]));

        let plain =
            |ansi: &str| -> String { screen(ansi).into_iter().map(|(t, _)| t + "\n").collect() };
        for ansi in [
            include_str!("fixtures/model_menu/pi-thinking.ansi"),
            include_str!("fixtures/model_menu/oc-variants.ansi"),
            include_str!("fixtures/model_menu/agy-effort.ansi"),
        ] {
            assert!(shows(&plain(ansi)), "an Enter would pick an effort");
        }
    }

    #[test]
    fn the_effort_under_each_reply_box_reads() {
        let rows =
            |ansi: &str| -> Vec<String> { screen(ansi).into_iter().map(|(t, _)| t).collect() };
        let effort_in = |kind, ansi: &str| {
            let rows = rows(ansi);
            let texts: Vec<&str> = rows.iter().map(String::as_str).collect();
            effort_now(kind, &texts)
        };
        let pi = include_str!("fixtures/model_menu/pi-idle.ansi");
        assert_eq!(effort_in(AgentKind::Pi, pi).as_deref(), Some("medium"));
        let oc = include_str!("fixtures/model_menu/oc-idle.ansi");
        assert_eq!(effort_in(AgentKind::OpenCode, oc).as_deref(), Some("high"));
        let agy = |row: &str| effort_now(AgentKind::Antigravity, &[row]);
        assert_eq!(
            agy("  auto · Gemini 4 Hydrogen · high").as_deref(),
            Some("high")
        );
        assert_eq!(agy("  Gemini 4 Hydrogen · low").as_deref(), Some("low"));
        assert_eq!(
            agy("? for shortcuts      auto · Claude Sonnet 4.6 (Thinking)"),
            None
        );
        let codex = |row: &str| effort_now(AgentKind::Codex, &[row]);
        assert_eq!(
            codex("  GPT-6.1 Terra medium · ~/work").as_deref(),
            Some("medium")
        );
        assert_eq!(codex("  GPT-6.1 Terra · ~/work"), None);
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

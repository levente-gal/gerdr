//! The menu Codex, OpenCode, pi and Antigravity open on `/model`, read off
//! the pane's screen so the chat and the phone show it as gerdr's own: a
//! row picked there is picked in the agent's menu with its arrow keys, and
//! the other keys go to it as typed (its search, its effort). Claude Code
//! and Cursor take the model on the line instead (`model`).

use super::menu::{Choice, ChoiceKind, Menu};
use crate::{
    ai_work::AgentKind,
    scrollback::{Style, Styled},
};

/// The line that opens it.
pub const LINE: &str = "/model";

/// Whether agent `kind` picks its model in a menu of its own on `LINE`.
pub fn opens(kind: AgentKind) -> bool {
    crate::integrations::of(kind).model_switch() == crate::integrations::ModelSwitch::OwnMenu
}

// shortcut: the menus' words and marks are as the CLIs drew them here
// (Codex 0.171.0, OpenCode 2.3.0, pi 1.6.0, Antigravity CLI 1.5.17); recheck
// them with `mode.rs`'s when one updates.
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

/// pi's: "(anthropic) claude-opus-5-5 • medium" at the right of its last
/// row.
pub(crate) fn pi_now<'a>(rows: &[&'a str]) -> Option<&'a str> {
    rows.iter().take(2).find_map(|row| {
        let right = row.rsplit("  ").next()?.trim();
        let (_, model) = right.strip_prefix('(')?.split_once(") ")?;
        Some(model.split(" • ").next().unwrap_or(model))
    })
}

/// pi's selector under its chat: its search line (">"), the models (→ on
/// the arrow's, ✓ on the current one, the provider in brackets), "(3/65)",
/// and its keys ("Enter to select").
pub(crate) fn pi(rows: &[(String, Styled)]) -> Option<Menu> {
    let keys = (rows.iter()).rposition(|(t, _)| t.contains("Enter to select"))?;
    let field = (rows[..keys].iter()).rposition(|(t, _)| t.trim_start().starts_with('>'))?;
    // The models end at a blank row, its count ("(3/65)", only while they
    // don't all fit) or what it says of the one under the arrow.
    let start = (rows[field + 1..keys].iter())
        .position(|(t, _)| !t.trim().is_empty())
        .map_or(keys, |at| field + 1 + at);
    let count = start
        + (rows[start..keys].iter())
            .position(|(t, _)| {
                let t = t.trim();
                t.is_empty() || t.starts_with('(') || t.starts_with("Model Name:")
            })
            .unwrap_or(keys - start);
    let search = rows[field]
        .0
        .trim()
        .trim_start_matches('>')
        .trim()
        .to_owned();
    let choices = (rows[start..count].iter())
        .map(|(t, _)| t.trim())
        .filter(|t| !t.is_empty())
        .map(|t| {
            let (picked, t) = match t.strip_prefix('→') {
                Some(after) => (true, after.trim_start()),
                None => (false, t),
            };
            let (current, t) = match t.strip_prefix('✓') {
                Some(after) => (true, after.trim_start()),
                None => (false, t),
            };
            let (label, detail) = match t.strip_suffix(']').and_then(|t| t.rsplit_once(" [")) {
                Some((label, provider)) => (label.to_owned(), provider.to_owned()),
                None => (t.to_owned(), String::new()),
            };
            choice(label, detail, picked, current)
        })
        .collect();
    Some(Menu {
        ask: asked("Select model", &search, None),
        choices,
        ..Menu::default()
    })
}

/// What the panel says above the rows: the menu's title, what is typed in
/// its search, and its effort.
pub(crate) fn asked(title: &str, search: &str, effort: Option<&str>) -> Vec<(String, bool)> {
    let mut ask = vec![(title.to_owned(), true)];
    if !search.is_empty() {
        ask.push((format!("Search: {search}"), false));
    }
    if let Some(effort) = effort {
        ask.push((format!("Effort: {effort} (← →)"), false));
    }
    ask
}

pub(crate) fn choice(label: String, detail: String, picked: bool, current: bool) -> Choice {
    let label = match current {
        true => format!("{label} (current)"),
        false => label,
    };
    Choice {
        n: 0,
        label,
        detail,
        picked,
        checked: None,
        kind: ChoiceKind::Pick,
    }
}

/// `text` split at its first gap of two blanks or more: a row's label and
/// what it says more, the blanks inside the label made one.
pub(crate) fn wide_split(text: &str) -> (String, String) {
    let (label, more) = text.trim().split_once("  ").unwrap_or((text.trim(), ""));
    let label = label.split_whitespace().collect::<Vec<_>>().join(" ");
    let more = more.split_whitespace().collect::<Vec<_>>().join(" ");
    match more.strip_prefix("(current)") {
        // Codex pads the label before its mark.
        Some(rest) => (format!("{label} (current)"), rest.trim().to_owned()),
        None => (label, more),
    }
}

/// Where `word` starts in `text`, in characters.
pub(crate) fn col_of(text: &str, word: &str) -> Option<usize> {
    let at = text.find(word)?;
    Some(text[..at].chars().count())
}

/// The characters of `text` from `from` up to `to`.
pub(crate) fn cut(text: &str, from: usize, to: usize) -> String {
    text.chars()
        .skip(from)
        .take(to.saturating_sub(from))
        .collect()
}

/// The style of character `col` of a row styled `styles`.
pub(crate) fn style_at(styles: &Styled, col: usize) -> Option<&Style> {
    styles
        .iter()
        .find(|(r, _)| r.contains(&col))
        .map(|(_, s)| s)
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
    fn pis_selector_reads_with_its_providers_and_search() {
        let rows = screen(include_str!("fixtures/model_menu/pi.ansi"));
        let m = read(AgentKind::Pi, &rows).expect("a menu");
        assert_eq!(m.choices[0].label, "claude-opus-5-5 (current)");
        assert_eq!(m.choices[0].detail, "anthropic");
        assert_eq!(picked(&m), "claude-fable-5");
        assert_eq!(m.ask.len(), 1);

        let rows = screen(include_str!("fixtures/model_menu/pi-search.ansi"));
        let m = read(AgentKind::Pi, &rows).expect("a menu searched");
        assert_eq!(m.ask[1].0, "Search: son");
        assert_eq!(picked(&m), "claude-sonnet-4-5 (Claude Sonnet 4.5 (latest))");

        // All fit: no count under them.
        let rows = screen(include_str!("fixtures/model_menu/pi-few.ansi"));
        let m = read(AgentKind::Pi, &rows).expect("a menu of a few");
        assert_eq!(m.choices.len(), 8);
        assert_eq!(picked(&m), "claude-opus-5-5");
        assert_eq!(m.ask[1].0, "Search: opus-5-5");

        let idle = screen(include_str!("fixtures/model_menu/pi-idle.ansi"));
        assert_eq!(read(AgentKind::Pi, &idle), None);
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

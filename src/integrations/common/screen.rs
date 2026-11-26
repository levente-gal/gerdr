//! Reading what an agent's screen shows: its menus' rows cut by column,
//! and the words several agents draw alike.

use crate::{
    chat::menu::{Choice, ChoiceKind},
    scrollback::{Style, Styled},
};

/// What Codex and Cursor say after their mode.
pub const CYCLE: &str = "(shift+tab to cycle)";

/// What the panel says above the rows: the menu's title, what is typed in
/// its search, and its effort.
pub fn asked(title: &str, search: &str, effort: Option<&str>) -> Vec<(String, bool)> {
    let mut ask = vec![(title.to_owned(), true)];
    if !search.is_empty() {
        ask.push((format!("Search: {search}"), false));
    }
    if let Some(effort) = effort {
        ask.push((format!("Effort: {effort} (← →)"), false));
    }
    ask
}

pub fn choice(label: String, detail: String, picked: bool, current: bool) -> Choice {
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
pub fn wide_split(text: &str) -> (String, String) {
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
pub fn col_of(text: &str, word: &str) -> Option<usize> {
    let at = text.find(word)?;
    Some(text[..at].chars().count())
}

/// The characters of `text` from `from` up to `to`.
pub fn cut(text: &str, from: usize, to: usize) -> String {
    text.chars()
        .skip(from)
        .take(to.saturating_sub(from))
        .collect()
}

/// The style of character `col` of a row styled `styles`.
pub fn style_at(styles: &Styled, col: usize) -> Option<&Style> {
    styles
        .iter()
        .find(|(r, _)| r.contains(&col))
        .map(|(_, s)| s)
}

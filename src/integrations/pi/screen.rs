//! What pi's screen shows: its model selector, and the model it is on.

use crate::{
    chat::{
        menu::Menu,
        model_menu::{asked, choice},
    },
    scrollback::Styled,
};

// shortcut: its words and marks are as pi 1.6.0 drew them; recheck them
// when it updates.
/// pi's: "(anthropic) claude-opus-5-5 • medium" at the right of its last
/// row.
pub(crate) fn model_now<'a>(rows: &[&'a str]) -> Option<&'a str> {
    rows.iter().take(2).find_map(|row| {
        let right = row.rsplit("  ").next()?.trim();
        let (_, model) = right.strip_prefix('(')?.split_once(") ")?;
        Some(model.split(" • ").next().unwrap_or(model))
    })
}

/// pi's selector under its chat: its search line (">"), the models (→ on
/// the arrow's, ✓ on the current one, the provider in brackets), "(3/65)",
/// and its keys ("Enter to select").
pub(crate) fn model_menu(rows: &[(String, Styled)]) -> Option<Menu> {
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
        search: true,
        ..Menu::default()
    })
}

#[cfg(test)]
mod tests {
    use crate::{
        ai_work::AgentKind,
        chat::model_menu::{
            read,
            tests::{picked, screen},
        },
    };

    #[test]
    fn pis_selector_reads_with_its_providers_and_search() {
        let rows = screen(include_str!("../../chat/fixtures/model_menu/pi.ansi"));
        let m = read(AgentKind::Pi, &rows).expect("a menu");
        assert_eq!(m.choices[0].label, "claude-opus-5-5 (current)");
        assert_eq!(m.choices[0].detail, "anthropic");
        assert_eq!(picked(&m), "claude-fable-5");
        assert_eq!(m.ask.len(), 1);

        let rows = screen(include_str!(
            "../../chat/fixtures/model_menu/pi-search.ansi"
        ));
        let m = read(AgentKind::Pi, &rows).expect("a menu searched");
        assert_eq!(m.ask[1].0, "Search: son");
        assert_eq!(picked(&m), "claude-sonnet-4-5 (Claude Sonnet 4.5 (latest))");

        // All fit: no count under them.
        let rows = screen(include_str!("../../chat/fixtures/model_menu/pi-few.ansi"));
        let m = read(AgentKind::Pi, &rows).expect("a menu of a few");
        assert_eq!(m.choices.len(), 8);
        assert_eq!(picked(&m), "claude-opus-5-5");
        assert_eq!(m.ask[1].0, "Search: opus-5-5");

        let idle = screen(include_str!("../../chat/fixtures/model_menu/pi-idle.ansi"));
        assert_eq!(read(AgentKind::Pi, &idle), None);
    }
}

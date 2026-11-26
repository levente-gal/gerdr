//! What the Antigravity CLI's screen shows: "mode · model · effort" at the
//! right of its last row, and its model menu.

use crate::{
    chat::{
        menu::Menu,
        model_menu::{asked, choice},
    },
    scrollback::Styled,
};

// shortcut: its words and marks are as the Antigravity CLI drew them (its
// modes 1.5.15, its model menu 1.5.17); recheck them when it updates.
/// The Antigravity CLI's: "auto · model · effort" at the right of its last
/// row; its default leaves the model first.
pub(super) fn mode_shown(rows: &[&str]) -> Option<String> {
    let row = rows.iter().take(2).find(|row| row.contains(" · "))?;
    let said = row.split_once(" · ")?.0.split_whitespace().last()?;
    let mode = match said {
        "auto" => "Auto",
        "plan" => "Plan",
        "accept-edits" => "Edits",
        _ => "Default",
    };
    Some(mode.to_owned())
}

/// The Antigravity CLI's: "auto · model · effort" at the right of its last
/// row: its default mode leaves the mode out, a model without efforts the
/// effort.
pub(super) fn model_now<'a>(rows: &[&'a str]) -> Option<&'a str> {
    rows.iter().take(2).find_map(|row| {
        let right = row.rsplit("  ").next()?.trim();
        let mut parts = right.split(" · ").peekable();
        parts.next_if(|p| ["auto", "plan", "accept-edits"].contains(p));
        parts.next().filter(|_| right.contains(" · "))
    })
}

/// Antigravity's: "Switch Model", "Search:", the models (> on the arrow's,
/// "(current)" after the current one), and an effort slider (◉ over its
/// level) that ←/→ move while no search is typed.
pub(super) fn model_menu(rows: &[(String, Styled)]) -> Option<Menu> {
    let top = (rows.iter()).rposition(|(t, _)| t.trim() == "Switch Model")?;
    let mut search = String::new();
    let mut choices = Vec::new();
    let mut effort = None;
    let mut i = top + 1;
    while i < rows.len() {
        let t = rows[i].0.trim();
        if let Some(typed) = t.strip_prefix("Search:") {
            search = typed.trim().to_owned();
        } else if t.starts_with("Effort") {
            // Its levels and what the one picked is for come under it.
            effort = rows
                .get(i + 1)
                .and_then(|(levels, _)| level(&rows[i].0, levels));
            break;
        } else if t.starts_with("Keyboard:") {
            break;
        } else if !t.is_empty() && !t.chars().all(|c| c == '─') {
            let (picked, t) = match t.strip_prefix("> ") {
                Some(after) => (true, after.trim()),
                None => (false, t),
            };
            let (current, t) = match t.strip_suffix("(current)") {
                Some(name) => (true, name.trim()),
                None => (false, t),
            };
            choices.push(choice(t.to_owned(), String::new(), picked, current));
        }
        i += 1;
    }
    Some(Menu {
        ask: asked("Switch model", &search, effort.as_deref()),
        sideways: effort.is_some(),
        choices,
        search: true,
        ..Menu::default()
    })
}

/// The level under the slider's knob (◉) on row `slider`, by the word of
/// row `levels` nearest it.
fn level(slider: &str, levels: &str) -> Option<String> {
    let knob = slider.chars().position(|c| c == '◉')?;
    let mut words = Vec::new();
    let mut start = None;
    for (i, c) in levels.chars().chain([' ']).enumerate() {
        match (c.is_alphabetic(), start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                let word: String = levels.chars().skip(s).take(i - s).collect();
                words.push(((s + i) / 2, word));
                start = None;
            }
            _ => {}
        }
    }
    let nearest = words
        .into_iter()
        .min_by_key(|(mid, _)| mid.abs_diff(knob))?;
    Some(nearest.1)
}

#[cfg(test)]
mod tests {
    use crate::{
        ai_work::AgentKind,
        chat::{menu::Menu, model_menu::read},
        scrollback::{Styled, parse_ansi},
    };

    fn screen(ansi: &str) -> Vec<(String, Styled)> {
        parse_ansi(ansi)
    }

    fn labels(m: &Menu) -> Vec<&str> {
        m.choices.iter().map(|c| c.label.as_str()).collect()
    }

    fn picked(m: &Menu) -> &str {
        (m.choices.iter().find(|c| c.picked)).map_or("", |c| c.label.as_str())
    }

    #[test]
    fn antigravitys_menu_reads_with_its_effort_then_searched_without() {
        let rows = screen(include_str!("../../chat/fixtures/model_menu/agy.ansi"));
        let m = read(AgentKind::Antigravity, &rows).expect("a menu");
        assert_eq!(m.choices.len(), 8);
        assert_eq!(m.choices[0].label, "Gemini 4 Hydrogen (current)");
        assert_eq!(picked(&m), "Gemini 3.8 Flash");
        assert!(m.sideways);
        // The effort the model under the arrow would get.
        assert_eq!(m.ask[1].0, "Effort: medium (← →)");

        let rows = screen(include_str!(
            "../../chat/fixtures/model_menu/agy-search.ansi"
        ));
        let m = read(AgentKind::Antigravity, &rows).expect("a menu searched");
        assert_eq!(labels(&m), ["Claude Sonnet 4.6 (Thinking)"]);
        assert_eq!(m.ask[1].0, "Search: son");
        assert!(!m.sideways);

        let idle = screen(include_str!("../../chat/fixtures/model_menu/agy-idle.ansi"));
        assert_eq!(read(AgentKind::Antigravity, &idle), None);
    }
}

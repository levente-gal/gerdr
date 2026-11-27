//! What OpenCode's screen shows: its agent (mode) and model under the
//! prompt, and its model dialog.

use crate::{
    chat::menu::Menu,
    integrations::{
        SidePanel,
        common::screen::{asked, choice, col_of, cut, style_at, wide_split},
    },
    scrollback::Styled,
};

// shortcut: its words and marks are as OpenCode 2.3.0 drew them; recheck
// them when it updates.
/// OpenCode's: the agent's name heads the line under the prompt, "Build ·
/// model".
pub(super) fn mode_shown(rows: &[&str]) -> Option<String> {
    rows.iter().take(8).find_map(|row| {
        let (name, _) = row.strip_prefix('┃')?.split_once(" · ")?;
        Some(name.trim().to_owned()).filter(|name| !name.is_empty())
    })
}

/// OpenCode's: "┃  Build · GPT-5.6 Sol OpenAI · high" under its reply box.
pub(super) fn model_now<'a>(rows: &[&'a str]) -> Option<&'a str> {
    rows.iter().find_map(|row| {
        let row = row.strip_prefix('┃')?;
        row.split(" · ").nth(1)
    })
}

/// OpenCode's variant (its effort), after the model on that same line.
pub(super) fn effort_now<'a>(rows: &[&'a str]) -> Option<&'a str> {
    rows.iter()
        .find_map(|row| row.strip_prefix('┃')?.split(" · ").nth(2))
}

/// The titles of its dialogs gerdr shows: its models, its variants.
const TITLES: [&str; 2] = ["Select model", "Select variant"];

/// OpenCode's dialog over its chat: "Select model … esc", its search
/// field, then models under headings (Recent, each provider); the current
/// one marked ●, the arrow's on a ground of its own.
pub(super) fn model_menu(rows: &[(String, Styled)]) -> Option<Menu> {
    let top = rows.iter().rposition(|(t, _)| {
        let t = t.trim();
        TITLES.iter().any(|w| t.starts_with(w)) && t.ends_with("esc")
    })?;
    let (title, title_styles) = &rows[top];
    let named = *TITLES.iter().find(|w| title.trim().starts_with(*w))?;
    let left = col_of(title, named)?;
    let right = col_of(title, "esc")? + 3;
    // The dialog's own ground: the arrow's row has another.
    let ground = style_at(title_styles, left).and_then(|s| s.bg);
    // Its left edge: the first column on that ground.
    let edge = (0..left)
        .find(|col| style_at(title_styles, *col).and_then(|s| s.bg) == ground)
        .unwrap_or(left);
    let mut search = None;
    let mut choices = Vec::new();
    for (text, styles) in &rows[top + 1..] {
        let row = cut(text, left.saturating_sub(2), right);
        let words = row.trim();
        if words.contains("ctrl+") {
            break;
        }
        // Past the dialog's ground at its left edge: its variants' has no
        // keys row under it.
        if ground.is_some() && style_at(styles, edge).and_then(|s| s.bg) != ground {
            break;
        }
        let at = style_at(styles, left).copied().unwrap_or_default();
        if words.is_empty() {
            continue;
        }
        // Its first row is the search field: a dim "Search" till typed in.
        let Some(_) = search else {
            search = Some(if words == "Search" { "" } else { words }.to_owned());
            continue;
        };
        if words == "No results found" {
            continue;
        }
        let picked = at.bg.is_some() && at.bg != ground;
        // A heading: bold on the dialog's ground.
        if at.bold && !picked {
            continue;
        }
        let current = row.trim_start().starts_with('●');
        let (label, detail) = wide_split(words.trim_start_matches('●').trim());
        choices.push(choice(label, detail, picked, current));
    }
    let search = search?;
    Some(Menu {
        ask: asked(named, &search, None),
        choices,
        search: true,
        ..Menu::default()
    })
}

// shortcut: its side panel's marks are as OpenCode 2.3.0 drew them (2026-11-25);
// recheck them when it updates.
/// Its `/btw` panel, floating over the chat: "esc" right of its question,
/// "copy c" under it, and none until it has answered.
pub(crate) const SIDE_PANEL: SidePanel = SidePanel {
    footers: &["copy c"],
    frame: "",
    question_end: Some("esc"),
    scroll_marks: &[],
    braille: false,
    stars: "",
    expand: None,
};

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
    fn opencodes_dialog_reads_without_its_headings_the_arrow_by_its_ground() {
        let rows = screen(include_str!("../../chat/fixtures/model_menu/oc.ansi"));
        let m = read(AgentKind::OpenCode, &rows).expect("a menu");
        assert_eq!(m.ask, [("Select model".to_owned(), true)]);
        assert_eq!(
            labels(&m)[..3],
            [
                "GPT-5.6 Sol OpenAI (current)",
                "GPT-5.6 Luna OpenAI",
                "Big Pickle OpenCode"
            ]
        );
        assert_eq!(m.choices[2].detail, "Free");
        assert!(!labels(&m).contains(&"Recent") && !labels(&m).contains(&"OpenCode"));
        assert!(labels(&m).contains(&"Tidepool Alpha Free"));
        assert_eq!(picked(&m), "GPT-5.6 Luna OpenAI");

        let rows = screen(include_str!(
            "../../chat/fixtures/model_menu/oc-search.ansi"
        ));
        let m = read(AgentKind::OpenCode, &rows).expect("a menu searched");
        assert_eq!(m.ask[1].0, "Search: son");
        assert!(m.choices.is_empty());

        let idle = screen(include_str!("../../chat/fixtures/model_menu/oc-idle.ansi"));
        assert_eq!(read(AgentKind::OpenCode, &idle), None);
    }
}

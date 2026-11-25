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
    matches!(
        kind,
        AgentKind::Codex | AgentKind::OpenCode | AgentKind::Pi | AgentKind::Antigravity
    )
}

// shortcut: the menus' words and marks are as the CLIs drew them here
// (Codex 0.171.0, OpenCode 2.3.0, pi 1.6.0, Antigravity CLI 1.5.17); recheck
// them with `mode.rs`'s when one updates.
/// The model menu agent `kind`'s screen `rows` show at their bottom, as a
/// menu picked with the arrows (`Menu::arrows`); none when they show none.
pub fn read(kind: AgentKind, rows: &[(String, Styled)]) -> Option<Menu> {
    let mut m = match kind {
        AgentKind::Codex => codex(rows),
        AgentKind::OpenCode => opencode(rows),
        AgentKind::Pi => pi(rows),
        AgentKind::Antigravity => antigravity(rows),
        _ => None,
    }?;
    m.arrows = true;
    for (i, choice) in m.choices.iter_mut().enumerate() {
        choice.n = u8::try_from(i + 1).unwrap_or(u8::MAX);
    }
    Some(m)
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
    let said = match kind {
        // "GPT-6.1 Terra medium · ~/folder": the effort after the model.
        AgentKind::Codex => rows.iter().find_map(|row| {
            let (model, place) = row.split_once(" · ")?;
            if !place.starts_with(['~', '/']) {
                return None;
            }
            let effort = model.rsplit_once(' ').filter(|(_, e)| EFFORTS.contains(e));
            Some(effort.map_or(model, |(model, _)| model))
        })?,
        // "┃  Build · GPT-5.6 Sol OpenAI · high" under its reply box.
        AgentKind::OpenCode => rows.iter().find_map(|row| {
            let row = row.strip_prefix('┃')?;
            row.split(" · ").nth(1)
        })?,
        // "(anthropic) claude-opus-5-5 • medium" at the right of its last row.
        AgentKind::Pi => rows.iter().take(2).find_map(|row| {
            let right = row.rsplit("  ").next()?.trim();
            let (_, model) = right.strip_prefix('(')?.split_once(") ")?;
            Some(model.split(" • ").next().unwrap_or(model))
        })?,
        // "auto · model · effort" at the right of its last row; its
        // default mode leaves the mode out.
        AgentKind::Antigravity => rows.iter().take(2).find_map(|row| {
            let right = row.rsplit("  ").next()?.trim();
            let parts: Vec<&str> = right.split(" · ").collect();
            match parts.len() {
                3 => Some(parts[1]),
                2 => Some(parts[0]),
                _ => None,
            }
        })?,
        _ => return None,
    };
    Some(said.trim().to_owned()).filter(|said| !said.is_empty())
}

/// The reasoning efforts Codex writes after its model.
const EFFORTS: [&str; 7] = ["minimal", "low", "medium", "high", "xhigh", "max", "ultra"];

/// Codex's: "Select Model and Effort", then the effort of the model picked
/// ("Select Reasoning Level for …", "Advanced Reasoning"): numbered rows,
/// the arrow (›) on one, what each is for after it.
fn codex(rows: &[(String, Styled)]) -> Option<Menu> {
    const TITLES: [&str; 3] = [
        "Select Model",
        "Select Reasoning Level",
        "Advanced Reasoning",
    ];
    let top = (rows.iter()).rposition(|(t, _)| TITLES.iter().any(|w| t.trim().starts_with(w)))?;
    let mut m = super::menu::menu(&rows[top..])?;
    for choice in &mut m.choices {
        let (label, detail) = wide_split(&choice.label);
        (choice.label, choice.detail) = (label, detail);
    }
    Some(m)
}

/// OpenCode's dialog over its chat: "Select model … esc", its search
/// field, then models under headings (Recent, each provider); the current
/// one marked ●, the arrow's on a ground of its own.
fn opencode(rows: &[(String, Styled)]) -> Option<Menu> {
    let top = rows.iter().rposition(|(t, _)| {
        let t = t.trim();
        t.starts_with("Select model") && t.ends_with("esc")
    })?;
    let (title, title_styles) = &rows[top];
    let left = col_of(title, "Select model")?;
    let right = col_of(title, "esc")? + 3;
    // The dialog's own ground: the arrow's row has another.
    let ground = style_at(title_styles, left).and_then(|s| s.bg);
    let mut search = None;
    let mut choices = Vec::new();
    for (text, styles) in &rows[top + 1..] {
        let row = cut(text, left.saturating_sub(2), right);
        let words = row.trim();
        if words.contains("ctrl+") {
            break;
        }
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
        let at = style_at(styles, left).copied().unwrap_or_default();
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
        ask: asked("Select model", &search, None),
        choices,
        ..Menu::default()
    })
}

/// pi's selector under its chat: its search line (">"), the models (→ on
/// the arrow's, ✓ on the current one, the provider in brackets), "(3/65)",
/// and its keys ("Enter to select").
fn pi(rows: &[(String, Styled)]) -> Option<Menu> {
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

/// Antigravity's: "Switch Model", "Search:", the models (> on the arrow's,
/// "(current)" after the current one), and an effort slider (◉ over its
/// level) that ←/→ move while no search is typed.
fn antigravity(rows: &[(String, Styled)]) -> Option<Menu> {
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

/// What the panel says above the rows: the menu's title, what is typed in
/// its search, and its effort.
fn asked(title: &str, search: &str, effort: Option<&str>) -> Vec<(String, bool)> {
    let mut ask = vec![(title.to_owned(), true)];
    if !search.is_empty() {
        ask.push((format!("Search: {search}"), false));
    }
    if let Some(effort) = effort {
        ask.push((format!("Effort: {effort} (← →)"), false));
    }
    ask
}

fn choice(label: String, detail: String, picked: bool, current: bool) -> Choice {
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
fn wide_split(text: &str) -> (String, String) {
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
fn col_of(text: &str, word: &str) -> Option<usize> {
    let at = text.find(word)?;
    Some(text[..at].chars().count())
}

/// The characters of `text` from `from` up to `to`.
fn cut(text: &str, from: usize, to: usize) -> String {
    text.chars()
        .skip(from)
        .take(to.saturating_sub(from))
        .collect()
}

/// The style of character `col` of a row styled `styles`.
fn style_at(styles: &Styled, col: usize) -> Option<&Style> {
    styles
        .iter()
        .find(|(r, _)| r.contains(&col))
        .map(|(_, s)| s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scrollback::parse_ansi;

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
    fn codexs_model_menu_and_its_effort_step_read_with_what_each_is_for() {
        let rows = screen(include_str!("fixtures/model_menu/codex.ansi"));
        let m = read(AgentKind::Codex, &rows).expect("a menu");
        assert!(m.arrows && !m.sideways);
        assert_eq!(m.ask[0].0, "Select Model and Effort");
        assert_eq!(m.choices.len(), 8);
        assert_eq!(m.choices[0].label, "GPT-6.2-Sol");
        assert_eq!(
            m.choices[0].detail,
            "Fast, go-to model for coding and everyday work."
        );
        assert_eq!(m.choices[1].label, "GPT-6.2-Astra (current)");
        assert_eq!(picked(&m), "GPT-6.1 Terra");
        assert_eq!(m.choices[7].n, 8);

        let rows = screen(include_str!("fixtures/model_menu/codex-effort.ansi"));
        let m = read(AgentKind::Codex, &rows).expect("the effort step");
        assert!(m.ask[0].0.starts_with("Select Reasoning Level for "));
        assert_eq!(labels(&m)[..2], ["Low (default)", "Medium"]);
        assert_eq!(picked(&m), "More reasoning… (current)");

        let rows = screen(include_str!("fixtures/model_menu/codex-advanced.ansi"));
        let m = read(AgentKind::Codex, &rows).expect("its advanced efforts");
        assert_eq!(m.ask[0].0, "Advanced Reasoning");
        assert_eq!(labels(&m), ["Max", "Ultra"]);

        let idle = screen(include_str!("fixtures/model_menu/codex-idle.ansi"));
        assert_eq!(read(AgentKind::Codex, &idle), None);
    }

    #[test]
    fn opencodes_dialog_reads_without_its_headings_the_arrow_by_its_ground() {
        let rows = screen(include_str!("fixtures/model_menu/oc.ansi"));
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

        let rows = screen(include_str!("fixtures/model_menu/oc-search.ansi"));
        let m = read(AgentKind::OpenCode, &rows).expect("a menu searched");
        assert_eq!(m.ask[1].0, "Search: son");
        assert!(m.choices.is_empty());

        let idle = screen(include_str!("fixtures/model_menu/oc-idle.ansi"));
        assert_eq!(read(AgentKind::OpenCode, &idle), None);
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
    fn antigravitys_menu_reads_with_its_effort_then_searched_without() {
        let rows = screen(include_str!("fixtures/model_menu/agy.ansi"));
        let m = read(AgentKind::Antigravity, &rows).expect("a menu");
        assert_eq!(m.choices.len(), 8);
        assert_eq!(m.choices[0].label, "Gemini 4 Hydrogen (current)");
        assert_eq!(picked(&m), "Gemini 3.8 Flash");
        assert!(m.sideways);
        // The effort the model under the arrow would get.
        assert_eq!(m.ask[1].0, "Effort: medium (← →)");

        let rows = screen(include_str!("fixtures/model_menu/agy-search.ansi"));
        let m = read(AgentKind::Antigravity, &rows).expect("a menu searched");
        assert_eq!(labels(&m), ["Claude Sonnet 4.6 (Thinking)"]);
        assert_eq!(m.ask[1].0, "Search: son");
        assert!(!m.sideways);

        let idle = screen(include_str!("fixtures/model_menu/agy-idle.ansi"));
        assert_eq!(read(AgentKind::Antigravity, &idle), None);
    }

    #[test]
    fn a_row_is_picked_with_the_arrow_moved_onto_it_then_enter() {
        let rows = screen(include_str!("fixtures/model_menu/codex.ansi"));
        let m = read(AgentKind::Codex, &rows).expect("a menu");
        assert_eq!(keys_to(&m, "GPT-6.2-Sol"), Some(vec!["Up", "Up", "Enter"]));
        assert_eq!(keys_to(&m, "GPT-6.1 Terra"), Some(vec!["Enter"]));
        assert_eq!(
            keys_to(&m, "GPT-5.6-Sol"),
            Some(vec!["Down", "Down", "Down", "Down", "Down", "Enter"])
        );
        assert_eq!(keys_to(&m, "Nothing"), None);
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
        assert_eq!(now(AgentKind::Claude, &["anything"]), None);
    }
}

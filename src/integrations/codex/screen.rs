//! What Codex's screen shows: its mode, its own `/model` menu and the
//! model under its reply box, and the menu it waits on under a plan.

use crate::{
    chat::menu::Menu,
    integrations::common::screen::{CYCLE, wide_split},
    scrollback::Styled,
};

// shortcut: its words and marks are as Codex 0.171.0 drew them; recheck
// them when it updates.
/// Codex's: its default says nothing.
pub(crate) fn codex_shown(rows: &[&str]) -> Option<String> {
    let said = rows.iter().take(3).find_map(|row| {
        let said = row.split_once(CYCLE)?.0.trim().strip_suffix(" mode")?;
        Some(said.rsplit("  ").next()?.trim().to_owned())
    });
    Some(said.unwrap_or_else(|| "Build".into()))
}

/// Codex's: "GPT-6.1 Terra medium · ~/folder", the effort after the model;
/// `rows` as `now` gives them.
pub(crate) fn codex_now<'a>(rows: &[&'a str]) -> Option<&'a str> {
    rows.iter().find_map(|row| {
        let (model, place) = row.split_once(" · ")?;
        if !place.starts_with(['~', '/']) {
            return None;
        }
        let effort = model.rsplit_once(' ').filter(|(_, e)| EFFORTS.contains(e));
        Some(effort.map_or(model, |(model, _)| model))
    })
}

/// The reasoning efforts Codex writes after its model.
const EFFORTS: [&str; 7] = ["minimal", "low", "medium", "high", "xhigh", "max", "ultra"];

/// Codex's: "Select Model and Effort", then the effort of the model picked
/// ("Select Reasoning Level for …", "Advanced Reasoning"): numbered rows,
/// the arrow (›) on one, what each is for after it.
pub(crate) fn codex(rows: &[(String, Styled)]) -> Option<Menu> {
    const TITLES: [&str; 3] = [
        "Select Model",
        "Select Reasoning Level",
        "Advanced Reasoning",
    ];
    let top = (rows.iter()).rposition(|(t, _)| TITLES.iter().any(|w| t.trim().starts_with(w)))?;
    let mut m = crate::chat::menu::menu(&rows[top..])?;
    for choice in &mut m.choices {
        let (label, detail) = wide_split(&choice.label);
        (choice.label, choice.detail) = (label, detail);
    }
    Some(m)
}

/// The menu Codex waits on under a plan it suggests, where Enter starts
/// building it: its question, its choice to keep planning, and the key that
/// takes that choice (herdr's name). Probed: Codex 0.171's Esc goes back to
/// its input box, still in Plan mode.
pub(crate) const PLAN_MENU: (&str, &str, &str) =
    ("Implement this plan?", "No, stay in Plan mode", "esc");

#[cfg(test)]
mod tests {
    use crate::{
        ai_work::AgentKind,
        chat::model_menu::{
            keys_to, read,
            tests::{labels, picked, screen},
        },
    };

    #[test]
    fn codexs_model_menu_and_its_effort_step_read_with_what_each_is_for() {
        let rows = screen(include_str!("../../chat/fixtures/model_menu/codex.ansi"));
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

        let rows = screen(include_str!("fixtures/codex-effort.ansi"));
        let m = read(AgentKind::Codex, &rows).expect("the effort step");
        assert!(m.ask[0].0.starts_with("Select Reasoning Level for "));
        assert_eq!(labels(&m)[..2], ["Low (default)", "Medium"]);
        assert_eq!(picked(&m), "More reasoning… (current)");

        let rows = screen(include_str!("fixtures/codex-advanced.ansi"));
        let m = read(AgentKind::Codex, &rows).expect("its advanced efforts");
        assert_eq!(m.ask[0].0, "Advanced Reasoning");
        assert_eq!(labels(&m), ["Max", "Ultra"]);

        let idle = screen(include_str!(
            "../../chat/fixtures/model_menu/codex-idle.ansi"
        ));
        assert_eq!(read(AgentKind::Codex, &idle), None);
    }

    #[test]
    fn a_row_is_picked_with_the_arrow_moved_onto_it_then_enter() {
        let rows = screen(include_str!("../../chat/fixtures/model_menu/codex.ansi"));
        let m = read(AgentKind::Codex, &rows).expect("a menu");
        assert_eq!(keys_to(&m, "GPT-6.2-Sol"), Some(vec!["Up", "Up", "Enter"]));
        assert_eq!(keys_to(&m, "GPT-6.1 Terra"), Some(vec!["Enter"]));
        assert_eq!(
            keys_to(&m, "GPT-5.6-Sol"),
            Some(vec!["Down", "Down", "Down", "Down", "Down", "Enter"])
        );
        assert_eq!(keys_to(&m, "Nothing"), None);
    }
}

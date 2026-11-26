//! What the Cursor CLI's screen shows: its mode, and the menu it waits on
//! under a plan.

use crate::chat::mode::CYCLE;

// shortcut: its words are as Cursor 2026.11.22 drew them; recheck them when
// it updates.
/// Cursor's: its default says nothing.
pub(crate) fn cursor_shown(rows: &[&str]) -> Option<String> {
    let mut said = rows.iter().take(4);
    let said = said.find_map(|row| Some(row.split_once(CYCLE)?.0.trim().to_owned()));
    Some(said.unwrap_or_else(|| "Agent".into()))
}

/// The menu Cursor waits on under a plan it suggests, where Enter starts
/// building it: its question, its choice to keep planning, and the key that
/// takes that choice (herdr's name). Probed: Cursor 2026.11.22's `p` opens a
/// box for how to revise the plan.
pub(crate) const PLAN_MENU: (&str, &str, &str) = ("Ready to build?", "No, propose changes", "p");

//! Google's Antigravity CLI (`agy`).

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use super::{Integration, ModelSwitch, Parser, Store, common, common::files::SubFiles};
use crate::{
    ai_work::{
        AgentKind, Chat,
        locate::{self, Found, Homes, Span},
    },
    chat::{
        Source, commands,
        commands::Command,
        live,
        menu::Menu,
        mode, model_menu,
        parse::{self, Reader},
        source,
    },
    scrollback::Styled,
};

pub struct Antigravity;

impl Integration for Antigravity {
    fn kind(&self) -> AgentKind {
        AgentKind::Antigravity
    }

    fn label(&self) -> &'static str {
        "Antigravity"
    }

    fn herdr_target(&self) -> Option<&'static str> {
        Some("antigravity-cli")
    }

    fn store(&self) -> Option<Store> {
        Some(Store::Lines)
    }

    fn parser(&self) -> Box<dyn Parser> {
        Box::<parse::antigravity::Antigravity>::default()
    }

    /// What the Antigravity CLI's subagents wrote since the last read:
    /// each a conversation of its own (`brain/<id>/…`), the one an
    /// `invoke_subagent` call's result names; else a child conversation of
    /// the chat's, as the summaries database says, whose first input is a
    /// prompt the call gave.
    fn subagents(
        &self,
        chat: &Source,
        reader: &mut Reader,
        files: &mut SubFiles,
        chunk: u64,
    ) -> std::io::Result<()> {
        let path = &chat.path;
        // `<home>/brain/<id>/.system_generated/logs/<transcript>`
        let (Some(home), Some(name)) = (path.ancestors().nth(5), path.file_name()) else {
            return Ok(());
        };
        let transcript = |id: &str| {
            (home.join("brain").join(id))
                .join(".system_generated/logs")
                .join(name)
        };
        for child in reader.named_subagents() {
            if child != chat.id && transcript(&child).is_file() {
                reader.take_subagent(&child, "");
            }
        }
        if reader.unfound_subagents() > 0 {
            let db = home.join("conversation_summaries.db");
            find_subagents(&db, &chat.id, reader, files, transcript);
        }
        for child in reader.subagents() {
            let path = transcript(&child);
            if let Err(err) = files.read(reader, &path, &child, "", chunk) {
                // One subagent unread leaves the chat as it is.
                tracing::warn!("agent chat: can't read {}: {err}", path.display());
            }
        }
        Ok(())
    }

    fn guesses(&self) -> bool {
        true
    }

    fn find(
        &self,
        homes: &Homes,
        chat: &Chat,
        id: Option<&str>,
        claimed: &HashSet<String>,
        span: Span,
    ) -> Result<Option<Found>, String> {
        Ok(match id {
            Some(id) => locate::antigravity_by_id(&homes.antigravity, id),
            None => locate::antigravity_by_time(&homes.antigravity, chat, claimed, span),
        })
    }

    fn transcript_at(&self, path: &str, homes: &Homes) -> Option<PathBuf> {
        source::antigravity_transcript(path, homes)
    }

    fn transcripts_under(&self, homes: &Homes) -> Vec<PathBuf> {
        vec![homes.antigravity.join("brain")]
    }

    /// A transcript is in `brain/<id>/.system_generated/logs/`.
    fn id_in_path<'a>(&self, stem: &'a str, path: &'a Path) -> &'a str {
        (path.ancestors().nth(3))
            .and_then(|dir| dir.file_name()?.to_str())
            .unwrap_or(stem)
    }

    fn modes(&self) -> &'static [&'static str] {
        &["Auto", "Plan", "Default", "Edits"]
    }

    fn mode_shown(&self, rows: &[&str]) -> Option<String> {
        mode::antigravity_shown(rows)
    }

    fn model_switch(&self) -> ModelSwitch {
        ModelSwitch::OwnMenu
    }

    fn model_menu(&self, rows: &[(String, Styled)]) -> Option<Menu> {
        model_menu::antigravity(rows)
    }

    fn model_on_screen<'a>(&self, rows: &[&'a str]) -> Option<&'a str> {
        model_menu::antigravity_now(rows)
    }

    fn builtins(&self) -> &'static [(&'static str, &'static str)] {
        commands::ANTIGRAVITY
    }

    fn disk_commands(&self, cwd: &Path, homes: &Homes, _config: &Path, found: &mut Vec<Command>) {
        commands::antigravity_disk(cwd, homes, found);
    }

    fn hook_command(&self) -> Option<&'static str> {
        Some(live::antigravity::HOOK_COMMAND)
    }

    fn run_hook(&self) {
        live::antigravity::run_hook();
    }

    fn install_live(&self, home: &Path) -> Vec<Result<Option<live::Changed>, String>> {
        if !home.join(".gemini/config").is_dir() {
            return Vec::new();
        }
        vec![
            live::hook_command(live::antigravity::HOOK_COMMAND).and_then(|command| {
                live::install(&home.join(".gemini/config/hooks.json"), |text| {
                    live::antigravity::merged(text, &command)
                })
            }),
        ]
    }
}

/// Take the child conversations summaries database `db` has of chat `id`
/// as subagents, by their first input; looked in again when it changed or
/// more are to find.
fn find_subagents(
    db: &Path,
    id: &str,
    reader: &mut Reader,
    files: &mut SubFiles,
    transcript: impl Fn(&str) -> PathBuf,
) {
    let Ok(stamps) = common::files::with_wal(db) else {
        return;
    };
    let now = (stamps, reader.unfound_subagents());
    if files.looked == Some(now) {
        return;
    }
    files.looked = Some(now);
    let children = match children(db, id) {
        Ok(children) => children,
        Err(err) => {
            tracing::warn!("agent chat: can't read {}: {err}", db.display());
            return;
        }
    };
    let taken = reader.subagents();
    for child in children.iter().filter(|c| !taken.contains(c)) {
        match common::files::first_line(&transcript(child), u64::MAX, true) {
            Some(first) => _ = reader.take_subagent(child, &first),
            // Not written yet: looked for again.
            None => files.looked = None,
        }
    }
}

/// The conversations under conversation `id`, as summaries database `db`
/// has them.
fn children(db: &Path, id: &str) -> rusqlite::Result<Vec<String>> {
    let conn = crate::cursor_agents::open_read_only(db)?;
    let mut rows = conn.prepare(
        "SELECT conversation_id FROM conversation_summaries WHERE parent_conversation_id = ?1",
    )?;
    rows.query_map([id], |row| row.get(0))?.collect()
}

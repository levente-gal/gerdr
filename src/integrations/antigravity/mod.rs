//! Google's Antigravity CLI (`agy`). Its chats are transcripts of lines,
//! one per conversation, under `~/.gemini/antigravity-cli/brain/<id>/`;
//! a subagent is a conversation of its own. Its live word comes from hooks
//! in `~/.gemini/config/hooks.json` that run `gerdr agy-hook`.

mod commands;
pub(crate) mod find;
pub(crate) mod live;
mod parse;
mod screen;
mod subagents;

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use super::{Integration, ModelSwitch, Parser, Store, common::files::SubFiles};
use crate::{
    ai_work::{
        AgentKind, Chat,
        locate::{Found, Homes, Span},
    },
    chat::{Source, commands::Command, live as hub, menu::Menu, parse::Reader},
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
        Box::<parse::Antigravity>::default()
    }

    fn subagents(
        &self,
        chat: &Source,
        reader: &mut Reader,
        files: &mut SubFiles,
        chunk: u64,
    ) -> std::io::Result<()> {
        subagents::read(chat, reader, files, chunk)
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
            Some(id) => find::by_id(&homes.antigravity, id),
            None => find::by_time(&homes.antigravity, chat, claimed, span),
        })
    }

    fn transcript_at(&self, path: &str, homes: &Homes) -> Option<PathBuf> {
        find::transcript_at(path, homes)
    }

    fn transcripts_under(&self, homes: &Homes) -> Vec<PathBuf> {
        vec![homes.antigravity.join("brain")]
    }

    /// A transcript is in `brain/<id>/.system_generated/logs/`.
    fn id_in_path<'a>(&self, stem: &'a str, path: &'a Path) -> &'a str {
        find::id_of(path).unwrap_or(stem)
    }

    fn modes(&self) -> &'static [&'static str] {
        &["Auto", "Plan", "Default", "Edits"]
    }

    fn mode_shown(&self, rows: &[&str]) -> Option<String> {
        screen::mode_shown(rows)
    }

    fn model_switch(&self) -> ModelSwitch {
        ModelSwitch::OwnMenu
    }

    fn model_menu(&self, rows: &[(String, Styled)]) -> Option<Menu> {
        screen::model_menu(rows)
    }

    fn model_on_screen<'a>(&self, rows: &[&'a str]) -> Option<&'a str> {
        screen::model_now(rows)
    }

    fn builtins(&self) -> &'static [(&'static str, &'static str)] {
        commands::BUILTINS
    }

    fn disk_commands(&self, cwd: &Path, homes: &Homes, _config: &Path, found: &mut Vec<Command>) {
        commands::on_disk(cwd, homes, found);
    }

    fn hook_command(&self) -> Option<&'static str> {
        Some(live::HOOK_COMMAND)
    }

    fn run_hook(&self) {
        live::run_hook();
    }

    fn install_live(&self, home: &Path) -> Vec<Result<Option<hub::Changed>, String>> {
        if !home.join(".gemini/config").is_dir() {
            return Vec::new();
        }
        vec![hub::hook_command(live::HOOK_COMMAND).and_then(|command| {
            hub::install(&home.join(".gemini/config/hooks.json"), |text| {
                live::merged(text, &command)
            })
        })]
    }
}

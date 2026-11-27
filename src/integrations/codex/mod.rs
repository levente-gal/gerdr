//! Codex. Its chats are rollouts, JSON lines under
//! `~/.codex/sessions/<y>/<m>/<d>/`, read as they grow. It has no live
//! hook: its word reaches gerdr only through its rollout and its screen.

mod commands;
pub(crate) mod find;
mod parse;
mod screen;
mod stats;

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use super::{Integration, ModelSwitch, Parser, Store};
use crate::{
    ai_work::{
        AgentKind, Chat,
        context::Context,
        locate::{Found, Homes, Span},
        usage::Message,
    },
    chat::{commands::Command, menu::Menu},
    scrollback::Styled,
};

pub struct Codex;

impl Integration for Codex {
    fn kind(&self) -> AgentKind {
        AgentKind::Codex
    }

    fn label(&self) -> &'static str {
        "Codex"
    }

    fn command(&self) -> Option<&'static str> {
        Some("codex")
    }

    fn adds_dirs(&self) -> bool {
        true
    }

    fn store(&self) -> Option<Store> {
        Some(Store::Lines)
    }

    fn parser(&self) -> Box<dyn Parser> {
        Box::new(parse::Codex)
    }

    fn guesses(&self) -> bool {
        true
    }

    fn backfill_by_time(&self) -> bool {
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
            Some(id) => find::codex_by_id(&homes.codex, id),
            None => find::codex_by_time(&homes.codex, chat, claimed, span),
        })
    }

    fn transcript_at(&self, path: &str, homes: &Homes) -> Option<PathBuf> {
        find::codex_rollout(path, &homes.codex)
    }

    fn transcripts_under(&self, homes: &Homes) -> Vec<PathBuf> {
        vec![homes.codex.join("sessions")]
    }

    /// A rollout is `rollout-<yyyy-mm-ddThh-mm-ss>-<id>`.
    fn id_in_path<'a>(&self, stem: &'a str, _path: &'a Path) -> &'a str {
        stem.get("rollout-".len() + 20..).unwrap_or(stem)
    }

    fn remote_script(&self, chat: &Chat, id: Option<&str>) -> Option<String> {
        Some(find::codex_script(chat, id))
    }

    fn remote_guesses(&self) -> bool {
        true
    }

    fn has_context(&self) -> bool {
        true
    }

    fn context_in(&self, tail: &[u8]) -> Option<Context> {
        stats::codex_context(tail)
    }

    fn usage(&self, raw: &[u8]) -> Vec<Message> {
        stats::codex(raw)
    }

    fn modes(&self) -> &'static [&'static str] {
        &["Build", "Plan"]
    }

    fn mode_shown(&self, rows: &[&str]) -> Option<String> {
        screen::codex_shown(rows)
    }

    fn model_switch(&self) -> ModelSwitch {
        ModelSwitch::OwnMenu
    }

    fn model_menu(&self, rows: &[(String, Styled)]) -> Option<Menu> {
        screen::codex(rows)
    }

    fn model_on_screen<'a>(&self, rows: &[&'a str]) -> Option<&'a str> {
        screen::codex_now(rows)
    }

    fn effort_switch(&self) -> ModelSwitch {
        ModelSwitch::OwnMenu
    }

    fn effort_line(&self) -> Option<(&'static str, bool)> {
        Some(("/model", true))
    }

    fn effort_on_screen<'a>(&self, rows: &[&'a str]) -> Option<&'a str> {
        screen::codex_effort(rows)
    }

    fn plan_menu(&self) -> Option<(&'static str, &'static str, &'static str)> {
        Some(screen::PLAN_MENU)
    }

    fn builtins(&self) -> &'static [(&'static str, &'static str)] {
        commands::CODEX
    }

    fn disk_commands(&self, _cwd: &Path, homes: &Homes, _config: &Path, found: &mut Vec<Command>) {
        commands::codex_disk(homes, found);
    }
}

//! Codex.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use super::{Integration, ModelSwitch, Store};
use crate::{
    ai_work::{
        AgentKind, Chat, context,
        context::Context,
        locate::{self, Found, Homes, Span},
        usage::{self, Message},
    },
    chat::{commands, commands::Command, menu::Menu, mode, model_menu, source},
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

    fn store(&self) -> Option<Store> {
        Some(Store::Lines)
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
            Some(id) => locate::codex_by_id(&homes.codex, id),
            None => locate::codex_by_time(&homes.codex, chat, claimed, span),
        })
    }

    fn transcript_at(&self, path: &str, homes: &Homes) -> Option<PathBuf> {
        source::codex_rollout(path, &homes.codex)
    }

    fn transcripts_under(&self, homes: &Homes) -> Vec<PathBuf> {
        vec![homes.codex.join("sessions")]
    }

    /// A rollout is `rollout-<yyyy-mm-ddThh-mm-ss>-<id>`.
    fn id_in_path<'a>(&self, stem: &'a str, _path: &'a Path) -> &'a str {
        stem.get("rollout-".len() + 20..).unwrap_or(stem)
    }

    fn remote_script(&self, chat: &Chat, id: Option<&str>) -> Option<String> {
        Some(locate::codex_script(chat, id))
    }

    fn remote_guesses(&self) -> bool {
        true
    }

    fn has_context(&self) -> bool {
        true
    }

    fn context_in(&self, tail: &[u8]) -> Option<Context> {
        context::codex_context(tail)
    }

    fn usage(&self, raw: &[u8]) -> Vec<Message> {
        usage::codex(raw)
    }

    fn modes(&self) -> &'static [&'static str] {
        &["Build", "Plan"]
    }

    fn mode_shown(&self, rows: &[&str]) -> Option<String> {
        mode::codex_shown(rows)
    }

    fn model_switch(&self) -> ModelSwitch {
        ModelSwitch::OwnMenu
    }

    fn model_menu(&self, rows: &[(String, Styled)]) -> Option<Menu> {
        model_menu::codex(rows)
    }

    fn model_on_screen<'a>(&self, rows: &[&'a str]) -> Option<&'a str> {
        model_menu::codex_now(rows)
    }

    fn plan_menu(&self) -> Option<(&'static str, &'static str, &'static str)> {
        Some(crate::herdr::PLAN_MENUS[1])
    }

    fn builtins(&self) -> &'static [(&'static str, &'static str)] {
        commands::CODEX
    }

    fn disk_commands(&self, _cwd: &Path, homes: &Homes, _config: &Path, found: &mut Vec<Command>) {
        commands::codex_disk(homes, found);
    }
}

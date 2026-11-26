//! The Cursor CLI (`cursor-agent`). Its chats are `store.db` databases
//! under `~/.cursor/chats/<folder>/<id>/`, read by what changed since. Its
//! live word comes from the hooks in `~/.cursor/hooks.json` (`live`), which
//! it shares with Cursor's IDE.

mod commands;
mod db;
mod find;
pub(crate) mod live;
mod parse;
mod screen;
pub(crate) mod stats;

use std::{collections::HashSet, path::Path};

use super::{DbChat, Integration, ModelSwitch, Parser, Store};
use crate::{
    ai_work::{
        AgentKind, Chat,
        context::Context,
        locate::{self, Found, Homes, Span},
        usage::Message,
    },
    chat::{commands::Command, live as hub, parse::Reader},
    stage_models::Offer,
};

pub struct Cursor;

impl Integration for Cursor {
    fn kind(&self) -> AgentKind {
        AgentKind::Cursor
    }

    fn label(&self) -> &'static str {
        "Cursor"
    }

    fn store(&self) -> Option<Store> {
        Some(Store::Database)
    }

    fn parser(&self) -> Box<dyn Parser> {
        Box::new(parse::Cursor)
    }

    fn db(&self) -> Option<Box<dyn DbChat>> {
        Some(Box::<db::CursorDb>::default())
    }

    fn guesses(&self) -> bool {
        true
    }

    fn find(
        &self,
        homes: &Homes,
        _chat: &Chat,
        id: Option<&str>,
        _claimed: &HashSet<String>,
        _span: Span,
    ) -> Result<Option<Found>, String> {
        Ok(id.and_then(|id| find::cursor(homes, id)))
    }

    /// One without an id is found by folder and time in a pane only.
    fn find_pane(
        &self,
        homes: &Homes,
        chat: &Chat,
        claimed: &HashSet<String>,
        span: Span,
    ) -> Result<Option<Found>, String> {
        match chat.id {
            None => Ok(find::cursor_by_time(homes, chat, claimed, span)),
            Some(_) => locate::find_in(homes, chat, claimed, span),
        }
    }

    fn remote_script(&self, _chat: &Chat, id: Option<&str>) -> Option<String> {
        id.map(find::cursor_script)
    }

    fn staged_store(&self) -> bool {
        true
    }

    fn model_in_chat(&self, _reader: &Reader) -> Option<String> {
        stats::cursor_now()
    }

    fn model_now(&self, _chat: Option<&Path>) -> Option<String> {
        stats::cursor_now()
    }

    fn has_context(&self) -> bool {
        true
    }

    fn context(&self, path: &Path) -> Option<Context> {
        stats::cursor_context(path)
    }

    fn usage(&self, raw: &[u8]) -> Vec<Message> {
        stats::cursor(raw)
    }

    fn modes(&self) -> &'static [&'static str] {
        &["Agent", "Plan", "Debug", "Ask"]
    }

    fn mode_shown(&self, rows: &[&str]) -> Option<String> {
        screen::cursor_shown(rows)
    }

    fn model_switch(&self) -> ModelSwitch {
        ModelSwitch::Typed {
            cli: "cursor-agent",
        }
    }

    fn offers(&self, cursor: &[Offer]) -> Vec<Offer> {
        cursor.to_vec()
    }

    fn models_read(&self) -> bool {
        true
    }

    fn plan_menu(&self) -> Option<(&'static str, &'static str, &'static str)> {
        Some(screen::PLAN_MENU)
    }

    fn builtins(&self) -> &'static [(&'static str, &'static str)] {
        commands::CURSOR
    }

    fn disk_commands(&self, cwd: &Path, homes: &Homes, _config: &Path, found: &mut Vec<Command>) {
        commands::cursor_disk(cwd, homes, found);
    }

    fn hook_command(&self) -> Option<&'static str> {
        Some(live::HOOK_COMMAND)
    }

    fn run_hook(&self) {
        live::run_hook();
    }

    /// Cursor's CLI runs the hooks Cursor's IDE does.
    fn install_live(&self, home: &Path) -> Vec<Result<Option<hub::Changed>, String>> {
        if !home.join(".cursor").is_dir() {
            return Vec::new();
        }
        vec![live::install_hooks(home)]
    }
}

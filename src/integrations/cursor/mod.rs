//! The Cursor CLI (`cursor-agent`). Its chats are `store.db` databases
//! under `~/.cursor/chats/<folder>/<id>/`, read by what changed since. Its
//! live word comes from the hooks in `~/.cursor/hooks.json` (`live`), which
//! it shares with Cursor's IDE.

mod commands;
mod db;
mod find;
pub(crate) mod launch;
pub(crate) mod live;
mod parse;
mod screen;
pub(crate) mod stats;

use std::{collections::HashSet, path::Path};

use super::{
    DbChat, Integration, ModelSwitch, Parser, PickedFor, SidePanel, Store, common::install::Changed,
};
use crate::{
    ai_work::{
        AgentKind, Chat,
        context::Context,
        locate::{self, Found, Homes, Span},
        usage::Message,
    },
    chat::{commands::Command, parse::Reader},
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
        ModelSwitch::Typed
    }

    fn offers(&self, listed: &[Offer]) -> Vec<Offer> {
        listed.to_vec()
    }

    fn models_read(&self) -> bool {
        true
    }

    fn plan_menu(&self) -> Option<(&'static str, &'static str, &'static str)> {
        Some(screen::PLAN_MENU)
    }

    fn side_panel(&self) -> Option<SidePanel> {
        Some(screen::SIDE_PANEL)
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
    fn install_live(&self, home: &Path) -> Vec<Result<Option<Changed>, String>> {
        if !home.join(".cursor").is_dir() {
            return Vec::new();
        }
        vec![live::install_hooks(home)]
    }

    fn command(&self) -> Option<&'static str> {
        Some("cursor-agent")
    }

    /// Each feature's folder is new to it (checked on 2026-11-04).
    fn asks_trust(&self) -> bool {
        true
    }

    fn adds_dirs(&self) -> bool {
        true
    }

    /// It tells an agent a root's git branch and status only for a root
    /// that is a git repository, and a root's own search leaves out roots
    /// nested in it.
    fn adds_roots(&self) -> bool {
        true
    }

    fn takes_model(&self) -> bool {
        true
    }

    /// Whether its chat keeps its model couldn't be told from its chat.
    fn retells_model(&self) -> bool {
        true
    }

    fn resumes(&self) -> bool {
        true
    }

    fn new_chat(&self, base: &str, id_file: &str) -> Option<String> {
        Some(launch::new_chat(base, id_file))
    }

    fn launcher_script(&self, argv: &[&str]) -> Option<usize> {
        launch::launcher_script(argv)
    }

    fn runs_as(&self, name: &str, command: &str) -> bool {
        launch::runs_as(name, command)
    }

    fn run_model(&self, homes: &Homes, id: &str) -> Option<String> {
        launch::run_model(homes, id)
    }

    fn list_models(&self, base: &str) -> Result<Vec<Offer>, String> {
        launch::list_models(base)
    }

    fn reading_hint(&self, failed: bool) -> &'static str {
        match failed {
            false => "Reading Cursor's models…",
            true => "Couldn't read Cursor's models: type a model's id…",
        }
    }

    fn model_hint(&self, of: PickedFor) -> Option<&'static str> {
        // Tried on 2026.10.22: its first message saves `--model` as the
        // default of every chat of it, as its own `/model` does.
        Some(match of {
            PickedFor::Stage => "Model for this stage's next start; Cursor keeps it as its default…",
            PickedFor::Start | PickedFor::Chat => {
                "Model for this agent; Cursor keeps it as its default…"
            }
        })
    }
}

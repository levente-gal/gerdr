//! The Cursor CLI.

use std::{collections::HashSet, path::Path};

use super::{DbChat, Integration, ModelSwitch, Parser, Store};
use crate::{
    ai_work::{
        AgentKind, Chat, context,
        context::Context,
        locate::{self, Found, Homes, Span},
        usage::{self, Message},
    },
    chat::{
        commands,
        commands::Command,
        live, mode,
        parse::{self, Reader},
        tail,
    },
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
        Box::new(parse::cursor::Cursor)
    }

    fn db(&self) -> Option<Box<dyn DbChat>> {
        Some(Box::<tail::db::CursorDb>::default())
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
        Ok(id.and_then(|id| locate::cursor(homes, id)))
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
            None => Ok(locate::cursor_by_time(homes, chat, claimed, span)),
            Some(_) => locate::find_in(homes, chat, claimed, span),
        }
    }

    fn remote_script(&self, _chat: &Chat, id: Option<&str>) -> Option<String> {
        id.map(locate::cursor_script)
    }

    fn staged_store(&self) -> bool {
        true
    }

    fn model_in_chat(&self, _reader: &Reader) -> Option<String> {
        crate::chat::model::cursor_now()
    }

    fn has_context(&self) -> bool {
        true
    }

    fn context(&self, path: &Path) -> Option<Context> {
        context::cursor_context(path)
    }

    fn usage(&self, raw: &[u8]) -> Vec<Message> {
        usage::cursor(raw)
    }

    fn modes(&self) -> &'static [&'static str] {
        &["Agent", "Plan", "Debug", "Ask"]
    }

    fn mode_shown(&self, rows: &[&str]) -> Option<String> {
        mode::cursor_shown(rows)
    }

    fn model_switch(&self) -> ModelSwitch {
        ModelSwitch::Typed {
            cli: "cursor-agent",
        }
    }

    fn offers(&self, cursor: &[Offer]) -> Vec<Offer> {
        cursor.to_vec()
    }

    fn plan_menu(&self) -> Option<(&'static str, &'static str, &'static str)> {
        Some(crate::herdr::PLAN_MENUS[0])
    }

    fn builtins(&self) -> &'static [(&'static str, &'static str)] {
        commands::CURSOR
    }

    fn disk_commands(&self, cwd: &Path, homes: &Homes, _config: &Path, found: &mut Vec<Command>) {
        commands::cursor_disk(cwd, homes, found);
    }

    fn hook_command(&self) -> Option<&'static str> {
        Some(crate::cursor_agents::HOOK_COMMAND)
    }

    fn run_hook(&self) {
        crate::cursor_agents::run_hook();
    }

    /// Cursor's CLI runs the hooks Cursor's IDE does.
    fn install_live(&self, home: &Path) -> Vec<Result<Option<live::Changed>, String>> {
        if !home.join(".cursor").is_dir() {
            return Vec::new();
        }
        vec![
            live::hook_command(crate::cursor_agents::HOOK_COMMAND).and_then(|command| {
                live::install(&home.join(".cursor/hooks.json"), |text| {
                    crate::cursor_agents::merge_hooks(text, &command)
                })
            }),
        ]
    }
}

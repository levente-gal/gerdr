//! OpenCode.

use std::{collections::HashSet, path::Path};

use super::{DbChat, Integration, ModelSwitch, Parser, Store};
use crate::{
    ai_work::{
        AgentKind, Chat,
        locate::{self, Found, Homes, Span},
    },
    chat::{commands, commands::Command, live, menu::Menu, mode, model_menu, parse, tail},
    scrollback::Styled,
};

pub struct OpenCode;

impl Integration for OpenCode {
    fn kind(&self) -> AgentKind {
        AgentKind::OpenCode
    }

    fn label(&self) -> &'static str {
        "OpenCode"
    }

    fn store(&self) -> Option<Store> {
        Some(Store::Database)
    }

    fn parser(&self) -> Box<dyn Parser> {
        Box::new(parse::opencode::OpenCode)
    }

    fn db(&self) -> Option<Box<dyn DbChat>> {
        Some(Box::<tail::db::OpenCodeDb>::default())
    }

    fn guesses(&self) -> bool {
        true
    }

    /// By `chat`'s id, or by folder and time.
    fn find(
        &self,
        homes: &Homes,
        chat: &Chat,
        _id: Option<&str>,
        claimed: &HashSet<String>,
        span: Span,
    ) -> Result<Option<Found>, String> {
        locate::opencode(&homes.opencode, chat, claimed, span)
    }

    fn modes(&self) -> &'static [&'static str] {
        &["Build", "Plan"]
    }

    fn mode_shown(&self, rows: &[&str]) -> Option<String> {
        mode::opencode_shown(rows)
    }

    fn model_switch(&self) -> ModelSwitch {
        ModelSwitch::OwnMenu
    }

    fn model_menu(&self, rows: &[(String, Styled)]) -> Option<Menu> {
        model_menu::opencode(rows)
    }

    fn model_on_screen<'a>(&self, rows: &[&'a str]) -> Option<&'a str> {
        model_menu::opencode_now(rows)
    }

    fn asks(&self) -> bool {
        true
    }

    fn builtins(&self) -> &'static [(&'static str, &'static str)] {
        commands::OPENCODE
    }

    fn disk_commands(&self, cwd: &Path, _homes: &Homes, config: &Path, found: &mut Vec<Command>) {
        commands::opencode_disk(cwd, config, found);
    }

    fn install_live(&self, home: &Path) -> Vec<Result<Option<live::Changed>, String>> {
        let config = home.join(".config/opencode");
        if !config.is_dir() {
            return Vec::new();
        }
        (live::opencode::files(&config).into_iter())
            .map(|(path, merge)| live::install(&path, merge))
            .collect()
    }
}

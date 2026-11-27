//! OpenCode. Its chats are sessions in its database,
//! `~/.local/share/opencode/opencode.db`, read by what changed. Its live
//! word comes from a plugin in its TUI (`~/.config/opencode/gerdr-live/`)
//! that tells gerdr's socket itself.

mod commands;
mod db;
mod find;
mod live;
pub(crate) mod parse;
mod screen;

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use super::{
    DbChat, Integration, ModelSwitch, Parser, SidePanel, Store,
    common::install::{self, Changed},
};
use crate::{
    ai_work::{
        AgentKind, Chat,
        locate::{Found, Homes, Span},
    },
    chat::{commands::Command, menu::Menu},
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

    fn command(&self) -> Option<&'static str> {
        Some("opencode")
    }

    fn store(&self) -> Option<Store> {
        Some(Store::Database)
    }

    fn parser(&self) -> Box<dyn Parser> {
        Box::new(parse::OpenCode)
    }

    fn db(&self) -> Option<Box<dyn DbChat>> {
        Some(Box::<db::OpenCodeDb>::default())
    }

    fn guesses(&self) -> bool {
        true
    }

    fn backfill_by_time(&self) -> bool {
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
        find::find(&homes.opencode, chat, claimed, span)
    }

    fn modes(&self) -> &'static [&'static str] {
        &["Build", "Plan"]
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

    fn effort_switch(&self) -> ModelSwitch {
        ModelSwitch::OwnMenu
    }

    fn effort_line(&self) -> Option<(&'static str, bool)> {
        Some(("/variants", false))
    }

    fn effort_on_screen<'a>(&self, rows: &[&'a str]) -> Option<&'a str> {
        screen::effort_now(rows)
    }

    fn side_panel(&self) -> Option<SidePanel> {
        Some(screen::SIDE_PANEL)
    }

    fn builtins(&self) -> &'static [(&'static str, &'static str)] {
        commands::BUILTINS
    }

    fn disk_commands(&self, cwd: &Path, _homes: &Homes, config: &Path, found: &mut Vec<Command>) {
        commands::on_disk(cwd, config, found);
    }

    fn skills_agent(&self) -> Option<&'static str> {
        Some("opencode")
    }

    fn skill_dirs(&self, cwd: &Path, homes: &Homes, config: &Path) -> Vec<PathBuf> {
        let mut dirs = vec![
            cwd.join(".opencode/skills"),
            cwd.join(".claude/skills"),
            cwd.join(".agents/skills"),
            config.join("opencode/skills"),
        ];
        if let Some(home) = homes.cursor.parent() {
            dirs.extend([home.join(".claude/skills"), home.join(".agents/skills")]);
        }
        dirs
    }

    fn install_live(&self, home: &Path) -> Vec<Result<Option<Changed>, String>> {
        let config = home.join(".config/opencode");
        if !config.is_dir() {
            return Vec::new();
        }
        (live::files(&config).into_iter())
            .map(|(path, merge)| install::install(&path, merge))
            .collect()
    }
}

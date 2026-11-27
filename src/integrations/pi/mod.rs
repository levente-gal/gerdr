//! pi (`pi`). Its chats are session files of JSON lines under
//! `~/.pi/agent/sessions/--<folder>--/`, each entry a node of a tree. Its
//! live word comes from gerdr's extension in pi's extensions folder.

mod commands;
mod find;
mod live;
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
    chat::{commands::Command, menu::Menu, source},
    integrations::common::install::Changed,
    scrollback::Styled,
};

pub struct Pi;

impl Integration for Pi {
    fn kind(&self) -> AgentKind {
        AgentKind::Pi
    }

    fn label(&self) -> &'static str {
        "pi"
    }

    fn command(&self) -> Option<&'static str> {
        Some("pi")
    }

    fn store(&self) -> Option<Store> {
        Some(Store::Lines)
    }

    fn parser(&self) -> Box<dyn Parser> {
        Box::<parse::Pi>::default()
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
            Some(id) => find::by_id(&homes.pi, id),
            None => find::by_time(&homes.pi, chat, claimed, span),
        })
    }

    fn transcript_at(&self, path: &str, homes: &Homes) -> Option<PathBuf> {
        source::transcript(path, std::slice::from_ref(&homes.pi), |_| true)
    }

    fn transcripts_under(&self, homes: &Homes) -> Vec<PathBuf> {
        vec![homes.pi.clone()]
    }

    /// A pi session is `<time>_<id>`.
    fn id_in_path<'a>(&self, stem: &'a str, _path: &'a Path) -> &'a str {
        stem.split_once('_').map_or(stem, |(_, id)| id)
    }

    fn has_context(&self) -> bool {
        true
    }

    fn context_in(&self, tail: &[u8]) -> Option<Context> {
        stats::context(tail)
    }

    fn usage(&self, raw: &[u8]) -> Vec<Message> {
        stats::usage(raw)
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
        Some(("/thinking", false))
    }

    fn effort_on_screen<'a>(&self, rows: &[&'a str]) -> Option<&'a str> {
        screen::effort_now(rows)
    }

    fn builtins(&self) -> &'static [(&'static str, &'static str)] {
        commands::BUILTINS
    }

    fn disk_commands(&self, cwd: &Path, homes: &Homes, _config: &Path, found: &mut Vec<Command>) {
        commands::on_disk(cwd, homes, found);
    }

    fn skill_dirs(&self, cwd: &Path, homes: &Homes, _config: &Path) -> Vec<PathBuf> {
        let agent = homes.pi.parent().unwrap_or(Path::new("/"));
        let mut dirs = vec![cwd.join(".pi/skills"), cwd.join(".agents/skills")];
        dirs.push(agent.join("skills"));
        dirs.extend(
            homes
                .cursor
                .parent()
                .map(|home| home.join(".agents/skills")),
        );
        dirs
    }

    fn install_live(&self, home: &Path) -> Vec<Result<Option<Changed>, String>> {
        live::install(home)
    }
}

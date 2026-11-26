//! pi.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use super::{Integration, ModelSwitch, Parser, Store};
use crate::{
    ai_work::{
        AgentKind, Chat, context,
        context::Context,
        locate::{self, Found, Homes, Span},
        usage::{self, Message},
    },
    chat::{commands, commands::Command, live, menu::Menu, model_menu, parse, source},
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

    fn store(&self) -> Option<Store> {
        Some(Store::Lines)
    }

    fn parser(&self) -> Box<dyn Parser> {
        Box::<parse::pi::Pi>::default()
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
            Some(id) => locate::pi_by_id(&homes.pi, id),
            None => locate::pi_by_time(&homes.pi, chat, claimed, span),
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
        context::pi_context(tail)
    }

    fn usage(&self, raw: &[u8]) -> Vec<Message> {
        usage::pi(raw)
    }

    fn model_switch(&self) -> ModelSwitch {
        ModelSwitch::OwnMenu
    }

    fn model_menu(&self, rows: &[(String, Styled)]) -> Option<Menu> {
        model_menu::pi(rows)
    }

    fn model_on_screen<'a>(&self, rows: &[&'a str]) -> Option<&'a str> {
        model_menu::pi_now(rows)
    }

    fn builtins(&self) -> &'static [(&'static str, &'static str)] {
        commands::PI
    }

    fn disk_commands(&self, cwd: &Path, homes: &Homes, _config: &Path, found: &mut Vec<Command>) {
        commands::pi_disk(cwd, homes, found);
    }

    /// pi loads every extension in its folder: gerdr's goes beside herdr's.
    fn install_live(&self, home: &Path) -> Vec<Result<Option<live::Changed>, String>> {
        if !home.join(".pi/agent").is_dir() {
            return Vec::new();
        }
        let path = home.join(".pi/agent/extensions/gerdr-live.ts");
        vec![live::install(&path, |_| {
            Ok(live::serve::PI_EXTENSION.to_owned())
        })]
    }
}

//! Reviewing a chat's plan card in a file's tab (`DocFile::ChatPlan`), as
//! a feature's plan is: its markdown beside its page, line comments, Send
//! back. The plan is never edited there: only commented on.
//!
//! Its text is the card's, held by the review as it opens, so Claude,
//! Cursor and Codex go one way (Codex writes no file; Claude rewrites its
//! own in place), and the comments stay on the version they were made on: a
//! plan written again is a new card with a review of its own. Its comments
//! are kept in `<state dir>/chat-plans/<key>-comments.json`; the files there
//! untouched for `KEEP` go as the next review opens (`tidy`).
//!
//! Sending it back: while the agent waits on that plan (Claude's
//! `ExitPlanMode`), the ask is denied with the comments as why, so Claude
//! stays in plan mode and writes it again; else the comments go in a file
//! (`<key>-back-<k>.md`) and one line to the agent names it, as a line
//! with newlines would be sent at its first.

use std::{
    io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use crate::{
    Gerdr,
    chat::{Item, PaneChat},
    herdr::AgentStatus,
    prompts,
};

/// How long a snapshot or its comments are kept untouched.
const KEEP: Duration = Duration::from_secs(30 * 24 * 3600);

/// The key of plan item `item_id`'s review: stable between builds, as its
/// files are named by it.
pub fn key(item_id: &str) -> u64 {
    crate::inbox_sources::fnv1a(item_id.as_bytes())
}

/// Where chat plans' snapshots and comments are kept.
pub fn dir() -> PathBuf {
    crate::state_dir().join("chat-plans")
}

/// Where the review comments on the plan of `key` are kept.
pub fn comments_path(key: u64) -> PathBuf {
    dir().join(format!("{key:016x}-comments.json"))
}

/// The plan of `key` comes under review: the old files of others go.
pub fn tidy(key: u64) {
    prune(&dir(), key, SystemTime::now());
}

/// Delete the files of `dir` untouched for `KEEP` by `now`, but those of
/// the plan of `key`, under review again.
fn prune(dir: &Path, key: u64, now: SystemTime) {
    let own = format!("{key:016x}");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(&own) {
            continue;
        }
        let old = (entry.metadata().and_then(|m| m.modified()))
            .is_ok_and(|at| now.duration_since(at).is_ok_and(|age| age > KEEP));
        if old && let Err(err) = std::fs::remove_file(entry.path()) {
            tracing::warn!("pruning {}: {err}", entry.path().display());
        }
    }
}

/// Write `comments` (Markdown) sent back on the plan of `key` to a file of
/// their own, next to its comments. Where it went.
pub fn write_back(key: u64, comments: &str) -> io::Result<PathBuf> {
    write_back_in(&dir(), key, comments)
}

fn write_back_in(dir: &Path, key: u64, comments: &str) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = (1..)
        .map(|k| dir.join(format!("{key:016x}-back-{k}.md")))
        .find(|path| !path.exists())
        .expect("a free name");
    std::fs::write(&path, comments)?;
    Ok(path)
}

/// The one line telling the agent its plan's comments are in `file`.
pub fn back_line(file: &Path) -> Result<String, String> {
    let mut vars = prompts::render::Vars::new();
    vars.insert("comments.path".into(), file.display().to_string());
    (prompts::render_builtin("chat-plan-back", &vars))
        .map(|line| line.trim().to_owned())
        .map_err(|e| e.to_string())
}

/// The plan of item `item_id` in `chat`, if it has one.
fn plan_text<'a>(chat: &'a PaneChat, item_id: &str) -> Option<&'a str> {
    chat.items.iter().find_map(|item| match item {
        Item::Plan { id, text, .. } if id == item_id => Some(text.as_str()),
        _ => None,
    })
}

/// Whether `chat`'s agent waits on the plan of item `item_id`: a plan ask
/// shows, and that plan is the chat's last.
fn waits_on(chat: &PaneChat, item_id: &str) -> bool {
    let last = chat.items.iter().rev().find_map(|item| match item {
        Item::Plan { id, .. } => Some(id.as_str()),
        _ => None,
    });
    (chat.live.ask.as_ref()).is_some_and(|ask| ask.plan) && last == Some(item_id)
}

impl Gerdr {
    /// The chat of pane `pane_id`, among the panes and those whose window
    /// is still to come.
    fn plan_chat(&self, pane_id: &str) -> Option<&PaneChat> {
        let tabs = self.workspaces.iter().flat_map(|ws| &ws.tabs);
        tabs.flat_map(|tab| {
            let panes = (tab.panes.iter())
                .filter_map(|p| Some((p.herdr.as_ref()?.pane_id.as_str(), p.chat.as_deref()?)));
            let expected = (tab.expected.iter())
                .filter_map(|e| Some((e.herdr.as_ref()?.pane_id.as_str(), e.chat.as_deref()?)));
            panes.chain(expected)
        })
        .find_map(|(id, chat)| (id == pane_id).then_some(chat))
    }

    /// The text of plan item `item_id` in pane `pane_id`'s chat, if it
    /// still has it.
    pub(crate) fn chat_plan_text(&self, pane_id: &str, item_id: &str) -> Option<String> {
        plan_text(self.plan_chat(pane_id)?, item_id).map(str::to_owned)
    }

    /// Whether pane `pane_id`'s agent waits on the user's answer to the
    /// plan of item `item_id`.
    pub(crate) fn plan_ask_waits(&self, pane_id: &str, item_id: &str) -> bool {
        self.plan_chat(pane_id)
            .is_some_and(|chat| waits_on(chat, item_id))
    }

    /// Allow the plan pane `pane_id`'s agent waits on, as its card's Allow.
    pub(crate) fn allow_plan(&mut self, pane_id: &str) {
        let ask = self
            .plan_chat(pane_id)
            .and_then(|chat| chat.live.ask.clone());
        if let Some(ask) = ask.filter(|ask| ask.plan) {
            self.live_answer(&ask.ask, crate::chat::live::AskReply::Allow);
            if let Some(chat) = super::view::chat_mut(&mut self.workspaces, pane_id) {
                chat.live.ask = None;
            }
        }
    }

    /// Send `comments` (Markdown) on plan item `item_id` back to pane
    /// `pane_id`'s agent: as why its plan ask is denied while it waits on
    /// that plan, else in a file a line to it names. Why it couldn't go.
    pub(crate) fn send_plan_back(
        &mut self,
        pane_id: &str,
        item_id: &str,
        comments: &str,
    ) -> Result<(), String> {
        if self.plan_ask_waits(pane_id, item_id) {
            let ask = self
                .plan_chat(pane_id)
                .and_then(|chat| chat.live.ask.clone());
            // Its hook still holding on: else the deny would go nowhere.
            let held =
                |ask: &crate::chat::live::PendingAsk| self.chats.live.asks.contains_key(&ask.ask);
            if let Some(ask) = ask.filter(held) {
                self.live_deny(&ask.ask, comments.to_owned());
                if let Some(chat) = super::view::chat_mut(&mut self.workspaces, pane_id) {
                    chat.live.ask = None;
                }
                return Ok(());
            }
        }
        // Its own menu would take the line's Enter as a choice: Claude's
        // "Ready to code?" given back to its screen would go ahead. Cursor's
        // and Codex's plan menus, shown idle, the line leaves itself.
        if (self.chat_agent(pane_id)).is_some_and(|a| a.agent_status == AgentStatus::Blocked) {
            return Err("its agent waits on its own menu: answer it there first".into());
        }
        let file = write_back(key(item_id), comments)
            .map_err(|err| format!("couldn't write the comments: {err}"))?;
        let line = back_line(&file)?;
        self.send_line(pane_id, &line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_stable_and_names_the_files() {
        assert_eq!(key("toolu_1"), key("toolu_1"));
        assert_ne!(key("toolu_1"), key("toolu_2"));
        assert!(comments_path(1).ends_with("chat-plans/0000000000000001-comments.json"));
    }

    #[test]
    fn old_files_are_pruned_and_a_back_file_takes_the_next_name() {
        let dir = std::env::temp_dir().join(format!("gerdr-chat-plans-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (old, own) = (
            dir.join("old.md"),
            dir.join(format!("{:016x}-comments.json", 7)),
        );
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&old, "x").unwrap();
        std::fs::write(&own, "{}").unwrap();
        let later = SystemTime::now() + KEEP + Duration::from_secs(60);
        prune(&dir, 7, later);
        assert!(!old.exists(), "pruned");
        assert!(own.exists(), "under review again");

        let one = write_back_in(&dir, 7, "- fix it").unwrap();
        let two = write_back_in(&dir, 7, "- and this").unwrap();
        assert!(one.ends_with("0000000000000007-back-1.md"));
        assert!(two.ends_with("0000000000000007-back-2.md"));
        assert_eq!(std::fs::read_to_string(two).unwrap(), "- and this");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_back_line_names_the_file_on_one_line() {
        let line = back_line(Path::new("/s/chat-plans/x-back-1.md")).unwrap();
        assert!(line.contains("/s/chat-plans/x-back-1.md"), "{line}");
        assert!(!line.contains('\n'), "{line}");
    }

    #[test]
    fn it_waits_on_the_last_plan_only_while_a_plan_ask_shows() {
        use crate::chat::live::PendingAsk;
        let plan = |id: &str| crate::chat::parse::plan_item(id.into(), None, "# P\n");
        let mut chat = *PaneChat::opened();
        chat.items = std::sync::Arc::new(vec![plan("a"), plan("b")]);
        assert!(!waits_on(&chat, "b"));
        chat.live.ask = Some(PendingAsk {
            ask: "q".into(),
            tool: "ExitPlanMode".into(),
            gist: String::new(),
            input: "# P\n".into(),
            questions: Vec::new(),
            plan: true,
            since: std::time::Instant::now(),
        });
        assert!(waits_on(&chat, "b"));
        assert!(!waits_on(&chat, "a"));
        assert_eq!(plan_text(&chat, "a"), Some("# P\n"));
        assert_eq!(plan_text(&chat, "z"), None);
    }
}

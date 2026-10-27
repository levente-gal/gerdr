//! The inbox's items, gathered from what gerdr already shows: the agent
//! cards, the workspaces' Git status and worktrees, the CI cards, the
//! issues read for the inbox and its own todos. Pure: everything comes in
//! as `Sources`, so tests build them by hand.

#![allow(dead_code)]

use std::{collections::HashMap, path::PathBuf, time::Instant};

use crate::{
    agents::AgentKey,
    chrome::{AgentView, WorkspaceView},
    ci::RepoCi,
    cursor_agents::CursorChat,
    herdr::{AgentInfo, AgentStatus},
    inbox::{InboxItem, PaneText},
    inbox_store::InboxFile,
    issues::IssuesState,
};

/// What the items are built from.
#[derive(Clone, Copy)]
pub struct Sources<'a> {
    /// Each workspace as the sidebar shows it, by index.
    pub workspaces: &'a [WorkspaceView],
    /// Each workspace's folder (`Workspace::dir`), by index.
    pub dirs: &'a [PathBuf],
    /// Each workspace's record id (`Workspace::record`), by index: what a
    /// todo is linked by.
    pub records: &'a [String],
    /// Each workspace's agent cards, in sidebar order (`agent_views`).
    pub agents: &'a [Vec<AgentView>],
    /// What herdr says of its agents (`Gerdr::agents`): their
    /// `state_change_seq`.
    pub herdr_agents: &'a [AgentInfo],
    /// Cursor's chats shown as cards (`Gerdr::cursor_chats`).
    pub cursor_chats: &'a [CursorChat],
    /// Since when each agent has been as it is (`Gerdr::agent_since`).
    pub agent_since: &'a HashMap<AgentKey, (AgentStatus, Instant)>,
    /// The CI of the project's repositories, by root (`Gerdr::ci`).
    pub ci: &'a HashMap<PathBuf, RepoCi>,
    pub issues: &'a IssuesState,
    /// The shown project's inbox file, once loaded.
    pub store: Option<&'a InboxFile>,
    /// Agent panes' last lines, by herdr pane id.
    pub pane_text: &'a HashMap<String, PaneText>,
    pub now: Instant,
    /// Now, in seconds since the epoch.
    pub now_secs: u64,
}

/// The agents blocked, done or at work, in every workspace.
pub fn agent_items(src: &Sources) -> Vec<InboxItem> {
    let _ = src;
    Vec::new()
}

/// The workspace folders and worktrees needing a look.
pub fn git_items(src: &Sources) -> Vec<InboxItem> {
    let _ = src;
    Vec::new()
}

/// The failed CI rows and the pull requests asking for review.
pub fn ci_items(src: &Sources) -> Vec<InboxItem> {
    let _ = src;
    Vec::new()
}

/// The GitHub and Linear issues assigned to the user.
pub fn issue_items(src: &Sources) -> Vec<InboxItem> {
    let _ = src;
    Vec::new()
}

/// gerdr's own todos not done yet.
pub fn todo_items(src: &Sources) -> Vec<InboxItem> {
    let _ = src;
    Vec::new()
}

/// Every item, each once.
pub fn collect(src: &Sources) -> Vec<InboxItem> {
    let mut items = agent_items(src);
    items.extend(git_items(src));
    items.extend(ci_items(src));
    items.extend(issue_items(src));
    items.extend(todo_items(src));
    items
}

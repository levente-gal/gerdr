//! Agents from any source, as cards, switcher items and notifications see
//! them: herdr's, running in its terminal panes, and Cursor's chats (see
//! `cursor_agents`).

use std::{
    collections::HashMap,
    fmt,
    time::{Duration, Instant},
};

use crate::herdr::AgentStatus;

/// Which agent: the herdr pane it runs in, or the Cursor chat it is. Or,
/// for a notification about the CI (see `ci_watch`), the GitHub page it
/// opens; for a test run (see `test_runs`), the herdr pane it runs in.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum AgentKey {
    Herdr(String),
    Cursor(String),
    Ci(String),
    Test(String),
}

/// What a Cursor key's text starts with; a herdr key is its bare pane id,
/// as notifications and the switcher's frecency have long named it.
const CURSOR_PREFIX: &str = "cursor:";
/// What a CI key's text starts with.
const CI_PREFIX: &str = "ci:";
/// What a test run's key's text starts with.
const TEST_PREFIX: &str = "test:";

impl fmt::Display for AgentKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentKey::Herdr(pane_id) => f.write_str(pane_id),
            AgentKey::Cursor(chat) => write!(f, "{CURSOR_PREFIX}{chat}"),
            AgentKey::Ci(link) => write!(f, "{CI_PREFIX}{link}"),
            AgentKey::Test(pane_id) => write!(f, "{TEST_PREFIX}{pane_id}"),
        }
    }
}

impl AgentKey {
    /// The key `text` names, as `Display` writes it.
    pub fn parse(text: &str) -> Self {
        if let Some(link) = text.strip_prefix(CI_PREFIX) {
            return AgentKey::Ci(link.to_owned());
        }
        if let Some(pane_id) = text.strip_prefix(TEST_PREFIX) {
            return AgentKey::Test(pane_id.to_owned());
        }
        match text.strip_prefix(CURSOR_PREFIX) {
            Some(chat) => AgentKey::Cursor(chat.to_owned()),
            None => AgentKey::Herdr(text.to_owned()),
        }
    }
}

/// An agent as notifications see it.
#[derive(Clone, Debug)]
pub struct Watched {
    pub key: AgentKey,
    pub status: AgentStatus,
    /// What a notification calls it.
    pub label: String,
    /// What a notification says under that: its workspace, what it is on.
    pub body: String,
    /// Whether the user can see it now.
    pub seen: bool,
    /// What a notification says it did once finished, if not "is done".
    pub finished: Option<&'static str>,
}

/// A notification to send: its key, summary and body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct News {
    pub key: AgentKey,
    pub summary: String,
    pub body: String,
}

/// What of `agents` is news, given how each was `before`: one that just
/// came to need input or finished where the user can't see it. A finished
/// agent may come back `idle` rather than `done`: both count. Only a change
/// is news; an agent seen for the first time (as all are when gerdr
/// starts) isn't.
///
/// One that stays blocked out of sight is reminded of every `reminder`,
/// counted from when it was last notified of or last in sight; when that
/// was is kept in `blocked_since`, which forgets those no longer blocked.
pub fn agent_news(
    before: &HashMap<AgentKey, AgentStatus>,
    agents: &[Watched],
    blocked_since: &mut HashMap<AgentKey, Instant>,
    now: Instant,
    reminder: Duration,
) -> Vec<News> {
    use AgentStatus::{Blocked, Done, Idle, Working};
    let mut news = Vec::new();
    for agent in agents {
        let was = before.get(&agent.key).copied();
        let what = match (was, agent.status) {
            (_, Blocked) => {
                let since = *blocked_since.entry(agent.key.clone()).or_insert(now);
                if agent.seen {
                    blocked_since.insert(agent.key.clone(), now);
                    continue;
                }
                let what = if was.is_some_and(|b| b != Blocked) {
                    "needs input"
                } else if now.duration_since(since) >= reminder {
                    "still needs input"
                } else {
                    continue;
                };
                blocked_since.insert(agent.key.clone(), now);
                what
            }
            (Some(Working), Idle | Done) if !agent.seen => agent.finished.unwrap_or("is done"),
            _ => continue,
        };
        news.push(News {
            key: agent.key.clone(),
            summary: format!("{} {what}", agent.label),
            body: agent.body.clone(),
        });
    }
    blocked_since.retain(|key, _| agents.iter().any(|a| a.key == *key && a.status == Blocked));
    news
}

#[cfg(test)]
mod tests {
    use super::*;

    const REMINDER: Duration = Duration::from_secs(60);

    fn watched(key: AgentKey, status: AgentStatus, seen: bool) -> Watched {
        Watched {
            key,
            status,
            label: "claude".into(),
            body: "gerdr".into(),
            seen,
            finished: None,
        }
    }

    fn herdr(id: &str) -> AgentKey {
        AgentKey::Herdr(id.into())
    }

    fn cursor(id: &str) -> AgentKey {
        AgentKey::Cursor(id.into())
    }

    #[test]
    fn keys_read_back_as_written() {
        for key in [
            herdr("p1"),
            cursor("6898cc4e-5cb2"),
            AgentKey::Test("w1:p2".into()),
        ] {
            assert_eq!(AgentKey::parse(&key.to_string()), key);
        }
        // herdr's stay bare pane ids, as saved frecency has them.
        assert_eq!(herdr("p1").to_string(), "p1");
        assert_eq!(cursor("c1").to_string(), "cursor:c1");
    }

    #[test]
    fn changes_out_of_sight_are_news() {
        let now = Instant::now();
        let mut blocked = HashMap::new();
        for key in [herdr("p1"), cursor("c1")] {
            let before = HashMap::from([(key.clone(), AgentStatus::Working)]);
            // Seen for the first time: no news.
            let first = [watched(key.clone(), AgentStatus::Done, false)];
            assert!(agent_news(&HashMap::new(), &first, &mut blocked, now, REMINDER).is_empty());
            // Done out of sight: news; in sight: none.
            let news = agent_news(&before, &first, &mut blocked, now, REMINDER);
            assert_eq!(news[0].summary, "claude is done");
            assert_eq!(news[0].key, key);
            let seen = [watched(key.clone(), AgentStatus::Idle, true)];
            assert!(agent_news(&before, &seen, &mut blocked, now, REMINDER).is_empty());
            // Blocked: needs input.
            let stuck = [watched(key.clone(), AgentStatus::Blocked, false)];
            let news = agent_news(&before, &stuck, &mut blocked, now, REMINDER);
            assert_eq!(news[0].summary, "claude needs input");
        }
    }

    #[test]
    fn one_blocked_out_of_sight_is_reminded() {
        let start = Instant::now();
        let key = cursor("c1");
        let before = HashMap::from([(key.clone(), AgentStatus::Blocked)]);
        let stuck = [watched(key.clone(), AgentStatus::Blocked, false)];
        let mut blocked = HashMap::from([(key.clone(), start)]);
        let soon = start + Duration::from_secs(10);
        assert!(agent_news(&before, &stuck, &mut blocked, soon, REMINDER).is_empty());
        let later = start + REMINDER;
        let news = agent_news(&before, &stuck, &mut blocked, later, REMINDER);
        assert_eq!(news[0].summary, "claude still needs input");
        assert_eq!(blocked[&key], later);
    }

    #[test]
    fn every_source_keeps_its_blocked_time() {
        let start = Instant::now();
        let (a, b) = (herdr("p1"), cursor("c1"));
        let before = HashMap::from([
            (a.clone(), AgentStatus::Blocked),
            (b.clone(), AgentStatus::Blocked),
        ]);
        let both = [
            watched(a.clone(), AgentStatus::Blocked, false),
            watched(b.clone(), AgentStatus::Blocked, false),
        ];
        let mut blocked = HashMap::from([(a.clone(), start), (b.clone(), start)]);
        let soon = start + Duration::from_secs(1);
        agent_news(&before, &both, &mut blocked, soon, REMINDER);
        assert_eq!(blocked[&a], start);
        assert_eq!(blocked[&b], start);
        // One no longer blocked goes.
        let one = [watched(a.clone(), AgentStatus::Working, false)];
        agent_news(&before, &one, &mut blocked, soon, REMINDER);
        assert!(blocked.is_empty());
    }

    #[test]
    fn a_finished_test_run_says_how_it_went() {
        let key = AgentKey::Test("w1:p2".into());
        let before = HashMap::from([(key.clone(), AgentStatus::Working)]);
        let mut done = watched(key, AgentStatus::Done, false);
        done.label = "cargo test".into();
        done.finished = Some("failed");
        let news = agent_news(
            &before,
            &[done],
            &mut HashMap::new(),
            Instant::now(),
            REMINDER,
        );
        assert_eq!(news[0].summary, "cargo test failed");
    }
}

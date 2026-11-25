//! gerdr's side of live word (see `super`): the socket, what each event
//! changes, the asks and their answers, and adding the hooks.

use std::{collections::HashMap, io::Read, os::unix::net::UnixStream, path::Path};

use smithay::reexports::calloop::{
    Interest, Mode, PostAction,
    generic::Generic,
    timer::{TimeoutAction, Timer},
};

use super::{
    ASK_WAIT, Answer, AskReply, Changed, HeldAsk, LiveEvent, PHONE_SIGHT, PendingAsk, SOURCE, What,
    antigravity, claude, opencode,
};
use crate::{
    Gerdr, ai_work::AgentKind, chat::view::chat_mut, herdr::AgentInfo, herdr::AgentSession,
    menu::MenuAction,
};

/// pi's extension, as gerdr writes it to `~/.pi/agent/extensions/`.
const PI_EXTENSION: &str = include_str!("../../../assets/pi/gerdr-live.ts");

/// How often a waiting ask looks whether its chat is still in sight.
const ASK_LOOK: std::time::Duration = std::time::Duration::from_secs(1);

impl Gerdr {
    /// Hear the agents of the herdr session at `herdr_socket` (see
    /// `super::socket_for`), till gerdr follows another session: its
    /// source goes with the session's.
    pub(crate) fn listen_live(&mut self, herdr_socket: &Path) {
        self.chats.live.sessions.clear();
        for (_, mut held) in self.chats.live.asks.drain() {
            let ask = String::new();
            super::answer(
                &mut held.stream,
                &Answer {
                    ask,
                    reply: AskReply::Pass,
                },
            );
        }
        let Some(path) = super::socket_for(herdr_socket) else {
            return tracing::warn!("agent live: no XDG_RUNTIME_DIR");
        };
        let listener = match super::bind(&path) {
            Ok(listener) => listener,
            Err(err) => return tracing::warn!("agent live, {}: {err}", path.display()),
        };
        let source = Generic::new(listener, Interest::READ, Mode::Level);
        let inserted = self
            .loop_handle
            .insert_source(source, |_, listener, state| {
                while let Ok((stream, _)) = listener.accept() {
                    state.live_connection(stream);
                }
                Ok(PostAction::Continue)
            });
        match inserted {
            Ok(token) => {
                self.session.sources.push(token);
                self.chats.live.socket = Some(path);
            }
            Err(err) => tracing::warn!("agent live: {err}"),
        }
    }

    /// Read connection `stream`'s lines as they come, till it closes.
    fn live_connection(&mut self, stream: UnixStream) {
        if let Err(err) = stream.set_nonblocking(true) {
            return tracing::debug!("agent live: {err}");
        }
        let conn = self.chats.live.next_conn;
        self.chats.live.next_conn += 1;
        let mut buf = Vec::new();
        let source = Generic::new(stream, Interest::READ, Mode::Level);
        let inserted = self
            .loop_handle
            .insert_source(source, move |_, stream, state| {
                let mut stream: &UnixStream = stream;
                let mut chunk = [0u8; 16 << 10];
                let closed = loop {
                    match stream.read(&mut chunk) {
                        Ok(0) => break true,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break false,
                        Err(_) => break true,
                    }
                };
                let (events, too_long) = super::take_lines(&mut buf);
                for event in events {
                    state.live_event(event, conn, stream.try_clone().ok());
                }
                if closed || too_long {
                    state.live_closed(conn);
                    return Ok(PostAction::Remove);
                }
                Ok(PostAction::Continue)
            });
        if let Err(err) = inserted {
            tracing::warn!("agent live: {err}");
        }
    }

    /// Take in what pane `event.pane`'s agent says, by connection `conn`
    /// (`stream`, to answer an ask on).
    pub(crate) fn live_event(
        &mut self,
        mut event: LiveEvent,
        conn: u64,
        stream: Option<UnixStream>,
    ) {
        let now = super::now_ms();
        if event.at_ms <= 0 || event.at_ms > now {
            event.at_ms = now;
        }
        let pane = event.pane.clone();
        match event.what {
            What::Session { kind, value } => {
                let session = AgentSession {
                    source: SOURCE.into(),
                    agent: event.agent,
                    kind,
                    value,
                };
                if self.chats.live.sessions.get(&pane) != Some(&session) {
                    self.chats.live.sessions.insert(pane.clone(), session);
                    self.chats_sync();
                }
            }
            What::Ask {
                ask,
                tool,
                gist,
                input,
            } => {
                let Some(mut stream) = stream else {
                    return;
                };
                // Nobody sees the chat, here or on a phone: the TUI asks, at once.
                if !self.ask_in_sight(&pane) {
                    let reply = AskReply::Pass;
                    return super::answer(&mut stream, &Answer { ask, reply });
                }
                let claimed: Vec<String> = (self.chats.live.asks.values())
                    .filter_map(|held| held.call.clone())
                    .collect();
                let asked = PendingAsk {
                    ask: ask.clone(),
                    tool,
                    gist,
                    input,
                    since: std::time::Instant::now(),
                };
                let mut call = None;
                if let Some(chat) = chat_mut(&mut self.workspaces, &pane) {
                    call =
                        super::asked_call(&chat.live.running, &asked.tool, &asked.gist, &claimed);
                    chat.live.ask = Some(asked.clone());
                }
                let held = HeldAsk {
                    pane: pane.clone(),
                    conn,
                    stream,
                    call,
                    asked,
                };
                self.chats.live.asks.insert(ask.clone(), held);
                // Looked at every `ASK_LOOK`: one that waited `ASK_WAIT`, or
                // whose chat a panel came over, goes back to the TUI.
                let since = std::time::Instant::now();
                let timer = Timer::from_duration(ASK_LOOK);
                let waited = self.loop_handle.insert_source(timer, move |_, _, state| {
                    let Some(pane) = state.chats.live.asks.get(&ask).map(|h| h.pane.clone()) else {
                        return TimeoutAction::Drop;
                    };
                    if since.elapsed() < ASK_WAIT && state.ask_in_sight(&pane) {
                        return TimeoutAction::ToDuration(ASK_LOOK);
                    }
                    state.live_answer(&ask, AskReply::Pass);
                    TimeoutAction::Drop
                });
                if let Err(err) = waited {
                    tracing::warn!("agent live: {err}");
                }
                // The sidebar and the tab say it waits too (X1).
                self.mark_agent_statuses();
                self.notify_agents();
            }
            What::AskGone { ask } => self.live_drop_ask(&ask),
            what => {
                let settled: Vec<String> = (self.chats.live.asks.iter())
                    .filter(|(_, held)| held.pane == pane && super::settled_by(held, &what))
                    .map(|(ask, _)| ask.clone())
                    .collect();
                for ask in settled {
                    self.live_answer(&ask, AskReply::Pass);
                }
                if let Some(chat) = chat_mut(&mut self.workspaces, &pane)
                    && chat.live.hear(&what, event.at_ms)
                {
                    chat.items = chat.live.timed(chat.items.clone());
                }
            }
        }
        // The transcript has news: read it now, not on the next poll.
        if self.chats.followed.contains_key(&pane)
            && let Some(tail) = self.chats.tail.as_ref()
        {
            tail.read(&pane);
        }
    }

    /// Answer ask `ask` with `reply`, if it still waits, and take its card
    /// away.
    pub(crate) fn live_answer(&mut self, ask: &str, reply: AskReply) {
        let Some(mut held) = self.chats.live.asks.remove(ask) else {
            return;
        };
        let ask = ask.to_owned();
        super::answer(
            &mut held.stream,
            &Answer {
                ask: ask.clone(),
                reply,
            },
        );
        self.live_clear_card(&held.pane, &ask);
    }

    /// Forget ask `ask`, answered elsewhere: its card goes.
    fn live_drop_ask(&mut self, ask: &str) {
        if let Some(held) = self.chats.live.asks.remove(ask) {
            self.live_clear_card(&held.pane, ask);
        }
        // OpenCode's asks don't wait on a connection: only their cards.
        for ws in &mut self.workspaces {
            for tab in &mut ws.tabs {
                for pane in &mut tab.panes {
                    if let Some(chat) = pane.chat.as_deref_mut()
                        && chat.live.ask.as_ref().is_some_and(|a| a.ask == ask)
                    {
                        chat.live.ask = None;
                    }
                }
            }
        }
    }

    fn live_clear_card(&mut self, pane: &str, ask: &str) {
        if let Some(chat) = chat_mut(&mut self.workspaces, pane)
            && chat.live.ask.as_ref().is_some_and(|a| a.ask == ask)
        {
            chat.live.ask = None;
        }
    }

    /// Connection `conn` closed: its asks wait no more.
    fn live_closed(&mut self, conn: u64) {
        let gone: Vec<String> = (self.chats.live.asks.iter())
            .filter(|(_, held)| held.conn == conn)
            .map(|(ask, _)| ask.clone())
            .collect();
        for ask in gone {
            self.live_drop_ask(&ask);
        }
    }

    /// The asks of chats no longer in sight go back to their TUIs (called
    /// as `chats_sync` ends).
    pub(crate) fn live_release_hidden(&mut self) {
        let hidden = hidden_asks(&self.chats.live.asks, |pane| self.ask_in_sight(pane));
        for ask in hidden {
            self.live_answer(&ask, AskReply::Pass);
        }
    }

    /// Whether an ask of agent pane `pane` can be answered on a card: its
    /// chat shows here (`chat_in_sight`), or a phone showed the pane lately.
    fn ask_in_sight(&self, pane: &str) -> bool {
        let phone =
            (self.chats.live.phone_seen.get(pane)).is_some_and(|at| at.elapsed() < PHONE_SIGHT);
        phone || self.chat_in_sight(pane)
    }

    /// The phone shows agent pane `pane` now: the ask waiting on it, if any,
    /// for its card. Asks wait on the phone's card while it does.
    pub(crate) fn phone_ask(&mut self, pane: &str) -> Option<PendingAsk> {
        let now = std::time::Instant::now();
        let seen = &mut self.chats.live.phone_seen;
        seen.retain(|_, at| now.duration_since(*at) < PHONE_SIGHT);
        seen.insert(pane.to_owned(), now);
        (self.chats.live.asks.values())
            .find(|held| held.pane == pane)
            .map(|held| held.asked.clone())
    }

    /// The phone answered ask `ask` of agent pane `pane` with `reply`:
    /// whether it still waited.
    pub(crate) fn phone_answer(&mut self, pane: &str, ask: &str, reply: AskReply) -> bool {
        let waits = (self.chats.live.asks.get(ask)).is_some_and(|held| held.pane == pane);
        if waits {
            self.live_answer(ask, reply);
        }
        waits
    }

    /// `agent` with the session heard live, over herdr's.
    pub(crate) fn with_live_session(&self, agent: &AgentInfo) -> AgentInfo {
        let mut agent = agent.clone();
        if let Some(heard) = live_session(&agent, self.chats.live.sessions.get(&agent.pane_id)) {
            agent.agent_session = Some(heard.clone());
        }
        agent
    }

    /// Add gerdr's hooks to the agents installed here, once a run, unless
    /// the user undid them: a toast says what changed, with Undo (D1).
    pub(crate) fn install_live_hooks(&mut self) {
        if std::mem::replace(&mut self.chats.live.installed, true) || !self.settings.agent_hooks {
            return;
        }
        let Some(home) = crate::home_dir() else {
            return;
        };
        let mut changed: Vec<Changed> = Vec::new();
        let mut failed: Vec<String> = Vec::new();
        let mut take = |result: Result<Option<Changed>, String>| match result {
            Ok(Some(c)) => changed.push(c),
            Ok(None) => {}
            Err(err) => failed.push(err),
        };
        if home.join(".claude").is_dir() {
            take(
                super::hook_command(claude::HOOK_COMMAND).and_then(|command| {
                    super::install(&home.join(".claude/settings.json"), |text| {
                        claude::merged(text, &command)
                    })
                }),
            );
        }
        // Cursor's CLI runs the hooks Cursor's IDE does.
        if home.join(".cursor").is_dir() {
            take(
                super::hook_command(crate::cursor_agents::HOOK_COMMAND).and_then(|command| {
                    super::install(&home.join(".cursor/hooks.json"), |text| {
                        crate::cursor_agents::merge_hooks(text, &command)
                    })
                }),
            );
        }
        let config = home.join(".config/opencode");
        if config.is_dir() {
            for (path, merge) in opencode::files(&config) {
                take(super::install(&path, merge));
            }
        }
        // pi loads every extension in its folder: gerdr's goes beside herdr's.
        if home.join(".pi/agent").is_dir() {
            let path = home.join(".pi/agent/extensions/gerdr-live.ts");
            take(super::install(&path, |_| Ok(PI_EXTENSION.to_owned())));
        }
        if home.join(".gemini/config").is_dir() {
            take(
                super::hook_command(antigravity::HOOK_COMMAND).and_then(|command| {
                    super::install(&home.join(".gemini/config/hooks.json"), |text| {
                        antigravity::merged(text, &command)
                    })
                }),
            );
        }
        for err in &failed {
            tracing::warn!("agent live: adding gerdr's hooks: {err}");
        }
        if let Some(err) = failed.first() {
            self.show_warning(format!(
                "Agent chats won't hear their agents at once: {err}"
            ));
        }
        if changed.is_empty() {
            return;
        }
        let names: Vec<String> = (changed.iter())
            .map(|c| crate::paths::tilde(&c.path, Some(&home)))
            .collect();
        self.toast_with(
            format!(
                "Agent chats now hear their agents at once: gerdr added its hooks to {}",
                names.join(", ")
            ),
            crate::toast::Level::Info,
            Some(("Undo".into(), MenuAction::UndoLiveHooks(changed))),
        );
    }

    /// Put back the files `install_live_hooks` changed, and don't add the
    /// hooks again: `[ui] agent_hooks = false` in the settings file (S3).
    pub(crate) fn undo_live_hooks(&mut self, changed: &[Changed]) {
        let failed: Vec<String> = changed
            .iter()
            .filter_map(|c| super::undo(c).err())
            .collect();
        self.settings.agent_hooks = false;
        if let Err(err) = crate::config::write_bool("ui", "agent_hooks", false) {
            tracing::warn!("agent live: [ui] agent_hooks: {err}");
            return self.show_error(format!("Couldn't save that gerdr adds no hooks: {err}"));
        }
        match failed.first() {
            Some(err) => self.show_error(format!("Couldn't take gerdr's hooks out: {err}")),
            None => self.show_success(
                "gerdr's hooks are out; [ui] agent_hooks = true adds them again".into(),
            ),
        }
    }
}

/// The asks of `asks` whose pane isn't `in_sight`: its chat flipped to its
/// screen, closed, in a tab out of sight or under a panel, and no phone on
/// it.
fn hidden_asks(asks: &HashMap<String, HeldAsk>, in_sight: impl Fn(&str) -> bool) -> Vec<String> {
    (asks.iter())
        .filter(|(_, held)| !in_sight(&held.pane))
        .map(|(ask, _)| ask.clone())
        .collect()
}

/// The session heard live for `agent`'s pane, `heard`, if it is that
/// agent's: it beats herdr's, which may be left from an earlier agent in the
/// pane, or (Antigravity's hook) a subagent's.
fn live_session<'a>(
    agent: &AgentInfo,
    heard: Option<&'a AgentSession>,
) -> Option<&'a AgentSession> {
    let kind = |name: &str| AgentKind::of(name);
    let running = kind(agent.agent.as_deref()?);
    heard.filter(|s| kind(&s.agent) == running)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pis_extension_names_the_socket_as_gerdr_does() {
        assert!(PI_EXTENSION.contains("gerdr-live-${hash}.sock"));
        assert!(PI_EXTENSION.contains("digest(\"hex\").slice(0, 16)"));
    }

    #[test]
    fn a_session_heard_beats_herdrs_when_it_is_the_running_agents() {
        let heard = |agent: &str| AgentSession {
            source: SOURCE.into(),
            agent: agent.into(),
            kind: "path".into(),
            value: "/t.jsonl".into(),
        };
        // herdr kept a Codex chat for the pane Claude runs in now.
        let stale = crate::chat::source::tests::agent("codex", "id", "x");
        let mut claude = stale.clone();
        claude.agent = Some("claude".into());
        assert_eq!(
            live_session(&claude, Some(&heard("claude"))),
            Some(&heard("claude"))
        );
        // herdr names one of Claude's: the heard one still wins (herdr's
        // Antigravity hook names subagents' too).
        let named = crate::chat::source::tests::agent("claude", "path", "/h.jsonl");
        assert_eq!(
            live_session(&named, Some(&heard("claude"))),
            Some(&heard("claude"))
        );
        // A session heard from another agent than the one running: not it.
        assert_eq!(live_session(&claude, Some(&heard("cursor"))), None);
        let mut none = claude.clone();
        none.agent_session = None;
        assert_eq!(
            live_session(&none, Some(&heard("claude"))),
            Some(&heard("claude"))
        );
    }

    #[test]
    fn asks_go_back_once_their_chat_is_out_of_sight() {
        let held = |pane: &str| HeldAsk {
            pane: pane.into(),
            conn: 0,
            stream: UnixStream::pair().unwrap().0,
            call: None,
            asked: PendingAsk {
                ask: "q".into(),
                tool: "Bash".into(),
                gist: "ls".into(),
                input: String::new(),
                since: std::time::Instant::now(),
            },
        };
        let asks = HashMap::from([
            ("a".to_owned(), held("w1:p1")),
            ("b".to_owned(), held("w1:p2")),
        ]);
        assert_eq!(hidden_asks(&asks, |pane| pane == "w1:p1"), ["b"]);
        assert_eq!(hidden_asks(&asks, |_| false).len(), 2);
    }
}

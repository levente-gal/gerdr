//! gerdr's side of live word (see `super`): the socket, what each event
//! changes, the asks and their answers, and adding the hooks.

use std::{io::Read, os::unix::net::UnixStream, path::Path};

use smithay::reexports::calloop::{
    Interest, Mode, PostAction,
    generic::Generic,
    timer::{TimeoutAction, Timer},
};

use super::{
    ASK_WAIT, Answer, AskReply, Changed, HeldAsk, LiveEvent, PendingAsk, SOURCE, What, claude,
    opencode,
};
use crate::{Gerdr, chat::view::chat_mut, herdr::AgentInfo, herdr::AgentSession, menu::MenuAction};

impl Gerdr {
    /// Hear the agents of the herdr session at `herdr_socket` (see
    /// `super::socket_for`), till gerdr follows another session: its
    /// source goes with the session's.
    pub(crate) fn listen_live(&mut self, herdr_socket: &Path) {
        self.chats.live.sessions.clear();
        for (_, mut held) in self.chats.live.asks.drain() {
            let ask = String::new();
            super::answer(&mut held.stream, &Answer { ask, reply: AskReply::Pass });
        }
        let Some(path) = super::socket_for(herdr_socket) else {
            return tracing::warn!("agent live: no XDG_RUNTIME_DIR");
        };
        let listener = match super::bind(&path) {
            Ok(listener) => listener,
            Err(err) => return tracing::warn!("agent live, {}: {err}", path.display()),
        };
        let source = Generic::new(listener, Interest::READ, Mode::Level);
        let inserted = self.loop_handle.insert_source(source, |_, listener, state| {
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
        let inserted = self.loop_handle.insert_source(source, move |_, stream, state| {
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
    pub(crate) fn live_event(&mut self, mut event: LiveEvent, conn: u64, stream: Option<UnixStream>) {
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
                // Nobody sees the chat: the TUI asks, at once.
                if !self.chats.followed.contains_key(&pane) {
                    let reply = AskReply::Pass;
                    return super::answer(&mut stream, &Answer { ask, reply });
                }
                if let Some(chat) = chat_mut(&mut self.workspaces, &pane) {
                    chat.live.ask = Some(PendingAsk {
                        ask: ask.clone(),
                        tool,
                        gist,
                        input,
                        since: std::time::Instant::now(),
                    });
                }
                let held = HeldAsk { pane: pane.clone(), conn, stream };
                self.chats.live.asks.insert(ask.clone(), held);
                let timer = Timer::from_duration(ASK_WAIT);
                let waited = self.loop_handle.insert_source(timer, move |_, _, state| {
                    state.live_answer(&ask, AskReply::Pass);
                    TimeoutAction::Drop
                });
                if let Err(err) = waited {
                    tracing::warn!("agent live: {err}");
                }
            }
            What::AskGone { ask } => self.live_drop_ask(&ask),
            what => {
                if let Some(chat) = chat_mut(&mut self.workspaces, &pane) {
                    chat.live.hear(&what, event.at_ms);
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
        super::answer(&mut held.stream, &Answer { ask: ask.clone(), reply });
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
        let hidden: Vec<String> = (self.chats.live.asks.iter())
            .filter(|(_, held)| !self.chats.followed.contains_key(&held.pane))
            .map(|(ask, _)| ask.clone())
            .collect();
        for ask in hidden {
            self.live_answer(&ask, AskReply::Pass);
        }
    }

    /// `agent` with the session heard live, when herdr names none.
    pub(crate) fn with_live_session(&self, agent: &AgentInfo) -> AgentInfo {
        let mut agent = agent.clone();
        if agent.agent_session.is_none() {
            agent.agent_session = self.chats.live.sessions.get(&agent.pane_id).cloned();
        }
        agent
    }

    /// Add gerdr's hooks to the agents installed here, once a run, unless
    /// the user undid them: a toast says what changed, with Undo (D1).
    pub(crate) fn install_live_hooks(&mut self) {
        if std::mem::replace(&mut self.chats.live.installed, true)
            || super::declined_marker().exists()
        {
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
            take(super::hook_command(claude::HOOK_COMMAND).and_then(|command| {
                super::install(&home.join(".claude/settings.json"), |text| {
                    claude::merged(text, &command)
                })
            }));
        }
        // Cursor's CLI runs the hooks Cursor's IDE does.
        if home.join(".cursor").is_dir() {
            take(super::hook_command(crate::cursor_agents::HOOK_COMMAND).and_then(|command| {
                super::install(&home.join(".cursor/hooks.json"), |text| {
                    crate::cursor_agents::merge_hooks(text, &command)
                })
            }));
        }
        let config = home.join(".config/opencode");
        if config.is_dir() {
            for (path, merge) in opencode::files(&config) {
                take(super::install(&path, merge));
            }
        }
        for err in &failed {
            tracing::warn!("agent live: adding gerdr's hooks: {err}");
        }
        if let Some(err) = failed.first() {
            self.show_warning(format!("Agent chats won't hear their agents at once: {err}"));
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
    /// hooks again.
    pub(crate) fn undo_live_hooks(&mut self, changed: &[Changed]) {
        let failed: Vec<String> = changed.iter().filter_map(|c| super::undo(c).err()).collect();
        let marker = super::declined_marker();
        if let Some(dir) = marker.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(err) = std::fs::write(&marker, b"") {
            tracing::warn!("agent live: {}: {err}", marker.display());
        }
        match failed.first() {
            Some(err) => self.show_error(format!("Couldn't take gerdr's hooks out: {err}")),
            None => self.show_notice("gerdr's hooks are out; it won't add them again".into()),
        }
    }
}

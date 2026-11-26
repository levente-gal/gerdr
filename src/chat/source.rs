//! Which chat a pane's agent holds, and where its transcript is.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use super::Source;
use crate::{
    ai_work::{
        AgentKind, Chat,
        locate::{self, Homes, Span},
    },
    feature::Stage,
    herdr::AgentInfo,
    integrations,
};

/// Said when the pane's chat can't be found.
pub const NOT_FOUND: &str = "Couldn't find this agent's chat. Its screen still works";
/// Said when herdr names a chat that isn't on disk yet: Claude writes it
/// with its first message.
pub const NOT_YET: &str = "No transcript of this chat yet. It shows once the agent writes one";

/// An agent's run in a pane, for finding its chat when herdr names none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    /// When it began, in seconds since the epoch.
    pub started: u64,
    /// Where it runs.
    pub cwd: PathBuf,
    /// Its process.
    pub pid: u32,
}

/// The kind of agent in a pane, as herdr names it.
pub fn kind(agent: &AgentInfo) -> Option<AgentKind> {
    let session = agent.agent_session.as_ref().map(|s| s.agent.as_str());
    (agent.agent.as_deref()).or(session).map(AgentKind::of)
}

/// Whether herdr names the chat of the agent in the pane.
pub fn named(agent: &AgentInfo) -> bool {
    let session = agent.agent_session.as_ref();
    kind(agent).is_some_and(|kind| session.is_some_and(|s| AgentKind::of(&s.agent) == kind))
}

/// Whether gerdr can find the chats of agent `kind` with no help from
/// herdr: by folder and start time.
pub fn guessable(kind: AgentKind) -> bool {
    integrations::of(kind).guesses()
}

/// The chat of the agent in a pane, as herdr knows it; else why there is
/// none, said to the user.
pub fn find(agent: &AgentInfo, homes: &Homes) -> Result<Source, String> {
    let session = agent.agent_session.as_ref();
    let Some(kind) = kind(agent) else {
        return Err(NOT_FOUND.into());
    };
    let agent_of = integrations::of(kind);
    if !agent_of.caps().chat {
        let name = (agent.agent.as_deref()).or(session.map(|s| s.agent.as_str()));
        let shown = agent
            .display_agent
            .as_deref()
            .or(name)
            .unwrap_or("this agent");
        return Err(format!("Chats of {shown} show in a later gerdr"));
    }
    let Some(session) = session.filter(|s| AgentKind::of(&s.agent) == kind) else {
        return Err(NOT_FOUND.into());
    };
    match session.kind.as_str() {
        "path" => {
            let path = match agent_of.transcript_at(&session.value, homes) {
                Some(path) => path,
                // Claude names its chat at start, before writing a line.
                None if not_written(&session.value, &agent_of.transcripts_under(homes)) => {
                    return Err(NOT_YET.into());
                }
                None => return Err(NOT_FOUND.into()),
            };
            let stem = (path.file_stem().and_then(|s| s.to_str())).ok_or(NOT_FOUND)?;
            // A rollout is `rollout-<yyyy-mm-ddThh-mm-ss>-<id>`, a pi session
            // `<time>_<id>`, an Antigravity transcript in
            // `brain/<id>/.system_generated/logs/`.
            let id = agent_of.id_in_path(stem, &path);
            Ok(Source {
                agent: kind,
                id: id.to_owned(),
                path,
                guessed: false,
            })
        }
        "id" if locate::plain_id(&session.value) => {
            let found = locate(
                homes,
                kind,
                Some(&session.value),
                None,
                Span::of(0),
                &HashSet::new(),
            )?;
            found.ok_or_else(|| NOT_YET.into())
        }
        _ => Err(NOT_FOUND.into()),
    }
}

/// The chat of agent `kind`'s run `run`, by its folder and start time, not
/// one of `claimed` (other panes'). It may be another chat of the agent
/// there: the source says it was guessed.
pub fn by_time(
    kind: AgentKind,
    run: &Run,
    homes: &Homes,
    claimed: &HashSet<String>,
) -> Result<Option<Source>, String> {
    match guessable(kind) {
        true => locate(homes, kind, None, Some(run), Span::of(run.started), claimed),
        false => Ok(None),
    }
}

/// How far back `newest` looks at most, so the chats it lists each few
/// seconds stay few.
const NEWEST_BACK: u64 = 86_400;

/// The newest chat of agent `kind` in `run`'s folder begun since the run
/// did (a day back at most), not one of `claimed`: the one the run moved
/// on to (`/new`) from the one `by_time` guessed. A guess too.
pub fn newest(
    kind: AgentKind,
    run: &Run,
    homes: &Homes,
    claimed: &HashSet<String>,
) -> Result<Option<Source>, String> {
    let now = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH))
        .map_or(0, |d| d.as_secs());
    // A minute ahead for a clock a little off; the latest is the nearest it.
    let to = now + 60;
    let span = Span {
        from: (Span::of(run.started).from).max(now.saturating_sub(NEWEST_BACK)),
        to,
        near: to,
    };
    match guessable(kind) {
        true => locate(homes, kind, None, Some(run), span, claimed),
        false => Ok(None),
    }
}

/// The problem said when herdr's integration for agent `kind`, `target`
/// to herdr, isn't installed.
pub fn not_installed(kind: AgentKind, target: &str) -> String {
    let install = format!("Install it with “herdr integration install {target}”.");
    match guessable(kind) {
        true => format!(
            "herdr's {target} integration isn't installed, so gerdr finds {target} chats by \
             folder and start time and can pick the wrong one. {install}"
        ),
        false => format!(
            "herdr's {target} integration isn't installed, so gerdr can't find {target} \
             chats. {install}"
        ),
    }
}

/// Said when no chat was found and herdr's integration for `target` isn't
/// installed.
pub fn missing_integration(target: &str) -> String {
    format!(
        "{NOT_FOUND}. herdr's {target} integration isn't installed: \
         “herdr integration install {target}” lets gerdr find its chats"
    )
}

/// A chat to look for with `locate`: `id`, else the one of `run`.
pub(super) fn chat(agent: AgentKind, id: Option<&str>, run: Option<&Run>) -> Chat {
    Chat {
        agent,
        id: id.map(str::to_owned),
        // No stage's: `locate` doesn't look at it.
        stage: Stage::Build,
        n: 0,
        started: run.map_or(0, |run| run.started),
        folders: run.map(|run| vec![run.cwd.clone()]).unwrap_or_default(),
        machine: None,
    }
}

/// Chat `id` of agent `kind`, else the one of `run` begun in `span`, as
/// `locate` finds it.
fn locate(
    homes: &Homes,
    kind: AgentKind,
    id: Option<&str>,
    run: Option<&Run>,
    span: Span,
    claimed: &HashSet<String>,
) -> Result<Option<Source>, String> {
    let chat = chat(kind, id, run);
    let found = (integrations::of(kind).find_pane(homes, &chat, claimed, span))
        .map_err(|err| format!("Couldn't read this agent's chats: {err}"))?;
    Ok(found.map(|found| Source {
        agent: kind,
        id: found.id,
        path: found.main,
        guessed: id.is_none(),
    }))
}

/// `path` as it really is, if it is a Claude transcript: a regular
/// `.jsonl` file under `projects/` of one of the config folders `homes`.
pub(crate) fn claude_transcript(path: &str, homes: &[PathBuf]) -> Option<PathBuf> {
    transcript(path, &claude_projects(homes), |_| true)
}

/// Where Claude keeps its transcripts under each of its `homes`.
pub(crate) fn claude_projects(homes: &[PathBuf]) -> Vec<PathBuf> {
    homes.iter().map(|home| home.join("projects")).collect()
}

/// Whether `path` names a `.jsonl` file not written yet under one of
/// folders `under`: the folders on its way that are there lead into one
/// (no link or `..` out), and the file isn't there.
fn not_written(path: &str, under: &[PathBuf]) -> bool {
    let path = Path::new(path);
    if !path.is_absolute() || path.exists() || path.extension().is_none_or(|ext| ext != "jsonl") {
        return false;
    }
    // The nearest folder on its way that is there, as it really is, and
    // the plain names after it.
    let mut rest = Vec::new();
    let mut at = path;
    let real = loop {
        let Some(name) = at.file_name() else {
            return false;
        };
        rest.push(name);
        let Some(parent) = at.parent() else {
            return false;
        };
        if let Ok(real) = parent.canonicalize() {
            break real;
        }
        at = parent;
    };
    let real = rest.iter().rev().fold(real, |path, name| path.join(name));
    (under.iter())
        .filter_map(|dir| dir.canonicalize().ok())
        .any(|dir| real.starts_with(dir))
}

/// `path` as it really is, if it is a Codex rollout: a regular
/// `rollout-*.jsonl` file under `sessions/` of Codex's home `codex`.
pub(crate) fn codex_rollout(path: &str, codex: &Path) -> Option<PathBuf> {
    transcript(path, &[codex.join("sessions")], |name| {
        name.starts_with("rollout-")
    })
}

/// `path` as it really is, if it is a regular `.jsonl` file under one of
/// folders `under` whose name passes `name_ok`. The path comes from herdr's
/// socket, which another process writes: no `..` or link leads out.
pub(crate) fn transcript(
    path: &str,
    under: &[PathBuf],
    name_ok: impl Fn(&str) -> bool,
) -> Option<PathBuf> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return None;
    }
    let real = path.canonicalize().ok()?;
    let inside = (under.iter())
        .filter_map(|dir| dir.canonicalize().ok())
        .any(|dir| real.starts_with(dir));
    let name = real.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let jsonl = real.extension().is_some_and(|ext| ext == "jsonl");
    (inside && jsonl && name_ok(name) && real.is_file()).then_some(real)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::herdr::AgentSession;

    #[test]
    fn a_claude_chat_named_before_its_first_line_is_not_written_yet() {
        let dir = std::env::temp_dir().join(format!("gerdr-nw-{}", std::process::id()));
        let claude = dir.join(".claude");
        std::fs::create_dir_all(claude.join("projects")).unwrap();
        let homes = Homes {
            claude: vec![claude.clone()],
            ..Homes::under(&dir)
        };
        // Its project's folder isn't there either yet.
        let path = claude.join("projects/-new/s1.jsonl");
        let found = find(&agent("claude", "path", path.to_str().unwrap()), &homes);
        assert_eq!(found, Err(NOT_YET.into()));
        // Out of Claude's folders, or through `..`: not its chat.
        let out = dir.join("elsewhere/s1.jsonl");
        assert_eq!(
            find(&agent("claude", "path", out.to_str().unwrap()), &homes),
            Err(NOT_FOUND.into())
        );
        let sneaky = claude.join("projects/../../x/s1.jsonl");
        assert_eq!(
            find(&agent("claude", "path", sneaky.to_str().unwrap()), &homes),
            Err(NOT_FOUND.into())
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pi_and_antigravity_chats_named_by_path_are_read_there() {
        let dir = std::env::temp_dir().join(format!("gerdr-piagy-{}", std::process::id()));
        let homes = Homes::under(&dir);
        let put = |path: &Path| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "{}\n").unwrap();
        };
        let session = homes.pi.join("--w--/2026-01-02T03-04-05-000Z_p-1.jsonl");
        put(&session);
        let found = find(&agent("pi", "path", session.to_str().unwrap()), &homes).unwrap();
        assert_eq!((found.id.as_str(), found.guessed), ("p-1", false));
        // herdr's hook names the cut transcript: the whole one is read.
        let logs = homes.antigravity.join("brain/a-1/.system_generated/logs");
        put(&logs.join("transcript.jsonl"));
        put(&logs.join("transcript_full.jsonl"));
        let cut = logs.join("transcript.jsonl");
        let found = find(&agent("agy", "path", cut.to_str().unwrap()), &homes).unwrap();
        assert_eq!(found.id, "a-1");
        assert!(found.path.ends_with("logs/transcript_full.jsonl"));
        // Out of their folders: not theirs.
        let out = dir.join("x.jsonl");
        put(&out);
        assert!(find(&agent("pi", "path", out.to_str().unwrap()), &homes).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    pub(crate) fn agent(name: &str, kind: &str, value: &str) -> AgentInfo {
        let info = serde_json::json!({
            "pane_id": "p1", "terminal_id": "t1", "workspace_id": "w1", "tab_id": "tab1",
            "agent": name, "display_agent": null, "name": null, "agent_session": null,
            "cwd": null, "foreground_cwd": null, "title": null, "terminal_title": null,
            "terminal_title_stripped": null,
        });
        let mut info: AgentInfo = serde_json::from_value(info).expect("agent info");
        info.agent_session = Some(AgentSession {
            source: format!("herdr:{name}"),
            agent: name.into(),
            kind: kind.into(),
            value: value.into(),
        });
        info
    }

    fn home(name: &str) -> (PathBuf, Homes) {
        let dir =
            std::env::temp_dir().join(format!("gerdr-chat-src-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".claude/projects/-work")).expect("dir");
        let homes = Homes::under(&dir);
        (dir, homes)
    }

    fn put(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).expect("dir");
        std::fs::write(path, text).expect("write");
    }

    #[test]
    fn an_id_finds_its_transcript_once_written() {
        let (dir, homes) = home("id");
        let info = agent("claude", "id", "abc-123_x");
        assert_eq!(find(&info, &homes), Err(NOT_YET.into()));
        let file = dir.join(".claude/projects/-work/abc-123_x.jsonl");
        std::fs::write(&file, "{}\n").expect("write");
        let source = find(&info, &homes).expect("found");
        assert_eq!(
            (source.agent, source.id.as_str()),
            (AgentKind::Claude, "abc-123_x")
        );
        assert_eq!(source.path, file);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn codex_opencode_and_cursor_ids_find_their_chats_once_written() {
        let (dir, homes) = home("ids");
        let found = |name: &str, id: &str| find(&agent(name, "id", id), &homes);
        assert_eq!(found("codex", "019a-1"), Err(NOT_YET.into()));
        assert_eq!(found("opencode", "ses_a"), Err(NOT_YET.into()));
        assert_eq!(found("cursor", "c-1"), Err(NOT_YET.into()));
        let rollout =
            dir.join(".codex/sessions/2026/08/26/rollout-2026-08-26T13-58-45-019a-1.jsonl");
        put(&rollout, "{}\n");
        let store = dir.join(".cursor/chats/f00/c-1/store.db");
        put(&store, "");
        std::fs::create_dir_all(homes.opencode.parent().unwrap()).expect("dir");
        let conn = rusqlite::Connection::open(&homes.opencode).expect("db");
        conn.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT, time_created INTEGER);
             CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);
             INSERT INTO session VALUES ('ses_a', NULL, '/w', 100500);",
        )
        .expect("tables");
        drop(conn);
        let of = |name: &str, id: &str| found(name, id).map(|s| (s.agent, s.id, s.path, s.guessed));
        assert_eq!(
            of("codex", "019a-1"),
            Ok((AgentKind::Codex, "019a-1".into(), rollout, false))
        );
        assert_eq!(
            of("opencode", "ses_a"),
            Ok((
                AgentKind::OpenCode,
                "ses_a".into(),
                homes.opencode.clone(),
                false
            ))
        );
        assert_eq!(
            of("cursor", "c-1"),
            Ok((AgentKind::Cursor, "c-1".into(), store, false))
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ids_that_could_leave_the_folder_are_refused() {
        let (dir, homes) = home("badid");
        std::fs::write(dir.join(".claude/x.jsonl"), "{}\n").expect("write");
        for name in ["claude", "codex", "cursor", "opencode"] {
            for id in ["../x", "a/b", "", "a b", "a.b"] {
                assert_eq!(
                    find(&agent(name, "id", id), &homes),
                    Err(NOT_FOUND.into()),
                    "{name} {id}"
                );
            }
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_path_is_used_only_inside_the_home() {
        let (dir, homes) = home("path");
        let inside = dir.join(".claude/projects/-work/s1.jsonl");
        std::fs::write(&inside, "{}\n").expect("write");
        let source = find(&agent("claude", "path", inside.to_str().unwrap()), &homes);
        assert_eq!(
            source.map(|s| (s.id, s.path)),
            Ok(("s1".into(), inside.canonicalize().unwrap()))
        );
        let outside = dir.join("s2.jsonl");
        std::fs::write(&outside, "{}\n").expect("write");
        let refused = |path: &Path| find(&agent("claude", "path", path.to_str().unwrap()), &homes);
        assert_eq!(refused(&outside), Err(NOT_FOUND.into()));
        // A `..` out, a link out, a folder, a file not a transcript, a relative path.
        assert_eq!(
            refused(&dir.join(".claude/projects/-work/../../../s2.jsonl")),
            Err(NOT_FOUND.into())
        );
        let link = dir.join(".claude/projects/-work/link.jsonl");
        std::os::unix::fs::symlink(&outside, &link).expect("link");
        assert_eq!(refused(&link), Err(NOT_FOUND.into()));
        assert_eq!(
            refused(&dir.join(".claude/projects/-work")),
            Err(NOT_FOUND.into())
        );
        std::fs::write(dir.join(".claude/.credentials.json"), "{}").expect("write");
        assert_eq!(
            refused(&dir.join(".claude/.credentials.json")),
            Err(NOT_FOUND.into())
        );
        assert_eq!(
            refused(Path::new("projects/-work/s1.jsonl")),
            Err(NOT_FOUND.into())
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_codex_path_is_used_only_if_a_rollout_in_its_sessions() {
        let (dir, homes) = home("codex-path");
        let day = dir.join(".codex/sessions/2026/08/26");
        let rollout = day.join("rollout-2026-08-26T13-58-45-019a-1.jsonl");
        put(&rollout, "{}\n");
        let path = |path: &Path| find(&agent("codex", "path", path.to_str().unwrap()), &homes);
        assert_eq!(
            path(&rollout).map(|s| (s.id, s.path)),
            Ok(("019a-1".into(), rollout.canonicalize().unwrap()))
        );
        let outside = dir.join("rollout-2026-08-26T13-58-45-x.jsonl");
        put(&outside, "{}\n");
        assert_eq!(path(&outside), Err(NOT_FOUND.into()));
        assert_eq!(
            path(&day.join("../../../../rollout-2026-08-26T13-58-45-x.jsonl")),
            Err(NOT_FOUND.into())
        );
        let link = day.join("rollout-2026-08-26T13-58-45-link.jsonl");
        std::os::unix::fs::symlink(&outside, &link).expect("link");
        assert_eq!(path(&link), Err(NOT_FOUND.into()));
        let other = day.join("history.jsonl");
        put(&other, "{}\n");
        assert_eq!(path(&other), Err(NOT_FOUND.into()));
        let not_jsonl = day.join("rollout-2026-08-26T13-58-45-y.json");
        put(&not_jsonl, "{}\n");
        assert_eq!(path(&not_jsonl), Err(NOT_FOUND.into()));
        // Cursor and OpenCode name no paths.
        assert_eq!(
            find(&agent("cursor", "path", rollout.to_str().unwrap()), &homes),
            Err(NOT_FOUND.into())
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_run_finds_its_chat_by_folder_and_time_as_a_guess() {
        let (dir, homes) = home("by-time");
        let transcript = dir.join(".cursor/projects/work-app/agent-transcripts/c-1");
        std::fs::create_dir_all(&transcript).expect("dir");
        put(&dir.join(".cursor/chats/f00/c-1/store.db"), "");
        let started = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let run = Run {
            started,
            cwd: "/work/app".into(),
            pid: 0,
        };
        let none = HashSet::new();
        let source = by_time(AgentKind::Cursor, &run, &homes, &none).expect("read");
        assert_eq!(
            source.map(|s| (s.id, s.guessed)),
            Some(("c-1".into(), true))
        );
        assert_eq!(by_time(AgentKind::Claude, &run, &homes, &none), Ok(None));
        assert_eq!(by_time(AgentKind::Codex, &run, &homes, &none), Ok(None));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn other_agents_and_no_session_say_why() {
        let (dir, homes) = home("other");
        let t3 = find(&agent("t3", "id", "abc"), &homes);
        assert_eq!(t3, Err("Chats of t3 show in a later gerdr".into()));
        let mut none = agent("claude", "id", "abc");
        none.agent_session = None;
        assert_eq!(find(&none, &homes), Err(NOT_FOUND.into()));
        assert!(!named(&none));
        let mut other = agent("codex", "id", "abc");
        other.agent_session.as_mut().unwrap().agent = "claude".into();
        assert_eq!(find(&other, &homes), Err(NOT_FOUND.into()));
        assert!(!named(&other));
        std::fs::remove_dir_all(&dir).ok();
    }
}

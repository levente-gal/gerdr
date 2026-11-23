//! Which chat a pane's agent holds, and where its transcript is.

use std::path::{Path, PathBuf};

use super::Source;
use crate::{
    ai_work::{
        AgentKind,
        locate::{self, Homes},
    },
    herdr::AgentInfo,
};

/// Said when the pane's chat can't be found.
pub const NOT_FOUND: &str = "Couldn't find this agent's chat. Its screen still works";
/// Said when herdr names a chat that isn't on disk yet: Claude writes it
/// with its first message.
pub const NOT_YET: &str = "No transcript of this chat yet. It shows once the agent writes one";

/// The chat of the agent in a pane, as herdr knows it; else why there is
/// none, said to the user.
pub fn find(agent: &AgentInfo, homes: &Homes) -> Result<Source, String> {
    let session = agent.agent_session.as_ref();
    let Some(name) = (agent.agent.as_deref()).or(session.map(|s| s.agent.as_str())) else {
        return Err(NOT_FOUND.into());
    };
    let kind = AgentKind::of(name);
    // shortcut: only Claude's chats are read; the others come in Inc 2.
    if kind != AgentKind::Claude {
        let shown = agent.display_agent.as_deref().unwrap_or(name);
        return Err(format!("Chats of {shown} show in a later gerdr"));
    }
    let Some(session) = session.filter(|s| AgentKind::of(&s.agent) == kind) else {
        return Err(NOT_FOUND.into());
    };
    match session.kind.as_str() {
        "path" => {
            let path = claude_transcript(&session.value, &homes.claude).ok_or(NOT_FOUND)?;
            let id = (path.file_stem().and_then(|s| s.to_str()))
                .ok_or(NOT_FOUND)?
                .to_owned();
            Ok(Source {
                agent: kind,
                id,
                path,
                guessed: false,
            })
        }
        "id" if locate::plain_id(&session.value) => {
            let found = locate::claude(homes, &session.value).ok_or(NOT_YET)?;
            Ok(Source {
                agent: kind,
                id: found.id,
                path: found.main,
                guessed: false,
            })
        }
        _ => Err(NOT_FOUND.into()),
    }
}

/// `path` as it really is, if it is a Claude transcript: a regular
/// `.jsonl` file under `projects/` of one of the config folders `homes`.
/// The path comes from herdr's socket, which another process writes: no
/// `..` or link leads out.
fn claude_transcript(path: &str, homes: &[PathBuf]) -> Option<PathBuf> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return None;
    }
    let real = path.canonicalize().ok()?;
    let inside = (homes.iter())
        .filter_map(|home| home.join("projects").canonicalize().ok())
        .any(|projects| real.starts_with(projects));
    let jsonl = real.extension().is_some_and(|ext| ext == "jsonl");
    (inside && jsonl && real.is_file()).then_some(real)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::herdr::AgentSession;

    pub(crate) fn agent(name: &str, kind: &str, value: &str) -> AgentInfo {
        let info = serde_json::json!({
            "pane_id": "p1", "terminal_id": "t1", "workspace_id": "w1", "tab_id": "tab1",
            "agent": name, "display_agent": null, "name": null, "agent_session": null,
            "cwd": null, "foreground_cwd": null, "title": null, "terminal_title": null,
            "terminal_title_stripped": null,
        });
        let mut info: AgentInfo = serde_json::from_value(info).expect("agent info");
        info.agent_session = Some(AgentSession {
            source: "herdr:claude".into(),
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
    fn ids_that_could_leave_the_folder_are_refused() {
        let (dir, homes) = home("badid");
        std::fs::write(dir.join(".claude/x.jsonl"), "{}\n").expect("write");
        for id in ["../x", "a/b", "", "a b", "a.b"] {
            assert_eq!(
                find(&agent("claude", "id", id), &homes),
                Err(NOT_FOUND.into()),
                "{id}"
            );
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
    fn other_agents_and_no_session_say_why() {
        let (dir, homes) = home("other");
        let codex = find(&agent("codex", "id", "abc"), &homes);
        assert_eq!(codex, Err("Chats of codex show in a later gerdr".into()));
        let mut none = agent("claude", "id", "abc");
        none.agent_session = None;
        assert_eq!(find(&none, &homes), Err(NOT_FOUND.into()));
        std::fs::remove_dir_all(&dir).ok();
    }
}

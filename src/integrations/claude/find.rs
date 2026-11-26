//! Where Claude Code keeps a chat: `projects/<folder>/<id>.jsonl` of each
//! config folder, with its tool results and subagents beside it.

use std::path::PathBuf;

use crate::{
    ai_work::{
        AgentKind,
        locate::{Form, Found, Homes, files_under, found},
    },
    chat::source,
};

/// Claude Code chat `id`: `projects/<folder>/<id>.jsonl` of a config
/// folder, and its `<id>/tool-results` and `<id>/subagents`.
pub(crate) fn by_id(homes: &Homes, id: &str) -> Option<Found> {
    let name = format!("{id}.jsonl");
    homes.claude.iter().find_map(|config| {
        let folders = std::fs::read_dir(config.join("projects")).ok()?;
        let main = (folders.flatten())
            .map(|dir| dir.path().join(&name))
            .find(|path| path.is_file())?;
        let mut chat = found(AgentKind::Claude, id, main.clone(), Form::Lines);
        chat.extra = files_under(&main.with_extension(""), &["tool-results", "subagents"]);
        Some(chat)
    })
}

/// The body of `locate::script` for Claude Code chat `id`.
pub(crate) fn script(id: &str) -> String {
    format!(
        r#"id='{id}'
for d in "${{CLAUDE_CONFIG_DIR:-$HOME/.claude}}" "$HOME/.claude"; do
  for f in "$d"/projects/*/"$id.jsonl"; do
    [ -f "$f" ] || continue
    say M '' "$f"
    b="${{f%.jsonl}}"
    for s in tool-results subagents; do
      [ -d "$b/$s" ] || continue
      find "$b/$s" -type f | while IFS= read -r x; do say X "${{x#"$b/"}}" "$x"; done
    done
    exit 0
  done
done
"#
    )
}

/// `path` as it really is, if it is a Claude transcript: a regular
/// `.jsonl` file under `projects/` of one of the config folders `homes`.
pub(crate) fn transcript(path: &str, homes: &[PathBuf]) -> Option<PathBuf> {
    source::transcript(path, &projects(homes), |_| true)
}

/// Where Claude keeps its transcripts under each of its `homes`.
pub(crate) fn projects(homes: &[PathBuf]) -> Vec<PathBuf> {
    homes.iter().map(|home| home.join("projects")).collect()
}

/// The transcript of the Claude Code chat running in herdr pane `pane`, if
/// herdr knows its id and it is on disk. Claude Code draws on the
/// alternate screen, where herdr keeps no scrollback: this stands for it.
pub(crate) fn claude_chat(herdr: &mut crate::herdr::Herdr, pane: &str) -> Option<PathBuf> {
    let agent = herdr
        .agent_list()
        .ok()?
        .into_iter()
        .find(|a| a.pane_id == pane)?;
    let session = agent
        .agent_session
        .filter(|s| s.agent == "claude" && s.kind == "id")?;
    // It goes into a path: an id, not a way out of the folder.
    let plain = (session.value.chars()).all(|c| c.is_ascii_alphanumeric() || c == '-');
    let homes = Homes::here()?;
    plain
        .then(|| by_id(&homes, &session.value))
        .flatten()
        .map(|f| f.main)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::ai_work::{
        AgentKind,
        locate::{
            Form, Homes, find,
            tests::{chat, put},
        },
        write::tests::scratch,
    };

    #[test]
    fn a_claude_chat_is_found_with_its_extras() {
        let home = scratch("locate-claude");
        let homes = Homes::under(&home);
        let folder = home.join(".claude/projects/-work-app");
        put(&folder.join("abc-1.jsonl"), "{}\n");
        put(&folder.join("abc-1/tool-results/t1.txt"), "out");
        put(&folder.join("abc-1/subagents/agent-x.jsonl"), "{}\n");
        put(&folder.join("abc-1/other/skip.txt"), "no");
        put(&home.join(".claude/projects/-other/zzz.jsonl"), "{}\n");
        let c = chat(AgentKind::Claude, Some("abc-1"), 0, &[]);
        let f = find(&homes, &c, &HashSet::new()).unwrap().unwrap();
        assert_eq!(f.main, folder.join("abc-1.jsonl"));
        assert_eq!(f.form, Form::Lines);
        let rels: Vec<&str> = f.extra.iter().map(|(rel, _)| rel.as_str()).collect();
        assert_eq!(rels, ["subagents/agent-x.jsonl", "tool-results/t1.txt"]);
        let gone = chat(AgentKind::Claude, Some("nope"), 0, &[]);
        assert_eq!(find(&homes, &gone, &HashSet::new()).unwrap(), None);
        let bad = chat(AgentKind::Claude, Some("../x"), 0, &[]);
        assert!(find(&homes, &bad, &HashSet::new()).is_err());
    }
}

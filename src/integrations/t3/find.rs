//! Where T3 Code keeps a thread: rows of its database, and the chat of
//! the agent the thread ran.

use std::collections::HashSet;

use serde_json::Value;

use crate::ai_work::{
    AgentKind, Chat,
    locate::{Form, Found, Homes, find, found, plain_id, pretty, rows_json},
};

/// A T3 Code thread's rows: the thread, its runs, its agents' threads.
type T3Rows = (Vec<Value>, Vec<Value>, Vec<Value>);

/// T3 Code thread `id`: its row, runs and agent threads, then the chat of
/// the agent it ran (Claude, Codex or Cursor) when that is found.
pub(crate) fn thread(homes: &Homes, chat: &Chat, id: &str) -> Result<Option<Found>, String> {
    let Some(home) = &homes.t3 else {
        return Ok(None);
    };
    let db = home.join(crate::t3_agents::DB);
    if !db.is_file() {
        return Ok(None);
    }
    let read = || -> rusqlite::Result<Option<T3Rows>> {
        let conn = crate::t3_agents::open_read_only(&db)?;
        conn.busy_timeout(std::time::Duration::from_secs(2))?;
        let threads = rows_json(
            &conn,
            "SELECT * FROM orchestration_v2_projection_threads WHERE thread_id = ?1",
            id,
        )?;
        if threads.is_empty() {
            return Ok(None);
        }
        let runs = rows_json(
            &conn,
            "SELECT * FROM orchestration_v2_projection_runs WHERE thread_id = ?1 ORDER BY ordinal",
            id,
        )?;
        // Newer than the schema gerdr reads threads by: missing is no error.
        let agents = rows_json(
            &conn,
            "SELECT * FROM orchestration_v2_projection_provider_threads WHERE thread_id = ?1 \
             ORDER BY updated_at",
            id,
        )
        .unwrap_or_default();
        Ok(Some((threads, runs, agents)))
    };
    let Some((threads, runs, agents)) =
        read().map_err(|err| format!("reading {}: {err}", db.display()))?
    else {
        return Ok(None);
    };
    let rows = vec![
        ("t3/thread.json".to_owned(), pretty(&Value::Array(threads))),
        ("t3/runs.json".to_owned(), pretty(&Value::Array(runs))),
        (
            "t3/agent-threads.json".to_owned(),
            pretty(&Value::Array(agents.clone())),
        ),
    ];
    let native = agents.iter().rev().find_map(|row| {
        let native = &row["payload_json"]["nativeThreadRef"];
        let driver = native["driver"].as_str().or(row["provider"].as_str())?;
        let native_id = native["nativeId"].as_str()?;
        let agent = match AgentKind::of(driver) {
            AgentKind::Claude => (AgentKind::Claude, native_id),
            AgentKind::Codex => (AgentKind::Codex, native_id),
            AgentKind::Cursor => (AgentKind::Cursor, native_id.trim_start_matches("agent-")),
            _ => return None,
        };
        plain_id(agent.1).then(|| (agent.0, agent.1.to_owned()))
    });
    let theirs = native.and_then(|(agent, native_id)| {
        let inner = Chat {
            agent,
            id: Some(native_id),
            ..chat.clone()
        };
        // The thread's rows are kept all the same.
        (find(homes, &inner, &HashSet::new()))
            .inspect_err(|err| tracing::warn!("finding the chat of T3 thread {id}: {err}"))
            .ok()
            .flatten()
    });
    let mut kept = theirs.unwrap_or_else(|| found(AgentKind::T3, id, db.clone(), Form::Nothing));
    kept.rows.extend(rows);
    Ok(Some(kept))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use serde_json::Value;

    use crate::ai_work::{
        AgentKind,
        locate::{
            Form, Homes, columns, find,
            tests::{chat, put},
        },
        write::tests::scratch,
    };

    #[test]
    fn a_t3_thread_leads_to_its_claude_chat() {
        let home = scratch("locate-t3");
        let mut homes = Homes::under(&home);
        let t3 = home.join(".t3");
        homes.t3 = Some(t3.clone());
        let db = t3.join(crate::t3_agents::DB);
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(include_str!("../../t3_schema.sql"))
            .unwrap();
        conn.execute_batch(
            "INSERT INTO orchestration_v2_projection_threads (thread_id, project_id, title, \
               default_provider, runtime_mode, interaction_mode, created_at, updated_at, payload_json) \
             VALUES ('th-1', 'p', 'T', 'claudeAgent', 'full', 'default', 'a', 'b', '{}');
             INSERT INTO orchestration_v2_projection_runs (run_id, thread_id, ordinal, provider, status, \
               requested_at, payload_json) VALUES ('r1', 'th-1', 1, 'claudeAgent', 'completed', 'a', '{}');",
        )
        .unwrap();
        let missing = columns(&conn, "orchestration_v2_projection_provider_threads")
            .unwrap()
            .is_empty();
        if missing {
            conn.execute_batch(
                "CREATE TABLE orchestration_v2_projection_provider_threads (provider_thread_id TEXT, \
                   thread_id TEXT, provider TEXT, updated_at TEXT, payload_json TEXT);",
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO orchestration_v2_projection_provider_threads (provider_thread_id, thread_id, \
               provider, updated_at, payload_json) VALUES ('pt', 'th-1', 'claudeAgent', 'c', ?1)",
            [r#"{"nativeThreadRef":{"driver":"claudeAgent","nativeId":"cl-9"}}"#],
        )
        .unwrap();
        drop(conn);
        let c = chat(AgentKind::T3, Some("th-1"), 0, &[]);
        // Its Claude chat isn't there yet: the thread's rows alone.
        let f = find(&homes, &c, &HashSet::new()).unwrap().unwrap();
        assert_eq!((f.agent, f.form.clone()), (AgentKind::T3, Form::Nothing));
        assert_eq!(f.rows.len(), 3);
        put(&home.join(".claude/projects/-w/cl-9.jsonl"), "{}\n");
        let f = find(&homes, &c, &HashSet::new()).unwrap().unwrap();
        assert_eq!((f.agent, f.id.as_str()), (AgentKind::Claude, "cl-9"));
        let runs: Value = serde_json::from_slice(&f.rows[1].1).unwrap();
        assert_eq!(runs[0]["run_id"], "r1");
    }
}

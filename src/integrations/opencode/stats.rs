//! How full an OpenCode chat's context is: what its last answer sent, by
//! the `tokens` OpenCode keeps on each assistant message.

use std::path::Path;

use serde_json::Value;

use crate::ai_work::context::{Context, num};

/// The context of OpenCode chat `id` in database `db`: its last answer's
/// input, read and written cache. Older chats are in `message`, newer ones
/// in `session_message`; the model's window isn't kept, so none is said.
pub(crate) fn context(db: &Path, id: &str) -> Option<Context> {
    let conn = crate::cursor_agents::open_read_only(db).ok()?;
    let tables = [
        "SELECT data FROM message WHERE session_id = ?1 ORDER BY time_created DESC LIMIT 50",
        "SELECT data FROM session_message WHERE session_id = ?1 ORDER BY seq DESC LIMIT 50",
    ];
    tables.iter().find_map(|sql| {
        let mut rows = conn.prepare(sql).ok()?;
        let datas = rows.query_map([id], |r| r.get::<_, String>(0)).ok()?;
        let mut datas = datas.flatten();
        datas.find_map(|data| used(&serde_json::from_str(&data).ok()?))
    })
}

/// The context message `data` says it sent, when it has tokens.
fn used(data: &Value) -> Option<Context> {
    let t = &data["tokens"];
    let used = num(&t["input"]) + num(&t["cache"]["read"]) + num(&t["cache"]["write"]);
    (used > 0).then_some(Context { used, window: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(name: &str, sql: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("gerdr-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("opencode.db");
        let _ = std::fs::remove_file(&db);
        rusqlite::Connection::open(&db)
            .unwrap()
            .execute_batch(sql)
            .unwrap();
        db
    }

    #[test]
    fn an_opencode_chat_says_its_last_answers_tokens() {
        let db = db(
            "opencode-context",
            "CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT);
             CREATE TABLE session_message (id TEXT, session_id TEXT, seq INTEGER, data TEXT);
             INSERT INTO message VALUES ('m1', 'ses_a', 1, '{\"role\":\"assistant\",\"tokens\":{\"input\":9,\"cache\":{\"read\":1000,\"write\":0}}}');
             INSERT INTO message VALUES ('m2', 'ses_a', 2, '{\"role\":\"assistant\",\"tokens\":{\"input\":251,\"output\":52,\"cache\":{\"read\":75904,\"write\":10}}}');
             INSERT INTO message VALUES ('m3', 'ses_a', 3, '{\"role\":\"user\"}');
             INSERT INTO session_message VALUES ('s1', 'ses_b', 1, '{\"tokens\":{\"input\":622,\"cache\":{\"read\":105088}}}');
             INSERT INTO session_message VALUES ('s2', 'ses_b', 2, '{\"finish\":\"stop\"}');",
        );
        let at = |id| context(&db, id).map(|c| c.used);
        assert_eq!(at("ses_a"), Some(76165));
        assert_eq!(at("ses_b"), Some(105710));
        assert_eq!(at("ses_none"), None);
    }
}

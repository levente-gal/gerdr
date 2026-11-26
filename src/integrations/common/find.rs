//! Finding an agent's chats: its folders and files, and the rows of its
//! database as JSON.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::ai_work::{
    AgentKind,
    locate::{Form, Found},
};

/// The most of a transcript's first line read: its session's start.
pub const FIRST_LINE: u64 = 1 << 20;

/// The files under `base`'s folders `subs`, by their path from `base`, in
/// order. Links aren't followed.
pub fn files_under(base: &Path, subs: &[&str]) -> Vec<(String, PathBuf)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => walk(&entry.path(), out),
                Ok(kind) if kind.is_file() => out.push(entry.path()),
                _ => {}
            }
        }
    }
    let mut paths = Vec::new();
    for sub in subs {
        walk(&base.join(sub), &mut paths);
    }
    let mut files: Vec<(String, PathBuf)> = (paths.into_iter())
        .filter_map(|path| {
            let rel = path.strip_prefix(base).ok()?.to_str()?.to_owned();
            Some((rel, path))
        })
        .collect();
    files.sort();
    files
}

pub fn found(agent: AgentKind, id: &str, main: PathBuf, form: Form) -> Found {
    Found {
        agent,
        id: id.to_owned(),
        main,
        extra: Vec::new(),
        form,
        rows: Vec::new(),
    }
}

/// The folders of `dir`, by name in order.
pub fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = (entries.flatten())
        .filter(|e| e.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|e| e.path())
        .collect();
    dirs.sort();
    dirs
}

/// `value` of a database row as JSON: texts of JSON columns (`*_json`,
/// `data`, `metadata`) as what they hold.
fn cell(name: &str, value: rusqlite::types::ValueRef) -> Value {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(n) => Value::from(n),
        ValueRef::Real(x) => Value::from(x),
        ValueRef::Text(text) => {
            let text = String::from_utf8_lossy(text);
            let json = name.ends_with("_json") || matches!(name, "data" | "metadata");
            (json.then(|| serde_json::from_str(&text).ok()).flatten())
                .unwrap_or_else(|| Value::String(text.into_owned()))
        }
        ValueRef::Blob(bytes) => Value::from(format!("<{} bytes>", bytes.len())),
    }
}

/// The rows `sql` gives for `arg`, each as a JSON object by column.
pub fn rows_json(
    conn: &rusqlite::Connection,
    sql: &str,
    arg: &str,
) -> rusqlite::Result<Vec<Value>> {
    rows_of(conn, sql, [arg])
}

/// The rows `sql` gives for `params`, each as a JSON object by column.
pub fn rows_of(
    conn: &rusqlite::Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> rusqlite::Result<Vec<Value>> {
    let mut query = conn.prepare(sql)?;
    let names: Vec<String> = query.column_names().iter().map(|n| n.to_string()).collect();
    let rows = query.query_map(params, |row| {
        let mut object = Map::new();
        for (i, name) in names.iter().enumerate() {
            object.insert(name.clone(), cell(name, row.get_ref(i)?));
        }
        Ok(Value::Object(object))
    })?;
    rows.collect()
}

/// The columns of table `table`; none when it isn't there.
pub fn columns(conn: &rusqlite::Connection, table: &str) -> rusqlite::Result<Vec<String>> {
    let mut query = conn.prepare("SELECT name FROM pragma_table_info(?1)")?;
    let names = query.query_map([table], |row| row.get(0))?;
    names.collect()
}

pub fn pretty(value: &Value) -> Vec<u8> {
    serde_json::to_vec_pretty(value).unwrap_or_default()
}

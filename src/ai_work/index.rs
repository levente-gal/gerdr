//! The index: `index.sqlite` under `ai-work`, drawn from the events files
//! for the stats panel. It is never the only copy of anything: gone, or of
//! another `user_version`, it is drawn again.
//!
//! Each events file is read on from where it was left, up to its last whole
//! line, all of a feature in one transaction. A file that shrank, or was put
//! in place of another, has the feature's rows drawn again from the start of
//! its files. The kept chats are read again when they, or the runs they
//! were of, change.

use std::{
    collections::BTreeMap,
    io::{Read as _, Seek as _, SeekFrom},
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};

use rusqlite::{
    Connection, ErrorCode, OpenFlags, OptionalExtension as _, Transaction, TransactionBehavior,
    params,
};
use serde_json::Value;

use super::{AgentKind, BlobRef, Event, FORMAT, Line, usage, zst};
use crate::feature::Stage;

/// The version of the tables (see `schema.sql`); `user_version` is
/// `SCHEMA * 1000 + FORMAT`.
pub const SCHEMA: u32 = 1;

/// The tables, as made in a new index.
pub const TABLES: &str = include_str!("schema.sql");

/// The tables that hold a feature's rows: all but `ingest`.
const FEATURE_TABLES: [&str; 16] = [
    "features",
    "stages",
    "runs",
    "run_changes",
    "sends",
    "sessions",
    "session_runs",
    "usage",
    "fix_rounds",
    "reviews",
    "findings",
    "verify_runs",
    "checks",
    "setups",
    "notes",
    "manual",
];

/// How long a write waits for a reader to let go.
const BUSY: Duration = Duration::from_secs(5);

/// The index's file under `root`.
pub fn path(root: &Path) -> PathBuf {
    root.join("index.sqlite")
}

/// The `user_version` of an index of these tables and lines.
pub fn version() -> i64 {
    i64::from(SCHEMA) * 1000 + i64::from(FORMAT)
}

/// The index under `root`, to write to; made, or made again when it is of
/// another version or no index at all. Another gerdr writing it a while,
/// or any other error, leaves it as it is: it is tried again later.
pub fn open(root: &Path) -> Result<Connection, String> {
    std::fs::create_dir_all(root).map_err(|e| format!("making {}: {e}", root.display()))?;
    if let Err(err) = std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)) {
        tracing::warn!("ai work: keeping {} private: {err}", root.display());
    }
    let file = path(root);
    match open_tables(&file) {
        Ok(conn) => Ok(conn),
        Err(Unopened::Redo(why)) => {
            // The connection is closed by now: what is left goes, then the
            // index is drawn again from the files.
            tracing::warn!("ai work: drawing {} again: {why}", file.display());
            remove(&file)?;
            open_tables(&file).map_err(|un| format!("making {}: {}", file.display(), un.why()))
        }
        Err(Unopened::Failed(why)) => Err(format!("opening {}: {why}", file.display())),
    }
}

/// Why the index couldn't be opened.
#[derive(Debug)]
enum Unopened {
    /// It is of another version, or no database at all: it is made again.
    Redo(String),
    /// Anything else, as a lock another gerdr holds: it stays.
    Failed(String),
}

impl Unopened {
    fn of(err: &rusqlite::Error) -> Self {
        match redo(err) {
            true => Self::Redo(err.to_string()),
            false => Self::Failed(err.to_string()),
        }
    }

    fn why(&self) -> &str {
        match self {
            Self::Redo(why) | Self::Failed(why) => why,
        }
    }
}

/// Whether `err` says the file is no index to read: what made again mends.
fn redo(err: &rusqlite::Error) -> bool {
    matches!(
        err.sqlite_error_code(),
        Some(ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt)
    )
}

/// The index at `file`, its tables made if it is new. Of another version,
/// or no database at all, it is to be made again.
fn open_tables(file: &Path) -> Result<Connection, Unopened> {
    let fail = |e: rusqlite::Error| Unopened::of(&e);
    let conn = Connection::open(file).map_err(fail)?;
    conn.busy_timeout(BUSY).map_err(fail)?;
    let found: i64 = (conn.query_row("PRAGMA user_version", [], |r| r.get(0))).map_err(fail)?;
    let tables: i64 =
        (conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))).map_err(fail)?;
    if found != version() {
        if found != 0 || tables != 0 {
            return Err(Unopened::Redo(format!(
                "its version is {found}, not {}",
                version()
            )));
        }
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(fail)?;
        conn.execute_batch(TABLES).map_err(fail)?;
        conn.pragma_update(None, "user_version", version())
            .map_err(fail)?;
    }
    for name in ["", "-wal", "-shm"] {
        let one = PathBuf::from(format!("{}{name}", file.display()));
        if let Err(err) = std::fs::set_permissions(&one, std::fs::Permissions::from_mode(0o600))
            && err.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!("ai work: keeping {} private: {err}", one.display());
        }
    }
    Ok(conn)
}

/// Remove the index at `file`, with its `-wal` and `-shm`.
fn remove(file: &Path) -> Result<(), String> {
    for name in ["", "-wal", "-shm"] {
        let one = PathBuf::from(format!("{}{name}", file.display()));
        match std::fs::remove_file(&one) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                return Err(format!("removing {}: {err}", one.display()));
            }
            _ => {}
        }
    }
    Ok(())
}

/// The index under `root`, only to read.
pub fn open_read(root: &Path) -> Result<Connection, String> {
    let file = path(root);
    if !file.exists() {
        return Err("the index isn't made yet".to_owned());
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let fail = |e: rusqlite::Error| format!("reading {}: {e}", file.display());
    let conn = Connection::open_with_flags(&file, flags).map_err(fail)?;
    conn.busy_timeout(BUSY).map_err(fail)?;
    let found: i64 = (conn.query_row("PRAGMA user_version", [], |r| r.get(0))).map_err(fail)?;
    if found != version() {
        return Err("the index is being drawn again".to_owned());
    }
    Ok(conn)
}

/// Every feature folder under `root`: `<project>/<slug>/`.
pub fn dirs(root: &Path) -> Vec<PathBuf> {
    let folders = |dir: &Path| -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut found: Vec<PathBuf> = (entries.flatten())
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .map(|e| e.path())
            .collect();
        found.sort();
        found
    };
    folders(root).iter().flat_map(|p| folders(p)).collect()
}

/// Empty every table of `conn`, for it to be drawn again from the files.
pub fn clear(conn: &mut Connection) -> rusqlite::Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for table in FEATURE_TABLES.iter().chain(&["ingest"]) {
        tx.execute(&format!("DELETE FROM {table}"), [])?;
    }
    tx.commit()
}

/// The index under `root`, emptied, to be drawn again from the files: the
/// file stays, so a reader open on it reads on. One that can't be emptied
/// as it is no index is made anew; one another gerdr holds stays.
pub fn reset(root: &Path) -> Result<Connection, String> {
    let mut conn = open(root)?;
    match clear(&mut conn) {
        Ok(()) => Ok(conn),
        Err(err) if redo(&err) => {
            drop(conn);
            let file = path(root);
            tracing::warn!("ai work: making {} anew: {err}", file.display());
            remove(&file)?;
            open(root)
        }
        Err(err) => Err(format!("emptying {}: {err}", path(root).display())),
    }
}

/// Take what was added to the events files of feature folders `dirs` since
/// last time into `conn`: the number of lines. A folder that fails is told
/// in the error once the others are in.
pub fn ingest(conn: &mut Connection, dirs: &[PathBuf]) -> Result<usize, String> {
    let mut lines = 0;
    let mut failed = Vec::new();
    for dir in dirs {
        // Several gerdrs may write it: each takes the lock before it reads.
        let one = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .and_then(|tx| {
                let n = ingest_dir(&tx, dir)?;
                tx.commit().map(|()| n)
            });
        match one {
            Ok(n) => lines += n,
            Err(err) => {
                tracing::warn!("ai work: indexing {}: {err}", dir.display());
                failed.push(format!("{}: {err}", dir.display()));
            }
        }
    }
    match failed.is_empty() {
        true => Ok(lines),
        false => Err(failed.join("; ")),
    }
}

/// A feature being taken in: its names and its folder.
struct Feature<'a> {
    project: String,
    slug: String,
    dir: &'a Path,
}

/// The project and slug feature folder `dir` stands for.
fn names(dir: &Path) -> Option<(String, String)> {
    let slug = dir.file_name()?.to_string_lossy().into_owned();
    let project = dir.parent()?.file_name()?.to_string_lossy().into_owned();
    Some((project, slug))
}

/// How far a file was read.
#[derive(Clone, Copy, Debug, Default)]
struct Read {
    offset: i64,
    size: i64,
    mtime: i64,
    bad: i64,
}

fn stored(tx: &Transaction, path: &str) -> rusqlite::Result<Option<Read>> {
    tx.query_row(
        "SELECT offset, size, mtime, bad_lines FROM ingest WHERE path = ?1",
        [path],
        |r| {
            Ok(Read {
                offset: r.get(0)?,
                size: r.get(1)?,
                mtime: r.get(2)?,
                bad: r.get(3)?,
            })
        },
    )
    .optional()
}

fn store(tx: &Transaction, path: &str, read: Read) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT OR REPLACE INTO ingest (path, offset, size, mtime, bad_lines)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![path, read.offset, read.size, read.mtime, read.bad],
    )
    .map(drop)
}

/// The size and the time it was changed (ms) of the file at `path`.
fn stat(path: &Path) -> Option<(i64, i64)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = (meta.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64);
    Some((meta.len() as i64, mtime))
}

/// A database error for what isn't one: a file that couldn't be read.
fn io_error(what: &Path, err: std::io::Error) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(format!("reading {}: {err}", what.display()).into())
}

/// The events files of feature folder `dir`, by name.
fn events_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = (entries.flatten())
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().unwrap_or_default().to_string_lossy();
            name.starts_with("events-") && name.ends_with(".jsonl")
        })
        .collect();
    files.sort();
    files
}

/// Whether the file at `path`, read up to `read`, is no longer what was
/// read: it shrank, or the byte before where reading stopped isn't the end
/// of a line.
fn replaced(path: &Path, read: Read) -> bool {
    let Some((size, _)) = stat(path) else {
        return true;
    };
    if size < read.offset {
        return true;
    }
    if read.offset == 0 {
        return false;
    }
    let mut last = [0u8];
    let at = std::fs::File::open(path).and_then(|mut f| {
        f.seek(SeekFrom::Start(read.offset as u64 - 1))?;
        f.read_exact(&mut last)
    });
    at.is_err() || last[0] != b'\n'
}

/// Take in feature folder `dir`: the number of new lines.
fn ingest_dir(tx: &Transaction, dir: &Path) -> rusqlite::Result<usize> {
    let Some((project, slug)) = names(dir) else {
        return Ok(0);
    };
    let feature = Feature { project, slug, dir };
    let f = &feature;
    let prefix = format!("{}/", dir.display());
    let files = events_files(dir);

    // A file read before that went, shrank or changed under what was read:
    // the feature is drawn again from the start.
    let mut known: Vec<(String, Read)> = Vec::new();
    {
        let mut stmt = tx.prepare(
            "SELECT path, offset, size, mtime, bad_lines FROM ingest
             WHERE substr(path, 1, length(?1)) = ?1",
        )?;
        let rows = stmt.query_map([&prefix], |r| {
            let read = Read {
                offset: r.get(1)?,
                size: r.get(2)?,
                mtime: r.get(3)?,
                bad: r.get(4)?,
            };
            Ok((r.get::<_, String>(0)?, read))
        })?;
        for row in rows {
            known.push(row?);
        }
    }
    let is_events = |p: &str| {
        let name = p.rsplit('/').next().unwrap_or_default();
        name.starts_with("events-") && name.ends_with(".jsonl")
    };
    let reset = (known.iter())
        .filter(|(p, _)| is_events(p) && !p[prefix.len()..].contains('/'))
        .any(|(p, read)| replaced(Path::new(p), *read));
    if reset {
        tracing::info!(
            "ai work: drawing {} again: an events file changed",
            dir.display()
        );
        wipe(tx, f, &prefix)?;
    }

    tx.execute(
        "INSERT OR IGNORE INTO features (project, slug, dir) VALUES (?1, ?2, ?3)",
        params![f.project, f.slug, dir.to_string_lossy()],
    )?;

    let mut lines = 0;
    for file in &files {
        let key = file.to_string_lossy();
        let was = match reset {
            true => Read::default(),
            false => stored(tx, &key)?.unwrap_or_default(),
        };
        let Some((size, mtime)) = stat(file) else {
            continue;
        };
        if was.offset == size {
            continue;
        }
        let mut buf = Vec::new();
        let opened = std::fs::File::open(file).and_then(|mut h| {
            h.seek(SeekFrom::Start(was.offset as u64))?;
            h.read_to_end(&mut buf)
        });
        opened.map_err(|e| io_error(file, e))?;
        // Up to the last whole line: the rest is still being written.
        let Some(end) = buf.iter().rposition(|b| *b == b'\n') else {
            continue;
        };
        let mut bad = 0;
        for raw in buf[..end].split(|b| *b == b'\n') {
            if raw.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            match parse(raw) {
                Parsed::Line(line) => {
                    apply(tx, f, &line)?;
                    lines += 1;
                }
                Parsed::Newer => {}
                Parsed::Bad => bad += 1,
            }
        }
        if bad > 0 {
            tracing::warn!("ai work: {bad} lines of {key} can't be read");
        }
        let read = Read {
            offset: was.offset + end as i64 + 1,
            size,
            mtime,
            bad: was.bad + bad,
        };
        store(tx, &key, read)?;
    }

    record(tx, f)?;
    sessions(tx, f)?;
    Ok(lines)
}

/// Take every row of feature `f` out, and what says how far its files were
/// read (`prefix` is its folder with a slash).
fn wipe(tx: &Transaction, f: &Feature, prefix: &str) -> rusqlite::Result<()> {
    for table in FEATURE_TABLES {
        tx.execute(
            &format!("DELETE FROM {table} WHERE project = ?1 AND slug = ?2"),
            params![f.project, f.slug],
        )?;
    }
    tx.execute(
        "DELETE FROM ingest WHERE substr(path, 1, length(?1)) = ?1",
        [prefix],
    )?;
    Ok(())
}

/// What a line of an events file came to.
enum Parsed {
    Line(Box<Line>),
    /// Written by a newer gerdr: passed over.
    Newer,
    Bad,
}

fn parse(raw: &[u8]) -> Parsed {
    let Ok(value) = serde_json::from_slice::<Value>(raw) else {
        return Parsed::Bad;
    };
    if value["v"].as_u64().is_some_and(|v| v > u64::from(FORMAT)) {
        return Parsed::Newer;
    }
    match serde_json::from_value::<Line>(value) {
        Ok(line) => Parsed::Line(Box::new(line)),
        Err(_) => Parsed::Bad,
    }
}

/// `stage` as the index keeps it: `ai-review`.
pub fn stage_name(stage: Stage) -> String {
    name_of(&stage)
}

/// The stage named `name` in the index.
pub fn stage_of(name: &str) -> Option<Stage> {
    serde_json::from_value(Value::String(name.to_owned())).ok()
}

/// A unit enum as its serde name.
fn name_of<T: serde::Serialize>(t: &T) -> String {
    match serde_json::to_value(t) {
        Ok(Value::String(s)) => s,
        Ok(other) => other.to_string(),
        Err(_) => String::new(),
    }
}

/// Unix seconds `v` in ms.
fn ms(v: &Value) -> Option<i64> {
    v.as_i64().map(|s| s * 1000)
}

/// `v` as text: a string as it is, anything else as JSON.
fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Where `blob` of feature `f` is kept, as text.
fn blob_path(f: &Feature, blob: &Option<BlobRef>) -> Option<String> {
    Some(blob.as_ref()?.path(f.dir).to_string_lossy().into_owned())
}

/// Put what `line` says of feature `f` in the tables.
fn apply(tx: &Transaction, f: &Feature, line: &Line) -> rusqlite::Result<()> {
    let at = line.at_ms as i64;
    let (p, s) = (&f.project, &f.slug);
    match &line.event {
        Event::Note { i, at: when, what } => {
            tx.execute(
                "INSERT OR REPLACE INTO notes (project, slug, i, at, what)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![p, s, *i as i64, (*when as i64) * 1000, what],
            )?;
        }
        Event::RunStarted {
            stage, n, run, how, ..
        } => {
            let held = run["held"].as_bool().unwrap_or(false);
            upsert_run(tx, f, *stage, *n, run, at, Some(&name_of(how)), None, held)?;
        }
        Event::RunChanged { stage, n, change } => {
            tx.execute(
                "INSERT INTO run_changes (project, slug, stage, n, at, change)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![p, s, stage_name(*stage), n, at, name_of(change)],
            )?;
        }
        Event::RunEnded { stage, n, why, run } => {
            let ended = Some((at, name_of(why)));
            upsert_run(tx, f, *stage, *n, run, at, None, ended, false)?;
        }
        Event::Marked { stage, mark } => {
            tx.execute(
                "INSERT OR REPLACE INTO stages (project, slug, stage, marked)
                 VALUES (?1, ?2, ?3, ?4)",
                params![p, s, stage_name(*stage), ms(&mark["at"]).unwrap_or(at)],
            )?;
        }
        Event::Unmarked { stage } => {
            tx.execute(
                "DELETE FROM stages WHERE project = ?1 AND slug = ?2 AND stage = ?3",
                params![p, s, stage_name(*stage)],
            )?;
        }
        Event::Status { to, .. } => {
            tx.execute(
                "UPDATE features SET status = ?3 WHERE project = ?1 AND slug = ?2",
                params![p, s, text(to)],
            )?;
        }
        Event::FixRoundOpened { n, round } => {
            let (items, fixes) = fixes_of(round);
            tx.execute(
                "INSERT INTO fix_rounds (project, slug, n, stage, opened, items, fixes)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT (project, slug, n) DO UPDATE SET
                    stage = coalesce(excluded.stage, stage),
                    opened = coalesce(opened, excluded.opened),
                    items = max(items, excluded.items),
                    fixes = max(fixes, excluded.fixes)",
                params![p, s, n, round["stage"].as_str(), at, items, fixes],
            )?;
        }
        Event::FixRoundClosed { n, round } => {
            let (items, fixes) = fixes_of(round);
            let closed = ms(&round["ended"]).unwrap_or(at);
            tx.execute(
                "INSERT INTO fix_rounds (project, slug, n, stage, closed, items, fixes)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT (project, slug, n) DO UPDATE SET
                    stage = coalesce(excluded.stage, stage),
                    closed = excluded.closed,
                    items = excluded.items,
                    fixes = excluded.fixes",
                params![p, s, n, round["stage"].as_str(), closed, items, fixes],
            )?;
        }
        Event::ReviewRead {
            n,
            findings,
            review,
        } => {
            tx.execute(
                "INSERT OR REPLACE INTO reviews (project, slug, kind, n, at, count)
                 VALUES (?1, ?2, 'ai', ?3, ?4, ?5)",
                params![p, s, n, at, *findings as i64],
            )?;
            if let Some(counts) = review.as_ref().and_then(|b| severities_of_blob(f, b)) {
                put_findings(tx, f, *n, &counts, true)?;
            }
        }
        Event::PlanReviewRead { n, suggestions, .. } => {
            tx.execute(
                "INSERT OR REPLACE INTO reviews (project, slug, kind, n, at, count)
                 VALUES (?1, ?2, 'plan', ?3, ?4, ?5)",
                params![p, s, n, at, *suggestions as i64],
            )?;
        }
        Event::VerifyStarted { run, verify } => {
            tx.execute(
                "INSERT INTO verify_runs (project, slug, run, started) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (project, slug, run) DO UPDATE SET started = excluded.started",
                params![p, s, run, ms(&verify["started"]).unwrap_or(at)],
            )?;
        }
        Event::CheckEnded {
            run,
            key,
            i,
            outcome,
            log,
            truncated,
        } => {
            // Each check is told twice: from the record, with no log, and
            // from the check's thread, with it. Either may come first, and
            // either may lack the outcome: what each has is kept.
            let (key_repo, key_command) = key.split_once(": ").unwrap_or((key, ""));
            let repo = outcome["repo"].as_str().unwrap_or(key_repo);
            let command = outcome["command"].as_str().unwrap_or(key_command);
            let said = outcome.as_object().is_some_and(|o| !o.is_empty());
            let passed = outcome["exit"].as_i64().map(|e| e == 0).unwrap_or(false);
            let interrupted = outcome["interrupted"].as_bool().unwrap_or(false);
            tx.execute(
                "INSERT INTO checks (project, slug, run, i, key, repo, command,
                    ended, secs, passed, interrupted, log, truncated)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                 ON CONFLICT (project, slug, run, i) DO UPDATE SET
                    key = excluded.key,
                    repo = CASE WHEN ?14 THEN excluded.repo ELSE repo END,
                    command = CASE WHEN ?14 THEN excluded.command ELSE command END,
                    secs = CASE WHEN ?14 THEN excluded.secs ELSE secs END,
                    passed = CASE WHEN ?14 THEN excluded.passed ELSE passed END,
                    interrupted = CASE WHEN ?14 THEN excluded.interrupted ELSE interrupted END,
                    ended = coalesce(min(ended, excluded.ended), ended, excluded.ended),
                    log = coalesce(excluded.log, log),
                    truncated = CASE WHEN excluded.log IS NOT NULL
                        THEN excluded.truncated ELSE truncated END",
                params![
                    p,
                    s,
                    run,
                    *i as i64,
                    key,
                    repo,
                    command,
                    at,
                    outcome["secs"].as_i64(),
                    passed,
                    interrupted,
                    blob_path(f, log),
                    truncated,
                    said
                ],
            )?;
        }
        Event::VerifyEnded { run, verify } => {
            tx.execute(
                "INSERT INTO verify_runs (project, slug, run, started, ended, passed)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (project, slug, run) DO UPDATE SET
                    started = coalesce(started, excluded.started),
                    ended = excluded.ended,
                    passed = excluded.passed",
                params![
                    p,
                    s,
                    run,
                    ms(&verify["started"]),
                    ms(&verify["finished"]).unwrap_or(at),
                    verify["passed"].as_bool()
                ],
            )?;
        }
        Event::SetupEnded { run, log } => {
            tx.execute(
                "INSERT OR REPLACE INTO setups (project, slug, started, ended, passed, log)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    p,
                    s,
                    ms(&run["started"]).unwrap_or(at),
                    ms(&run["finished"]).unwrap_or(at),
                    run["failed"].is_null(),
                    blob_path(f, log)
                ],
            )?;
        }
        Event::Manual { manual } => {
            tx.execute(
                "INSERT INTO manual (project, slug, at, sheet) VALUES (?1, ?2, ?3, ?4)",
                params![p, s, at, manual.to_string()],
            )?;
        }
        Event::Sent {
            stage,
            n,
            send,
            via,
            ..
        } => {
            tx.execute(
                "INSERT INTO sends (project, slug, stage, n, at, kind, via)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![p, s, stage_name(*stage), n, at, name_of(send), name_of(via)],
            )?;
        }
        Event::SendFailed {
            stage,
            n,
            send,
            via,
            why,
            ..
        } => {
            tx.execute(
                "INSERT INTO sends (project, slug, stage, n, at, kind, via, failed)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    p,
                    s,
                    stage_name(*stage),
                    n,
                    at,
                    name_of(send),
                    name_of(via),
                    why
                ],
            )?;
        }
        Event::Transcript {
            agent,
            session,
            runs,
            raw_bytes,
            complete,
        } => {
            tx.execute(
                "INSERT INTO sessions (project, slug, agent, id, raw_bytes, complete)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (project, slug, agent, id) DO UPDATE SET
                    raw_bytes = excluded.raw_bytes, complete = excluded.complete",
                params![p, s, agent.name(), session, *raw_bytes as i64, complete],
            )?;
            for (stage, n) in runs {
                session_run(tx, f, *agent, session, *stage, *n)?;
            }
        }
        Event::StartedOver { .. }
        | Event::Snapshot { .. }
        | Event::Output { .. }
        | Event::TranscriptMissing { .. } => {}
    }
    Ok(())
}

/// A fix round's things to fix and those made, as its record says.
fn fixes_of(round: &Value) -> (i64, i64) {
    let items = round["items"].as_array().map_or(0, Vec::len) as i64;
    let done = round["done"].as_i64().unwrap_or(0);
    (items, if items > 0 { done.min(items) } else { done })
}

/// Put run `n` of `stage` (`run`, its record) in, or what is new of it.
/// `at`: when it was seen, its start when the record has none.
#[allow(clippy::too_many_arguments)]
fn upsert_run(
    tx: &Transaction,
    f: &Feature,
    stage: Stage,
    n: u32,
    run: &Value,
    at: i64,
    how: Option<&str>,
    ended: Option<(i64, String)>,
    held: bool,
) -> rusqlite::Result<()> {
    let started = match ms(&run["started"]) {
        Some(started) => started,
        // No start said: the latest run of that number, else now.
        None => tx
            .query_row(
                "SELECT max(started) FROM runs
                 WHERE project = ?1 AND slug = ?2 AND stage = ?3 AND n = ?4",
                params![f.project, f.slug, stage_name(stage), n],
                |r| r.get::<_, Option<i64>>(0),
            )?
            .unwrap_or(at),
    };
    let model = (run["model"].as_str())
        .or(run["asked_model"].as_str())
        .unwrap_or_default();
    let (ended_at, why) = ended.unzip();
    tx.execute(
        "INSERT INTO runs (project, slug, stage, n, started, agent, model, how, ready,
            ended, why, nudged, sent_back, machine, held)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
         ON CONFLICT (project, slug, stage, n, started) DO UPDATE SET
            agent = CASE WHEN excluded.agent != '' THEN excluded.agent ELSE agent END,
            model = CASE WHEN excluded.model != '' THEN excluded.model ELSE model END,
            how = coalesce(how, excluded.how),
            ready = coalesce(excluded.ready, ready),
            ended = coalesce(excluded.ended, ended),
            why = coalesce(excluded.why, why),
            nudged = max(nudged, excluded.nudged),
            sent_back = max(sent_back, excluded.sent_back),
            machine = coalesce(excluded.machine, machine),
            held = max(held, excluded.held)",
        params![
            f.project,
            f.slug,
            stage_name(stage),
            n,
            started,
            run["agent"].as_str().unwrap_or_default(),
            model,
            how,
            ms(&run["ready"]),
            ended_at,
            why,
            run["nudged"].as_bool().unwrap_or(false),
            run["sent_back"].as_i64().unwrap_or(0),
            run["machine"].as_str(),
            held
        ],
    )?;
    Ok(())
}

fn session_run(
    tx: &Transaction,
    f: &Feature,
    agent: AgentKind,
    id: &str,
    stage: Stage,
    n: u32,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO session_runs (project, slug, agent, id, stage, n)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![f.project, f.slug, agent.name(), id, stage_name(stage), n],
    )
    .map(drop)
}

/// Findings by severity in `review`, as an AI review's file or record
/// holds them: `{"findings": [{"severity": ...}]}`. Those without a
/// severity are medium, as `review_import` reads them; the user's own
/// (`u<k>` ids) aren't the reviewer's.
fn severities(review: &Value, only_run: Option<u32>) -> BTreeMap<u32, BTreeMap<String, i64>> {
    let mut out: BTreeMap<u32, BTreeMap<String, i64>> = BTreeMap::new();
    let findings = review["findings"].as_array().into_iter().flatten();
    for finding in findings {
        let id = finding["id"].as_str().unwrap_or_default();
        if id.starts_with('u') || finding["yours"].as_bool() == Some(true) {
            continue;
        }
        let run = match only_run {
            Some(run) => run,
            // `r<run>-<k>`: raised first by review `run`.
            None => match (id.strip_prefix('r'))
                .and_then(|rest| rest.split('-').next())
                .and_then(|n| n.parse().ok())
            {
                Some(run) => run,
                None => continue,
            },
        };
        let severity = (finding["severity"].as_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_else(|| "medium".to_owned());
        *out.entry(run).or_default().entry(severity).or_default() += 1;
    }
    out
}

/// The findings by severity in kept blob `blob` of feature `f`, when it
/// can be read.
fn severities_of_blob(f: &Feature, blob: &BlobRef) -> Option<BTreeMap<String, i64>> {
    let packed = std::fs::read(blob.path(f.dir)).ok()?;
    let raw = zst::decompress(&packed)
        .inspect_err(|err| tracing::warn!("ai work: reading review {}: {err}", blob.sha))
        .ok()?;
    let review: Value = serde_json::from_slice(&raw).ok()?;
    review["findings"].is_array().then_some(())?;
    Some(severities(&review, Some(0)).remove(&0).unwrap_or_default())
}

/// Put review `review`'s findings `counts` of feature `f` in, from its
/// blob, or from the record unless its blob's are in.
fn put_findings(
    tx: &Transaction,
    f: &Feature,
    review: u32,
    counts: &BTreeMap<String, i64>,
    from_blob: bool,
) -> rusqlite::Result<()> {
    let (p, s) = (&f.project, &f.slug);
    if !from_blob {
        let blob: i64 = tx.query_row(
            "SELECT count(*) FROM findings
             WHERE project = ?1 AND slug = ?2 AND review = ?3 AND from_blob = 1",
            params![p, s, review],
            |r| r.get(0),
        )?;
        if blob > 0 {
            return Ok(());
        }
    }
    tx.execute(
        "DELETE FROM findings WHERE project = ?1 AND slug = ?2 AND review = ?3",
        params![p, s, review],
    )?;
    for (severity, count) in counts {
        tx.execute(
            "INSERT INTO findings (project, slug, review, severity, count, from_blob)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![p, s, review, severity, count, from_blob],
        )?;
    }
    Ok(())
}

/// Take in feature `f`'s `record.json` when it changed: its title, status,
/// age, and its AI review's findings for reviews whose own file wasn't kept.
fn record(tx: &Transaction, f: &Feature) -> rusqlite::Result<()> {
    let file = f.dir.join("record.json");
    let key = file.to_string_lossy();
    let Some((size, mtime)) = stat(&file) else {
        return Ok(());
    };
    let was = stored(tx, &key)?;
    if was.is_some_and(|w| w.size == size && w.mtime == mtime) {
        return Ok(());
    }
    let raw = std::fs::read(&file).map_err(|e| io_error(&file, e))?;
    let mut read = Read {
        offset: size,
        size,
        mtime,
        bad: 0,
    };
    match serde_json::from_slice::<Value>(&raw) {
        Ok(rec) => {
            tx.execute(
                "UPDATE features SET
                    title = coalesce(?3, title),
                    status = coalesce(?4, status),
                    created = coalesce(?5, created)
                 WHERE project = ?1 AND slug = ?2",
                params![
                    f.project,
                    f.slug,
                    rec["title"].as_str(),
                    rec["status"].as_str().or(Some("open")),
                    ms(&rec["created"])
                ],
            )?;
            for (review, counts) in severities(&rec["review"], None) {
                put_findings(tx, f, review, &counts, false)?;
            }
        }
        Err(err) => {
            tracing::warn!("ai work: reading {key}: {err}");
            read.bad = 1;
        }
    }
    store(tx, &key, read)
}

/// The runs `(stage, n)` a kept chat's `meta.json` says it was of:
/// `[["build", 1]]` or `[{"stage": "build", "n": 1}]`.
fn meta_runs(meta: &Value) -> Vec<(Stage, u32)> {
    let runs = meta["runs"].as_array().into_iter().flatten();
    (runs.filter_map(|r| {
        let (stage, n) = match r {
            Value::Array(pair) => (pair.first()?, pair.get(1)?),
            Value::Object(_) => (&r["stage"], &r["n"]),
            _ => return None,
        };
        let stage = serde_json::from_value::<Stage>(stage.clone()).ok()?;
        Some((stage, u32::try_from(n.as_u64()?).ok()?))
    }))
    .collect()
}

/// Take in feature `f`'s kept chats, `sessions/<agent>-<id>/`, whose
/// transcript or runs changed since: what each run of them cost.
fn sessions(tx: &Transaction, f: &Feature) -> rusqlite::Result<()> {
    let Ok(entries) = std::fs::read_dir(f.dir.join("sessions")) else {
        return Ok(());
    };
    let mut folders: Vec<PathBuf> = (entries.flatten())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    folders.sort();
    for folder in folders {
        session(tx, f, &folder)?;
    }
    Ok(())
}

fn session(tx: &Transaction, f: &Feature, folder: &Path) -> rusqlite::Result<()> {
    let (p, s) = (&f.project, &f.slug);
    let name = folder.file_name().unwrap_or_default().to_string_lossy();
    let meta: Value = std::fs::read(folder.join("meta.json"))
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default();
    let (named_agent, named_id) = name.split_once('-').unwrap_or((&name, ""));
    let agent = (serde_json::from_value::<AgentKind>(meta["agent"].clone()).ok())
        .or_else(|| serde_json::from_value(Value::String(named_agent.to_owned())).ok())
        .unwrap_or(AgentKind::Unknown);
    let id = meta["id"].as_str().unwrap_or(named_id).to_owned();
    if id.is_empty() {
        return Ok(());
    }
    let transcript = ["transcript.jsonl.zst", "store.db.zst"]
        .map(|file| folder.join(file))
        .into_iter()
        .find(|path| path.is_file());
    let Some(transcript) = transcript else {
        return Ok(());
    };
    let Some((size, mtime)) = stat(&transcript) else {
        return Ok(());
    };
    tx.execute(
        "INSERT INTO sessions (project, slug, agent, id, raw_bytes, complete, transcript)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (project, slug, agent, id) DO UPDATE SET
            transcript = excluded.transcript,
            raw_bytes = max(raw_bytes, excluded.raw_bytes),
            complete = max(complete, excluded.complete)",
        params![
            p,
            s,
            agent.name(),
            id,
            meta["raw_len"].as_i64().unwrap_or(0),
            meta["complete"].as_bool().unwrap_or(false),
            transcript.to_string_lossy()
        ],
    )?;
    for (stage, n) in meta_runs(&meta) {
        session_run(tx, f, agent, &id, stage, n)?;
    }

    // The runs it was of, each by its start.
    let mut runs: Vec<(String, u32, i64)> = Vec::new();
    {
        let mut stmt = tx.prepare(
            "SELECT r.stage, r.n, r.started FROM runs r
             JOIN session_runs sr ON sr.project = r.project AND sr.slug = r.slug
                AND sr.stage = r.stage AND sr.n = r.n
             WHERE sr.project = ?1 AND sr.slug = ?2 AND sr.agent = ?3 AND sr.id = ?4
             ORDER BY r.started, r.stage, r.n",
        )?;
        let rows = stmt.query_map(params![p, s, agent.name(), id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
        for row in rows {
            runs.push(row?);
        }
    }
    let split = format!("{size}:{mtime}:{runs:?}");
    let was: Option<String> = tx.query_row(
        "SELECT split FROM sessions WHERE project = ?1 AND slug = ?2 AND agent = ?3 AND id = ?4",
        params![p, s, agent.name(), id],
        |r| r.get(0),
    )?;
    if was.as_deref() == Some(split.as_str()) {
        return Ok(());
    }
    tx.execute(
        "DELETE FROM usage WHERE project = ?1 AND slug = ?2 AND agent = ?3 AND id = ?4",
        params![p, s, agent.name(), id],
    )?;
    let raw = std::fs::read(&transcript).and_then(|packed| zst::decompress(&packed));
    match raw {
        Ok(raw) if !runs.is_empty() => {
            let messages = usage::read(agent, &raw);
            let starts: Vec<u64> = runs.iter().map(|r| r.2.max(0) as u64).collect();
            for ((i, model, sidechain), u) in usage::split_at(&messages, &starts) {
                let (stage, n, started) = &runs[i];
                tx.execute(
                    "INSERT INTO usage (project, slug, agent, id, stage, n, started, model,
                        sidechain, messages, tool_calls, input, output, cache_create, cache_read)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                    params![
                        p,
                        s,
                        agent.name(),
                        id,
                        stage,
                        n,
                        started,
                        model,
                        sidechain,
                        u.messages as i64,
                        u.tool_calls as i64,
                        u.input as i64,
                        u.output as i64,
                        u.cache_create as i64,
                        u.cache_read as i64
                    ],
                )?;
            }
        }
        Ok(_) => {}
        // Tried again once the file changes.
        Err(err) => tracing::warn!("ai work: reading {}: {err}", transcript.display()),
    }
    tx.execute(
        "UPDATE sessions SET split = ?5
         WHERE project = ?1 AND slug = ?2 AND agent = ?3 AND id = ?4",
        params![p, s, agent.name(), id, split],
    )?;
    Ok(())
}

/// How many lines of the events files under `conn` couldn't be read.
pub fn bad_lines(conn: &Connection) -> Result<i64, String> {
    conn.query_row("SELECT coalesce(sum(bad_lines), 0) FROM ingest", [], |r| {
        r.get(0)
    })
    .map_err(|e| e.to_string())
}

/// Feature folders written as the other parts of AI work write them, for
/// the index's tests and the stats'.
#[cfg(test)]
pub mod testing {
    use std::{io::Write as _, path::PathBuf};

    use super::super::{Event, FORMAT, Line};

    /// A new, empty folder for test `name`, outside any repository.
    pub fn root(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("gerdr-ai-index-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// `event` at unix second `at`.
    pub fn line(at: u64, event: Event) -> Line {
        Line {
            v: FORMAT,
            at_ms: at * 1000,
            machine: "here".into(),
            backfill: false,
            event,
        }
    }

    /// Append `lines` to `events-<machine>.jsonl` of feature folder `dir`.
    pub fn append(dir: &std::path::Path, machine: &str, lines: &[Line]) {
        std::fs::create_dir_all(dir).unwrap();
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(format!("events-{machine}.jsonl")))
            .unwrap();
        for line in lines {
            writeln!(file, "{}", serde_json::to_string(line).unwrap()).unwrap();
        }
    }

    /// Append raw `text` to `events-<machine>.jsonl` of `dir`.
    pub fn append_raw(dir: &std::path::Path, machine: &str, text: &str) {
        std::fs::create_dir_all(dir).unwrap();
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(format!("events-{machine}.jsonl")))
            .unwrap();
        file.write_all(text.as_bytes()).unwrap();
    }

    /// Keep chat `agent`-`id` in `dir`, of `runs`, its transcript `raw`.
    pub fn chat(dir: &std::path::Path, agent: &str, id: &str, runs: &str, raw: &str) {
        let folder = dir.join("sessions").join(format!("{agent}-{id}"));
        std::fs::create_dir_all(&folder).unwrap();
        let meta = format!(
            r#"{{"agent":"{agent}","id":"{id}","raw_len":{},"complete":true,"runs":{runs}}}"#,
            raw.len()
        );
        std::fs::write(folder.join("meta.json"), meta).unwrap();
        let packed = super::zst::compress(raw.as_bytes(), 3).unwrap();
        std::fs::write(folder.join("transcript.jsonl.zst"), packed).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{testing::*, *};
    use crate::ai_work::{EndWhy, RunChange, StartHow};

    fn count(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    fn note(i: usize, at: u64) -> Line {
        line(
            at,
            Event::Note {
                i,
                at,
                what: format!("note {i}"),
            },
        )
    }

    #[test]
    fn only_new_whole_lines_are_taken_in() {
        let root = root("offsets");
        let dir = root.join("p").join("a");
        append(&dir, "here", &[note(0, 10), note(1, 11)]);
        let mut conn = open(&root).unwrap();
        assert_eq!(ingest(&mut conn, std::slice::from_ref(&dir)), Ok(2));
        assert_eq!(ingest(&mut conn, std::slice::from_ref(&dir)), Ok(0));

        // A line still being written waits for its end.
        append(&dir, "here", &[note(2, 12)]);
        let half = serde_json::to_string(&note(3, 13)).unwrap();
        append_raw(&dir, "here", &half[..10]);
        assert_eq!(ingest(&mut conn, std::slice::from_ref(&dir)), Ok(1));
        append_raw(&dir, "here", &format!("{}\n", &half[10..]));
        // Another machine's file, read on its own.
        append(&dir, "m2", &[note(4, 14)]);
        assert_eq!(ingest(&mut conn, std::slice::from_ref(&dir)), Ok(2));
        assert_eq!(count(&conn, "notes"), 5);
        let at: i64 = conn
            .query_row("SELECT at FROM notes WHERE i = 3", [], |r| r.get(0))
            .unwrap();
        assert_eq!(at, 13_000);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn bad_lines_are_counted_and_passed_over() {
        let root = root("bad");
        let dir = root.join("p").join("a");
        append(&dir, "here", &[note(0, 10)]);
        append_raw(&dir, "here", "garbage\n");
        append_raw(
            &dir,
            "here",
            "{\"v\":1,\"at_ms\":1,\"machine\":\"here\",\"kind\":\"nope\"}\n",
        );
        // A newer gerdr's line: not bad, just not ours to read.
        append_raw(
            &dir,
            "here",
            "{\"v\":99,\"at_ms\":1,\"machine\":\"here\",\"kind\":\"nope\"}\n",
        );
        append(&dir, "here", &[note(1, 11)]);
        let mut conn = open(&root).unwrap();
        assert_eq!(ingest(&mut conn, std::slice::from_ref(&dir)), Ok(2));
        assert_eq!(bad_lines(&conn), Ok(2));
        // Counted once, not again on the next read.
        append(&dir, "here", &[note(2, 12)]);
        assert_eq!(ingest(&mut conn, std::slice::from_ref(&dir)), Ok(1));
        assert_eq!(bad_lines(&conn), Ok(2));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_file_put_in_place_of_another_is_read_again() {
        let root = root("replaced");
        let dir = root.join("p").join("a");
        append(&dir, "here", &[note(0, 10), note(1, 11), note(2, 12)]);
        let mut conn = open(&root).unwrap();
        assert_eq!(ingest(&mut conn, std::slice::from_ref(&dir)), Ok(3));

        // Shorter: what it had goes, and it is read from its start.
        std::fs::remove_file(dir.join("events-here.jsonl")).unwrap();
        append(&dir, "here", &[note(5, 50)]);
        assert_eq!(ingest(&mut conn, std::slice::from_ref(&dir)), Ok(1));
        assert_eq!(count(&conn, "notes"), 1);

        // As long, but other lines: read again too.
        std::fs::remove_file(dir.join("events-here.jsonl")).unwrap();
        append(&dir, "here", &[note(6, 60), note(7, 70)]);
        let _ = ingest(&mut conn, std::slice::from_ref(&dir)).unwrap();
        let before = std::fs::metadata(dir.join("events-here.jsonl"))
            .unwrap()
            .len();
        std::fs::remove_file(dir.join("events-here.jsonl")).unwrap();
        let text = format!(
            "{}\n{}\n",
            serde_json::to_string(&note(8, 81)).unwrap(),
            serde_json::to_string(&note(9, 91)).unwrap()
        );
        // The same length, so that only what lies where reading stopped
        // tells: "...}\n" with the newline elsewhere.
        let padded = format!(
            "{text}{}",
            " ".repeat((before as usize).saturating_sub(text.len()))
        );
        append_raw(&dir, "here", padded.trim_end());
        append_raw(&dir, "here", " ");
        assert_eq!(ingest(&mut conn, std::slice::from_ref(&dir)), Ok(1));
        let is: Vec<i64> = {
            let mut stmt = conn.prepare("SELECT i FROM notes ORDER BY i").unwrap();
            stmt.query_map([], |r| r.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        assert_eq!(is, vec![8]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_index_of_another_version_is_drawn_again() {
        let root = root("version");
        let dir = root.join("p").join("a");
        append(&dir, "here", &[note(0, 10)]);
        assert!(open_read(&root).is_err());
        let mut conn = open(&root).unwrap();
        assert_eq!(ingest(&mut conn, std::slice::from_ref(&dir)), Ok(1));
        assert!(open_read(&root).is_ok());
        conn.pragma_update(None, "user_version", 5).unwrap();
        drop(conn);
        assert!(open_read(&root).is_err());

        let conn = open(&root).unwrap();
        let found: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(found, version());
        assert_eq!(count(&conn, "notes"), 0);
        assert_eq!(count(&conn, "ingest"), 0);
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        drop(conn);

        // Not a database at all: made again too.
        remove(&path(&root)).unwrap();
        std::fs::write(path(&root), "not a database, not at all, no").unwrap();
        let mut conn = open(&root).unwrap();
        assert_eq!(ingest(&mut conn, std::slice::from_ref(&dir)), Ok(1));
        let mode = std::fs::metadata(path(&root)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Only an index that is none any more is drawn again: one another
    /// gerdr holds stays.
    #[test]
    fn only_a_broken_index_is_made_again() {
        let code = |c| rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(c), None);
        assert!(redo(&code(rusqlite::ffi::SQLITE_NOTADB)));
        assert!(redo(&code(rusqlite::ffi::SQLITE_CORRUPT)));
        assert!(!redo(&code(rusqlite::ffi::SQLITE_BUSY)));
        assert!(!redo(&code(rusqlite::ffi::SQLITE_IOERR)));
        assert!(matches!(
            Unopened::of(&code(rusqlite::ffi::SQLITE_BUSY)),
            Unopened::Failed(_)
        ));
    }

    #[test]
    fn a_reset_index_takes_everything_in_again() {
        let root = root("reset");
        let dir = root.join("p").join("a");
        append(&dir, "here", &[note(0, 10)]);
        let mut conn = open(&root).unwrap();
        assert_eq!(ingest(&mut conn, std::slice::from_ref(&dir)), Ok(1));
        assert_eq!(ingest(&mut conn, std::slice::from_ref(&dir)), Ok(0));
        drop(conn);
        let mut conn = reset(&root).unwrap();
        assert_eq!(count(&conn, "notes"), 0);
        assert_eq!(ingest(&mut conn, std::slice::from_ref(&dir)), Ok(1));
        assert_eq!(count(&conn, "notes"), 1);
        drop(conn);
        // Not a database: made anew.
        remove(&path(&root)).unwrap();
        std::fs::write(path(&root), "not a database, not at all, no").unwrap();
        let mut conn = reset(&root).unwrap();
        assert_eq!(ingest(&mut conn, std::slice::from_ref(&dir)), Ok(1));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn record_and_runs_fill_their_tables() {
        let root = root("record");
        let dir = root.join("p").join("a");
        let run = json!({"n": 1, "agent": "Claude Code", "asked_model": "opus", "started": 100,
            "prompt": "/x", "template": "t", "command": "c", "held": true});
        let ended = json!({"n": 1, "agent": "Claude Code", "model": "claude-opus-5-5",
            "started": 100, "ready": 150, "nudged": true, "sent_back": 2,
            "prompt": "/x", "template": "t", "command": "c"});
        append(
            &dir,
            "here",
            &[
                line(
                    100,
                    Event::RunStarted {
                        stage: Stage::Plan,
                        n: 1,
                        run,
                        prompt: None,
                        how: StartHow::Fresh,
                    },
                ),
                line(
                    120,
                    Event::RunChanged {
                        stage: Stage::Plan,
                        n: 1,
                        change: RunChange::Nudged,
                    },
                ),
                line(
                    200,
                    Event::RunEnded {
                        stage: Stage::Plan,
                        n: 1,
                        why: EndWhy::Replaced,
                        run: ended,
                    },
                ),
                line(
                    201,
                    Event::Status {
                        from: json!("open"),
                        to: json!("done"),
                    },
                ),
            ],
        );
        std::fs::write(
            dir.join("record.json"),
            json!({"title": "Alpha", "status": "abandoned", "created": 90,
                "review": {"n": 2, "findings": [
                    {"id": "r1-1", "severity": "high"}, {"id": "r1-2", "severity": "low"},
                    {"id": "r2-1", "severity": "low"}, {"id": "u1", "severity": "high"}]}})
            .to_string(),
        )
        .unwrap();
        let mut conn = open(&root).unwrap();
        assert_eq!(ingest(&mut conn, std::slice::from_ref(&dir)), Ok(4));
        let row: (
            String,
            String,
            i64,
            String,
            Option<i64>,
            i64,
            String,
            bool,
            i64,
            bool,
        ) = conn
            .query_row(
                "SELECT agent, model, started, how, ready, ended, why, nudged, sent_back, held
                 FROM runs",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                        r.get(8)?,
                        r.get(9)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            row,
            (
                "Claude Code".into(),
                "claude-opus-5-5".into(),
                100_000,
                "fresh".into(),
                Some(150_000),
                200_000,
                "replaced".into(),
                true,
                2,
                true
            )
        );
        let feature: (String, String, i64) = conn
            .query_row("SELECT title, status, created FROM features", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        // The record is the latest word.
        assert_eq!(feature, ("Alpha".into(), "abandoned".into(), 90_000));
        let findings: Vec<(i64, String, i64)> = {
            let mut stmt = conn
                .prepare("SELECT review, severity, count FROM findings ORDER BY review, severity")
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        assert_eq!(
            findings,
            vec![
                (1, "high".into(), 1),
                (1, "low".into(), 1),
                (2, "low".into(), 1)
            ]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_chat_counts_each_claude_message_once_split_by_run() {
        let root = root("usage");
        let dir = root.join("p").join("a");
        let run = |n: u32, started: u64| {
            json!({"n": n, "agent": "Claude Code", "started": started, "prompt": "/x",
                "template": "t", "command": "c"})
        };
        append(
            &dir,
            "here",
            &[
                line(
                    1000,
                    Event::RunStarted {
                        stage: Stage::Build,
                        n: 1,
                        run: run(1, 1000),
                        prompt: None,
                        how: StartHow::Fresh,
                    },
                ),
                line(
                    2000,
                    Event::RunStarted {
                        stage: Stage::Build,
                        n: 2,
                        run: run(2, 2000),
                        prompt: None,
                        how: StartHow::Reused,
                    },
                ),
                line(
                    2500,
                    Event::Transcript {
                        agent: AgentKind::Claude,
                        session: "s1".into(),
                        runs: vec![(Stage::Build, 2)],
                        raw_bytes: 10,
                        complete: true,
                    },
                ),
            ],
        );
        let msg = |id: &str, at: &str, out: u64, tool: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"{at}","message":{{"id":"{id}","model":"claude-opus-5-5","usage":{{"input_tokens":3,"cache_creation_input_tokens":10,"cache_read_input_tokens":100,"output_tokens":{out}}},"content":[{{"type":"tool_use","id":"{tool}"}}]}}}}"#
            )
        };
        // 1000 s is 1970-01-01T00:16:40Z; 2000 s is 00:33:20.
        let raw = [
            msg("m1", "1970-01-01T00:17:00Z", 20, "t1"),
            msg("m1", "1970-01-01T00:17:00.5Z", 20, "t2"),
            msg("m1", "1970-01-01T00:17:01Z", 20, "t3"),
            msg("m2", "1970-01-01T00:34:00Z", 7, "t4"),
            msg("m2", "1970-01-01T00:34:01Z", 7, "t4"),
        ]
        .join("\n");
        // The meta names run 1; the event named run 2.
        chat(&dir, "claude", "s1", r#"[["build", 1]]"#, &raw);
        let mut conn = open(&root).unwrap();
        ingest(&mut conn, std::slice::from_ref(&dir)).unwrap();
        let rows: Vec<(i64, i64, i64, i64, i64, i64)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT n, messages, tool_calls, input, cache_read, output FROM usage
                     ORDER BY n",
                )
                .unwrap();
            stmt.query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect()
        };
        assert_eq!(rows, vec![(1, 1, 3, 3, 100, 20), (2, 1, 1, 3, 100, 7)]);
        let path: String = conn
            .query_row("SELECT transcript FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert!(path.ends_with("sessions/claude-s1/transcript.jsonl.zst"));

        // Unchanged: not read again; a new run of it: split again.
        assert_eq!(ingest(&mut conn, std::slice::from_ref(&dir)), Ok(0));
        append(
            &dir,
            "here",
            &[
                line(
                    2100,
                    Event::RunStarted {
                        stage: Stage::Build,
                        n: 3,
                        run: run(3, 2030),
                        prompt: None,
                        how: StartHow::Reused,
                    },
                ),
                line(
                    2600,
                    Event::Transcript {
                        agent: AgentKind::Claude,
                        session: "s1".into(),
                        runs: vec![(Stage::Build, 3)],
                        raw_bytes: 10,
                        complete: true,
                    },
                ),
            ],
        );
        ingest(&mut conn, std::slice::from_ref(&dir)).unwrap();
        let n3: i64 = conn
            .query_row("SELECT sum(output) FROM usage WHERE n = 3", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(n3, 7);
        assert_eq!(count(&conn, "usage"), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_checks_two_lines_make_one_row() {
        let root = root("checks");
        let dir = root.join("p").join("a");
        let check = |at: u64, i: usize, outcome: serde_json::Value, log: Option<&str>| {
            line(
                at,
                Event::CheckEnded {
                    run: "v1@100".into(),
                    key: "api: cargo test".into(),
                    i,
                    outcome,
                    log: log.map(|l| BlobRef::of(l.as_bytes())),
                    truncated: log.is_some(),
                },
            )
        };
        let outcome = json!({"repo": "api", "command": "cargo test", "exit": 1, "secs": 30});
        append(
            &dir,
            "here",
            &[
                // The record's first, then the thread's with no outcome.
                check(110, 0, outcome.clone(), None),
                check(105, 0, json!(null), Some("log 0")),
                // The thread's first, then the record's.
                check(120, 1, outcome, Some("log 1")),
                check(125, 1, json!({"exit": 0, "secs": 5}), None),
            ],
        );
        let mut conn = open(&root).unwrap();
        ingest(&mut conn, std::slice::from_ref(&dir)).unwrap();
        type Row = (String, String, i64, Option<i64>, bool, Option<String>, bool);
        let rows: Vec<Row> = {
            let mut stmt = conn
                .prepare(
                    "SELECT repo, command, ended, secs, passed, log, truncated FROM checks
                     ORDER BY i",
                )
                .unwrap();
            stmt.query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect()
        };
        let log = |l: &str| {
            Some(
                BlobRef::of(l.as_bytes())
                    .path(&dir)
                    .to_string_lossy()
                    .into_owned(),
            )
        };
        assert_eq!(
            rows,
            vec![
                (
                    "api".into(),
                    "cargo test".into(),
                    105_000,
                    Some(30),
                    false,
                    log("log 0"),
                    true
                ),
                // Without a repo the key names it.
                (
                    "api".into(),
                    "cargo test".into(),
                    120_000,
                    Some(5),
                    true,
                    log("log 1"),
                    true
                ),
            ]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn feature_folders_are_found_by_project() {
        let root = root("dirs");
        for d in ["p/a", "p/b", "q/c", ".staging/x/y"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join("index.sqlite"), "").unwrap();
        let found: Vec<PathBuf> = dirs(&root)
            .into_iter()
            .map(|d| d.strip_prefix(&root).unwrap().to_owned())
            .collect();
        assert_eq!(found, ["p/a", "p/b", "q/c"].map(PathBuf::from).to_vec());
        let _ = std::fs::remove_dir_all(&root);
    }
}

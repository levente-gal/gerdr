//! What the inbox keeps of a project: its todos, and how its items were
//! triaged (dismissed, snoozed), in `~/.local/share/gerdr/inbox/<id>.json`.
//! Written atomically (see `projects::write_atomic`); a file that can't be
//! read reads as empty, and is set aside.
//!
//! An item is hidden while its triage's fingerprint is the item's current
//! one and its snooze (if any) hasn't run out: a change to the item (an
//! agent's new state, a new commit) brings it back. Entries not seen for
//! `PRUNE_AFTER` are forgotten.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::ErrorKind,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// The file's format version.
pub const VERSION: u32 = 1;

/// How long a triage entry whose item is gone is kept, in seconds.
pub const PRUNE_AFTER: u64 = 30 * 24 * 3600;

/// A todo's id within its project: `t1`, `t2`…
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TodoId(pub String);

/// A todo written in gerdr.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Todo {
    pub id: TodoId,
    pub text: String,
    /// The workspace it is about, by its record's id (`Workspace::record`,
    /// `projects::WorkspaceRecord::id`); `None`: the whole project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// When it was written, in seconds since the epoch.
    pub created: u64,
    /// When it was marked done; done todos aren't listed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done: Option<u64>,
}

/// How an item was triaged, by its id.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Triage {
    /// The item's fingerprint when it was dismissed or snoozed.
    pub fingerprint: String,
    /// Snoozed until then, in seconds since the epoch; `None`: dismissed
    /// until it changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<u64>,
    /// When the item was last listed, for pruning.
    pub seen: u64,
}

/// A project's inbox file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct InboxFile {
    pub version: u32,
    /// The number the next todo's id takes.
    pub next_todo: u64,
    pub todos: Vec<Todo>,
    /// By item id (`crate::inbox::ItemId`).
    pub triage: BTreeMap<String, Triage>,
}

impl Default for InboxFile {
    fn default() -> Self {
        InboxFile {
            version: VERSION,
            next_todo: 1,
            todos: Vec::new(),
            triage: BTreeMap::new(),
        }
    }
}

/// Project `project`'s file under `root` (`None`: gerdr's data folder,
/// `projects::data_dir`).
pub fn file(root: Option<&Path>, project: &str) -> PathBuf {
    let root = root.map_or_else(crate::projects::data_dir, Path::to_path_buf);
    root.join("inbox").join(format!("{project}.json"))
}

/// Inbox files last loaded as empty though they are still there, unread:
/// saving would lose them, so they aren't saved over until a load reads them.
static FROZEN: std::sync::Mutex<BTreeSet<PathBuf>> = std::sync::Mutex::new(BTreeSet::new());

/// Project `project`'s inbox, as saved under `root`; empty when there is
/// none or it can't be read. One that can't be parsed is set aside as
/// `<id>.json.bad-<unix time>` (as `projects` does); one that can't be read,
/// or set aside, isn't saved over.
pub fn load(root: Option<&Path>, project: &str) -> InboxFile {
    let path = file(root, project);
    let read = match std::fs::read(&path) {
        Ok(raw) => serde_json::from_slice(&raw).map_err(|err| {
            tracing::warn!("reading {}: {err}", path.display());
            crate::projects::set_aside(&path)
        }),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(InboxFile::default()),
        Err(err) => {
            tracing::warn!("reading {}: {err}", path.display());
            Err(false)
        }
    };
    let mut frozen = FROZEN.lock().unwrap_or_else(|e| e.into_inner());
    match read {
        Ok(inbox) => {
            frozen.remove(&path);
            inbox
        }
        Err(set_aside) => {
            if !set_aside {
                frozen.insert(path);
            }
            InboxFile::default()
        }
    }
}

/// Save project `project`'s inbox under `root`, atomically.
pub fn save(root: Option<&Path>, project: &str, inbox: &InboxFile) -> std::io::Result<()> {
    let path = file(root, project);
    if FROZEN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&path)
    {
        return Err(std::io::Error::other(format!(
            "not saving over {}: it couldn't be read",
            path.display()
        )));
    }
    let bytes = serde_json::to_vec_pretty(inbox).map_err(std::io::Error::other)?;
    crate::projects::write_atomic(&path, &bytes)
}

/// The hour "tomorrow morning" starts at, local time.
const MORNING: i32 = 9;

/// Before this hour the night isn't over: "tomorrow morning" is this
/// morning.
const NIGHT_ENDS: i32 = 5;

/// 09:00 local time on the day after `now` (seconds since the epoch): when
/// "snooze until tomorrow" runs out. Before 05:00 it is the same day's
/// 09:00, as the user hasn't slept yet. A day later if the local time
/// can't be worked out.
pub fn tomorrow_morning(now: u64) -> u64 {
    let fallback = now + 24 * 3600;
    let Ok(time) = libc::time_t::try_from(now) else {
        return fallback;
    };
    // SAFETY: `localtime_r` only writes the `tm` it is given (zeroed, a
    // valid `tm`) and reads `time`; `mktime` only reads and normalises
    // the `tm` it is given. Both are thread-safe, unlike `localtime`.
    let at = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&time, &mut tm).is_null() {
            return fallback;
        }
        let mut morning = morning_after(tm);
        libc::mktime(&mut morning)
    };
    match u64::try_from(at) {
        Ok(at) if at > now => at,
        _ => fallback,
    }
}

/// The broken-down local time of the morning after `tm`, for `mktime` to
/// normalise (a day past the month's end rolls over) and to work out
/// daylight saving time for (`tm_isdst` -1).
fn morning_after(mut tm: libc::tm) -> libc::tm {
    if tm.tm_hour >= NIGHT_ENDS {
        tm.tm_mday += 1;
    }
    tm.tm_hour = MORNING;
    tm.tm_min = 0;
    tm.tm_sec = 0;
    tm.tm_isdst = -1;
    tm
}

/// Forget the triage entries not seen for `PRUNE_AFTER` before `now`, and
/// the todos done that long ago.
pub fn prune(inbox: &mut InboxFile, now: u64) {
    let fresh = |at: u64| now.saturating_sub(at) <= PRUNE_AFTER;
    inbox.triage.retain(|_, triage| fresh(triage.seen));
    inbox.todos.retain(|todo| todo.done.is_none_or(fresh));
}

/// The current time, in seconds since the epoch.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of its own for a test, empty.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gerdr-inbox-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    fn todo(n: u64, done: Option<u64>) -> Todo {
        Todo {
            id: TodoId(format!("t{n}")),
            text: format!("todo {n}"),
            workspace: n.is_multiple_of(2).then(|| "w1".to_owned()),
            created: 100 + n,
            done,
        }
    }

    fn triage(seen: u64) -> Triage {
        Triage {
            fingerprint: "f".to_owned(),
            until: None,
            seen,
        }
    }

    #[test]
    fn a_saved_inbox_loads_back() {
        let root = scratch("roundtrip");
        assert_eq!(load(Some(&root), "api"), InboxFile::default());
        let mut inbox = InboxFile {
            next_todo: 3,
            todos: vec![todo(1, None), todo(2, Some(500))],
            ..InboxFile::default()
        };
        inbox.triage.insert("agent:a".to_owned(), triage(7));
        inbox.triage.insert(
            "git:/x".to_owned(),
            Triage {
                until: Some(99),
                ..triage(8)
            },
        );
        save(Some(&root), "api", &inbox).unwrap();
        assert_eq!(names(&root.join("inbox")), ["api.json"]);
        assert_eq!(load(Some(&root), "api"), inbox);
        // Other projects' are their own.
        assert_eq!(load(Some(&root), "web"), InboxFile::default());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unreadable_inbox_reads_as_empty_and_is_set_aside() {
        let root = scratch("corrupt");
        let path = file(Some(&root), "api");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{ not json").unwrap();
        assert_eq!(load(Some(&root), "api"), InboxFile::default());
        let left = names(&root.join("inbox"));
        assert_eq!(left.len(), 1);
        assert!(left[0].starts_with("api.json.bad-"), "{left:?}");
        assert_eq!(
            std::fs::read(root.join("inbox").join(&left[0])).unwrap(),
            b"{ not json"
        );
        // Saving then writes a fresh one beside it.
        save(Some(&root), "api", &InboxFile::default()).unwrap();
        assert_eq!(names(&root.join("inbox")).len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_fields_take_their_defaults() {
        let inbox: InboxFile =
            serde_json::from_str(r#"{"todos":[{"id":"t4","text":"x","created":1}]}"#).unwrap();
        assert_eq!(inbox.version, VERSION);
        assert_eq!(inbox.next_todo, 1);
        assert_eq!(inbox.todos[0].workspace, None);
        assert_eq!(inbox.todos[0].done, None);
        assert!(inbox.triage.is_empty());
    }

    #[test]
    fn pruning_forgets_only_what_is_stale() {
        let now = 100 * 24 * 3600;
        let old = now - PRUNE_AFTER - 1;
        let mut inbox = InboxFile {
            todos: vec![todo(1, None), todo(2, Some(old)), todo(3, Some(now - 60))],
            ..InboxFile::default()
        };
        inbox.triage.insert("old".to_owned(), triage(old));
        inbox
            .triage
            .insert("edge".to_owned(), triage(now - PRUNE_AFTER));
        inbox.triage.insert("fresh".to_owned(), triage(now));
        // Seen "in the future" (a clock set back) isn't stale.
        inbox.triage.insert("ahead".to_owned(), triage(now + 60));
        prune(&mut inbox, now);
        let kept: Vec<&str> = inbox.triage.keys().map(String::as_str).collect();
        assert_eq!(kept, ["ahead", "edge", "fresh"]);
        let todos: Vec<&str> = inbox.todos.iter().map(|t| t.id.0.as_str()).collect();
        assert_eq!(todos, ["t1", "t3"]);
    }

    fn tm(mday: i32, hour: i32, min: i32) -> libc::tm {
        // SAFETY: a zeroed `tm` is a valid one.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        tm.tm_year = 126;
        tm.tm_mon = 9;
        tm.tm_mday = mday;
        tm.tm_hour = hour;
        tm.tm_min = min;
        tm.tm_sec = 30;
        tm.tm_isdst = 0;
        tm
    }

    #[test]
    fn the_morning_after_is_the_next_days_nine_oclock() {
        let at = |tm: libc::tm| (tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec, tm.tm_isdst);
        assert_eq!(at(morning_after(tm(27, 14, 5))), (28, 9, 0, 0, -1));
        assert_eq!(at(morning_after(tm(27, 9, 0))), (28, 9, 0, 0, -1));
        assert_eq!(at(morning_after(tm(27, 5, 0))), (28, 9, 0, 0, -1));
        assert_eq!(at(morning_after(tm(27, 23, 59))), (28, 9, 0, 0, -1));
        // Past midnight, before the night is over: this morning.
        assert_eq!(at(morning_after(tm(27, 0, 10))), (27, 9, 0, 0, -1));
        assert_eq!(at(morning_after(tm(27, 4, 59))), (27, 9, 0, 0, -1));
        // The month's end is left for `mktime` to roll over.
        assert_eq!(at(morning_after(tm(31, 12, 0))), (32, 9, 0, 0, -1));
    }

    /// The local hour and minute at `secs`.
    fn local(secs: u64) -> (i32, i32, i32) {
        let time = secs as libc::time_t;
        // SAFETY: as in `tomorrow_morning`.
        unsafe {
            let mut tm: libc::tm = std::mem::zeroed();
            assert!(!libc::localtime_r(&time, &mut tm).is_null());
            (tm.tm_yday, tm.tm_hour, tm.tm_min)
        }
    }

    #[test]
    fn tomorrow_morning_is_nine_oclock_local_and_ahead() {
        let start = 1_792_800_000;
        // Every half hour over ten days that cross the ends of daylight saving
        // time in Europe and America, in whatever zone the tests run.
        for now in (start..start + 10 * 24 * 3600).step_by(1800) {
            let morning = tomorrow_morning(now);
            assert!(morning > now, "{now}");
            assert!(morning - now <= 33 * 3600, "{now}");
            let (_, hour, min) = local(morning);
            assert_eq!((hour, min), (9, 0), "{now}");
            let (day, hour, _) = local(now);
            let (morning_day, _, _) = local(morning);
            if hour < NIGHT_ENDS {
                assert_eq!(morning_day, day, "{now}");
            } else {
                assert_ne!(morning_day, day, "{now}");
            }
        }
    }
}

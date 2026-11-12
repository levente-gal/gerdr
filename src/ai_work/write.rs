//! Writing a feature's AI work: lines appended to this machine's events
//! file, the check logs kept as blobs, the record's last snapshot and how
//! far it was taken (`cursor.json`). Folders are 0700 and files 0600:
//! transcripts and logs can hold secrets.
//!
//! Nothing here syncs to disk: the appends are small and come from the
//! loop, and the snapshots are written again at the next change.

use std::{
    collections::BTreeMap,
    fs::{DirBuilder, File, OpenOptions},
    io::{self, Write as _},
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};

use super::{BlobRef, Line};

/// The events file of machine `machine` in feature folder `dir`.
pub fn events_path(dir: &Path, machine: &str) -> PathBuf {
    let machine = match machine {
        "" => "here".to_owned(),
        m => safe_name(m),
    };
    dir.join(format!("events-{machine}.jsonl"))
}

/// The name the lines machine `from` wrote with no id yet (its
/// `events-here.jsonl`) are kept under here: every machine has a file of
/// that name, each its own.
pub fn brought_name(from: &str) -> String {
    format!("events-here-{}.jsonl", safe_name(from))
}

/// `name` fit for a file's name: what isn't a letter, digit, `-`, `_` or
/// `.` becomes `_`, and a name of dots only is `_` too.
pub fn safe_name(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(
            |c| match c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                true => c,
                false => '_',
            },
        )
        .collect();
    match safe.trim_matches('.').is_empty() {
        true => "_".to_owned(),
        false => safe,
    }
}

/// Make folder `path` and those above it, each made 0700.
pub fn make_dir(path: &Path) -> io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(path)
}

/// File `path` opened to append to; it and its folders are made as needed,
/// 0600 and 0700.
pub fn open_append(path: &Path) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        make_dir(parent)?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
}

/// Write `bytes` to `path` whole or not at all: to a file beside it, then
/// renamed over it. A new file is made 0600, its folders 0700; one replaced
/// keeps its mode (a config the user made readable, say). A symlink is
/// written at its target and stays a link; a dangling one is replaced.
pub fn atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    static N: AtomicU64 = AtomicU64::new(0);
    let target = match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => std::fs::canonicalize(path).ok(),
        _ => None,
    };
    let path = target.as_deref().unwrap_or(path);
    let parent = path.parent().unwrap_or(Path::new("."));
    make_dir(parent)?;
    let name = path
        .file_name()
        .map_or_else(|| "file".into(), |name| name.to_string_lossy().into_owned());
    let n = N.fetch_add(1, Ordering::Relaxed);
    let tmp = parent.join(format!(".{name}.{}-{n}.tmp", std::process::id()));
    let written = (|| {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(bytes)?;
        if let Ok(old) = std::fs::metadata(path) {
            file.set_permissions(old.permissions())?;
        }
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        // What is left of it is of no use; the error said is the write's.
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// Append `line` to `events-<machine>.jsonl` of feature folder `dir`, in
/// one small write.
pub fn append(dir: &Path, line: &Line) -> io::Result<()> {
    let mut text = serde_json::to_vec(line).map_err(io::Error::other)?;
    text.push(b'\n');
    let mut file = open_append(&events_path(dir, &line.machine))?;
    // An append of one buffer: lines of other threads never mix into it.
    file.write_all(&text)
}

/// Keep what a check printed, `scrollback` cut from the last `head` (the
/// line a check starts with) on, in feature folder `dir`. True with it:
/// `head` was gone, so its start is missing.
pub fn check_log(dir: &Path, scrollback: &str, head: &str) -> io::Result<(BlobRef, bool)> {
    let (from, truncated) = match scrollback.rfind(head) {
        Some(at) => (at, false),
        None => (0, true),
    };
    let blob = super::blobs::put(dir, &scrollback.as_bytes()[from..])?;
    Ok((blob, truncated))
}

/// The record's latest snapshot in feature folder `dir`.
pub fn record_path(dir: &Path) -> PathBuf {
    dir.join("record.json")
}

/// Keep `record` (the feature's `feature.json`, as JSON) as the latest
/// snapshot in feature folder `dir`.
pub fn record(dir: &Path, record: &[u8]) -> io::Result<()> {
    atomic(&record_path(dir), record)
}

/// How far a feature's AI work was taken: `cursor.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Marks {
    /// `FORMAT` when it was written.
    pub format: u32,
    /// How many lines of the record's history are in the events already.
    pub history_noted: usize,
    /// The blob of the last `Snapshot`.
    pub last_snapshot: Option<BlobRef>,
    /// The earlier AI work was kept (see `backfill`).
    pub backfilled: bool,
    /// The stage files last kept, by name: the SHA-256 of each (see
    /// `thread`'s `Outputs`).
    pub outputs: BTreeMap<String, String>,
}

/// `cursor.json` of feature folder `dir`.
pub fn marks_path(dir: &Path) -> PathBuf {
    dir.join("cursor.json")
}

/// How far feature folder `dir` was taken. None yet: from the start.
pub fn read_marks(dir: &Path) -> io::Result<Marks> {
    match std::fs::read(marks_path(dir)) {
        Ok(raw) => serde_json::from_slice(&raw).map_err(io::Error::other),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Marks::default()),
        Err(err) => Err(err),
    }
}

/// Keep `marks` as how far feature folder `dir` was taken.
pub fn write_marks(dir: &Path, marks: &Marks) -> io::Result<()> {
    let marks = Marks {
        format: super::FORMAT,
        ..marks.clone()
    };
    let text = serde_json::to_vec_pretty(&marks).map_err(io::Error::other)?;
    atomic(&marks_path(dir), &text)
}

#[cfg(test)]
pub(super) mod tests {

    #[test]
    fn a_symlink_is_written_at_its_target() {
        let dir = std::env::temp_dir().join(format!("gerdr-atomic-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("real"), "old").unwrap();
        std::os::unix::fs::symlink("real", dir.join("link")).unwrap();
        super::atomic(&dir.join("link"), b"new").unwrap();
        assert!(
            std::fs::symlink_metadata(dir.join("link"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(dir.join("real")).unwrap(), "new");
        std::fs::remove_dir_all(&dir).unwrap();
    }
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;
    use crate::{
        ai_work::{Event, FORMAT, RunChange},
        feature::Stage,
    };

    /// A folder of its own for test `name`, empty, under the temp folder.
    pub fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gerdr-ai-work-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn line(n: u32) -> Line {
        Line {
            v: FORMAT,
            at_ms: 7,
            machine: "m1".into(),
            backfill: false,
            event: Event::RunChanged {
                stage: Stage::Plan,
                n,
                change: RunChange::Ready,
            },
        }
    }

    #[test]
    fn a_replaced_file_keeps_its_mode() {
        let path = scratch("keep-mode").join("config.toml");
        atomic(&path, b"a").unwrap();
        assert_eq!(mode(&path), 0o600);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        atomic(&path, b"b").unwrap();
        assert_eq!(
            (std::fs::read(&path).unwrap(), mode(&path)),
            (b"b".to_vec(), 0o644)
        );
    }

    #[test]
    fn lines_append_to_the_machines_file_private() {
        let dir = scratch("append").join("proj").join("slug");
        append(&dir, &line(1)).unwrap();
        append(&dir, &line(2)).unwrap();
        let path = dir.join("events-m1.jsonl");
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<Line> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines, vec![line(1), line(2)]);
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(dir.parent().unwrap()), 0o700);
    }

    #[test]
    fn a_check_log_is_cut_at_its_last_head_and_kept() {
        let dir = scratch("check-log");
        let scroll = "old\n== npm test\nfirst\n== npm test\nsecond\n";
        let (blob, truncated) = check_log(&dir, scroll, "== npm test").unwrap();
        assert!(!truncated);
        let kept = super::super::blobs::get(&dir, &blob).unwrap();
        assert_eq!(kept, b"== npm test\nsecond\n");
        let (blob, truncated) = check_log(&dir, "no head", "== npm test").unwrap();
        assert!(truncated);
        assert_eq!(super::super::blobs::get(&dir, &blob).unwrap(), b"no head");
    }

    #[test]
    fn snapshot_and_marks_are_written_whole_and_private() {
        let dir = scratch("marks");
        record(&dir, b"{\"v\":1}").unwrap();
        record(&dir, b"{\"v\":2}").unwrap();
        assert_eq!(std::fs::read(record_path(&dir)).unwrap(), b"{\"v\":2}");
        assert_eq!(mode(&record_path(&dir)), 0o600);
        assert_eq!(read_marks(&dir).unwrap(), Marks::default());
        let marks = Marks {
            history_noted: 4,
            backfilled: true,
            ..Marks::default()
        };
        write_marks(&dir, &marks).unwrap();
        let read = read_marks(&dir).unwrap();
        assert_eq!(read.format, FORMAT);
        assert_eq!(read.history_noted, 4);
        assert!(read.backfilled);
        assert_eq!(mode(&marks_path(&dir)), 0o600);
        // No temp file is left behind.
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
    }

    #[test]
    fn names_are_made_safe_for_files() {
        assert_eq!(safe_name("a/b:c"), "a_b_c");
        assert_eq!(safe_name(".."), "_");
        assert_eq!(
            events_path(Path::new("/d"), ""),
            Path::new("/d/events-here.jsonl")
        );
    }
}

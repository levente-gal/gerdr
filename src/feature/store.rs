//! Where features are kept: `<data>/features/<project>/<slug>/feature.json`,
//! beside the stage files later increments write there (`brief.md`,
//! `plan.md`…). `<data>` is gerdr's data folder (`projects::data_dir`), or
//! a folder given in tests.
//!
//! What only grows is kept beside the record, so saving it stays small: its
//! history in `history.jsonl`, one event a line and appended to, and its AI
//! review's earlier rounds in `review-rounds.json`, written again only when
//! they change. `Feature` reads and writes as it always did: `save_at` takes
//! them out of what it writes, and the readers here put them back. An older
//! gerdr writes them in the record again; reading folds them back in.

use std::{
    collections::{BTreeMap, HashMap},
    hash::{DefaultHasher, Hash, Hasher},
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    sync::{
        LazyLock, Mutex, PoisonError, RwLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::SystemTime,
};

use super::{Event, Feature, VERSION, review_import::ReviewRound};

/// The folder of project `project`'s features under `root` (`None`: the
/// data folder).
pub fn project_dir(root: Option<&Path>, project: &str) -> PathBuf {
    let root = root.map_or_else(crate::projects::data_dir, Path::to_path_buf);
    root.join("features").join(project)
}

/// The copies of the features other machines write, by project and slug: where
/// their folders are. Everything reading a feature's files goes by `dir`, from
/// threads too, so this is where they are told apart.
static SEEN: RwLock<BTreeMap<(String, String), PathBuf>> = RwLock::new(BTreeMap::new());

/// The features shown here that other machines write: their folders.
pub fn set_seen(seen: BTreeMap<(String, String), PathBuf>) {
    if let Ok(mut mine) = SEEN.write() {
        *mine = seen;
    }
}

/// Feature `slug`'s folder: its record and stage files.
pub fn dir(root: Option<&Path>, project: &str, slug: &str) -> PathBuf {
    if root.is_none()
        && let Ok(seen) = SEEN.read()
        && let Some(found) = seen.get(&(project.to_owned(), slug.to_owned()))
    {
        return found.clone();
    }
    project_dir(root, project).join(slug)
}

/// How many features `load_all` moved the history of out of their records,
/// since it was last asked (`take_moved`).
static MOVED: AtomicUsize = AtomicUsize::new(0);

/// How many features had their history and review rounds moved to files of
/// their own since this was last asked: what the user is told once.
pub fn take_moved() -> usize {
    MOVED.swap(0, Ordering::Relaxed)
}

/// Every feature of project `project`, open or not, oldest first. A record
/// that can't be read is set aside (`feature.json.bad-<time>`, like the
/// other stores do) and logged, and the rest still load. A folder that
/// can't be listed is an error.
///
/// A record of an older format is saved in this one: one that still holds
/// its history has it moved out (counted by `take_moved`). The files are
/// synced to disk on a thread of their own (`sync_later`), so a start with
/// many to move doesn't wait on the disk.
pub fn load_all(root: Option<&Path>, project: &str) -> Result<Vec<Feature>, String> {
    let (features, moved) = load_dir(&project_dir(root, project))?;
    MOVED.fetch_add(moved, Ordering::Relaxed);
    Ok(features)
}

/// `load_all` of folder `folder`, and how many features had their history moved
/// out of their records.
fn load_dir(folder: &Path) -> Result<(Vec<Feature>, usize), String> {
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok((Vec::new(), 0)),
        Err(err) => return Err(format!("couldn't list {}: {err}", folder.display())),
    };
    let mut features = Vec::new();
    let mut moved = 0;
    let mut written = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| format!("couldn't list {}: {err}", folder.display()))?;
        let dir = entry.path();
        let path = dir.join(FILE);
        let raw = match std::fs::read(&path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == ErrorKind::NotFound => continue,
            Err(err) if err.kind() == ErrorKind::NotADirectory => continue,
            // Not set aside: the file may be fine, only out of reach now.
            Err(err) => {
                tracing::warn!("reading {}: {err}", path.display());
                continue;
            }
        };
        match serde_json::from_slice::<Feature>(&raw) {
            // A copied folder (`cp -r login login-old`): saving it would go
            // over the real one's.
            Ok(feature) if dir.file_name() != Some(feature.slug.as_ref()) => {
                tracing::warn!(
                    "{} is of feature {:?}: not loaded",
                    path.display(),
                    feature.slug
                );
                crate::problems::report(
                    "store",
                    format!(
                        "{} is of feature {:?}, not of its folder: it isn't loaded",
                        path.display(),
                        feature.slug
                    ),
                );
            }
            Ok(mut feature) => {
                let inline = holds_sides(&feature);
                let first = inline && !dir.join(HISTORY).exists() && !dir.join(ROUNDS).exists();
                attach_sides(&dir, &mut feature);
                // Unsaved, it is only brought up to date again next time.
                if feature.migrate(super::now()) || inline {
                    match write_at(&dir, &feature) {
                        Ok(()) => {
                            moved += usize::from(first);
                            written.push(dir.clone());
                        }
                        Err(err) => {
                            tracing::warn!("saving {} in its new format: {err}", path.display());
                        }
                    }
                }
                features.push(feature);
            }
            Err(err) => {
                tracing::warn!("reading {}: {err}", path.display());
                crate::projects::set_aside(&path);
            }
        }
    }
    features.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.slug.cmp(&b.slug)));
    sync_later(written);
    Ok((features, moved))
}

/// Sync to disk, on a thread of its own, the files saved in folders `dirs`
/// and the folders: what is kept beside each record first, then the
/// record. Only syncs: a save made meanwhile is never written over.
fn sync_later(dirs: Vec<PathBuf>) {
    if dirs.is_empty() {
        return;
    }
    let sync = move || {
        for dir in &dirs {
            for name in [HISTORY, ROUNDS, FILE] {
                let path = dir.join(name);
                match std::fs::File::open(&path).and_then(|f| f.sync_all()) {
                    Err(err) if err.kind() != ErrorKind::NotFound => {
                        tracing::warn!("syncing {}: {err}", path.display());
                    }
                    _ => {}
                }
            }
            if let Err(err) = std::fs::File::open(dir).and_then(|f| f.sync_all()) {
                tracing::warn!("syncing {}: {err}", dir.display());
            }
        }
    };
    if let Err(err) = std::thread::Builder::new()
        .name("feature store sync".into())
        .spawn(sync)
    {
        tracing::warn!("syncing the features saved in their new format: {err}");
    }
}

/// The record's file name in a feature's folder.
pub const FILE: &str = "feature.json";

/// The feature's history, beside its record: one `Event` a line, appended to.
pub const HISTORY: &str = "history.jsonl";

/// Its AI review's earlier rounds (`AiReview::rounds`), beside its record: a
/// JSON list, written again whole when it changes.
pub const ROUNDS: &str = "review-rounds.json";

/// Whether record `feature` holds what is kept beside it: one from an older
/// gerdr, or not saved here yet.
fn holds_sides(feature: &Feature) -> bool {
    !feature.history.is_empty()
        || feature
            .review
            .as_ref()
            .is_some_and(|r| !r.rounds.is_empty())
}

/// The record in folder `dir`, with what is kept beside it. Not brought up to
/// date (`Feature::migrate`).
pub fn read_at(dir: &Path) -> Result<Feature, String> {
    let path = dir.join(FILE);
    let raw = std::fs::read(&path).map_err(|err| format!("reading {}: {err}", path.display()))?;
    let mut feature: Feature =
        serde_json::from_slice(&raw).map_err(|err| format!("reading {}: {err}", path.display()))?;
    attach_sides(dir, &mut feature);
    Ok(feature)
}

/// Put back in `feature`, read from folder `dir`, what is kept beside its
/// record. What the record still holds itself, as an older gerdr writes it,
/// stays too: history it has that the file doesn't comes after the file's, and
/// a review round of its own goes over the file's of that number. A file that
/// can't be read is set aside (`projects::set_aside`), and the record's own
/// is all there is.
pub fn attach_sides(dir: &Path, feature: &mut Feature) {
    let path = dir.join(HISTORY);
    match read_history(&path) {
        Ok(Some(read)) => {
            if read.whole {
                let written = Written {
                    count: read.events.len(),
                    hash: history_hash(&read.events),
                    len: read.len,
                };
                saved().insert(path, written);
            }
            let inline = std::mem::take(&mut feature.history);
            feature.history = merge_history(read.events, inline);
        }
        Ok(None) => {}
        Err(err) => {
            tracing::warn!("reading {}: {err}", path.display());
            crate::projects::set_aside(&path);
        }
    }
    // With no review, rounds of one are nothing: the next save takes them away.
    let Some(review) = feature.review.as_mut() else {
        return;
    };
    let path = dir.join(ROUNDS);
    let raw = match std::fs::read(&path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == ErrorKind::NotFound => return,
        Err(err) => {
            tracing::warn!("reading {}: {err}", path.display());
            crate::projects::set_aside(&path);
            return;
        }
    };
    match serde_json::from_slice::<Vec<ReviewRound>>(&raw) {
        Ok(rounds) => {
            let written = Written {
                count: rounds.len(),
                hash: bytes_hash(&raw),
                len: raw.len() as u64,
            };
            saved().insert(path, written);
            let inline = std::mem::take(&mut review.rounds);
            review.rounds = merge_rounds(rounds, inline);
        }
        Err(err) => {
            tracing::warn!("reading {}: {err}", path.display());
            crate::projects::set_aside(&path);
        }
    }
}

/// History file `side`'s events, then those of record `inline` it doesn't
/// have: what an older gerdr added. Only the file's events from the first of
/// `inline` on can be the same, and each counts once.
fn merge_history(mut side: Vec<Event>, inline: Vec<Event>) -> Vec<Event> {
    let Some(first) = inline.iter().map(|e| e.at).min() else {
        return side;
    };
    let keep: Vec<bool> = {
        let mut had: HashMap<(u64, &str), usize> = HashMap::new();
        for e in side.iter().filter(|e| e.at >= first) {
            *had.entry((e.at, e.what.as_str())).or_default() += 1;
        }
        (inline.iter())
            .map(|e| match had.get_mut(&(e.at, e.what.as_str())) {
                Some(n) if *n > 0 => {
                    *n -= 1;
                    false
                }
                _ => true,
            })
            .collect()
    };
    side.extend(
        inline
            .into_iter()
            .zip(keep)
            .filter_map(|(e, keep)| keep.then_some(e)),
    );
    side
}

/// Rounds file `side`'s rounds and record `inline`'s, by number: one of the
/// record's goes over the file's of the same number, as the record was written
/// last.
fn merge_rounds(side: Vec<ReviewRound>, inline: Vec<ReviewRound>) -> Vec<ReviewRound> {
    let mut by_n: BTreeMap<u32, ReviewRound> = side.into_iter().map(|r| (r.n, r)).collect();
    by_n.extend(inline.into_iter().map(|r| (r.n, r)));
    by_n.into_values().collect()
}

/// A history file as read.
struct ReadHistory {
    events: Vec<Event>,
    /// Every line whole and read: the next save can append to it.
    whole: bool,
    /// Its length in bytes.
    len: u64,
}

/// The events in history file `path`; `None` when there is none. A line that
/// can't be read is skipped: a last one cut short by a crash, most likely.
fn read_history(path: &Path) -> std::io::Result<Option<ReadHistory>> {
    let raw = match std::fs::read(path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    let mut whole = raw.is_empty() || raw.ends_with(b"\n");
    let mut events = Vec::new();
    let mut lines = raw.split(|b| *b == b'\n').enumerate().peekable();
    while let Some((i, line)) = lines.next() {
        if line.is_empty() {
            continue;
        }
        match serde_json::from_slice::<Event>(line) {
            Ok(event) => events.push(event),
            // How a crash leaves it: no line after it, and no line end.
            Err(_) if lines.peek().is_none() => {
                tracing::info!("{}: its last line is cut short, skipped", path.display());
            }
            Err(err) => {
                whole = false;
                tracing::warn!("{}: line {} skipped: {err}", path.display(), i + 1);
            }
        }
    }
    Ok(Some(ReadHistory {
        events,
        whole,
        len: raw.len() as u64,
    }))
}

/// What was last written of a side file, or read of it whole: it is still that
/// file while it is this long.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Written {
    /// How many events or rounds.
    count: usize,
    /// `history_hash` of the events, `bytes_hash` of a rounds file.
    hash: u64,
    /// The file's length.
    len: u64,
}

/// The side files written or read here, by path: whether a save appends, and
/// whether it writes anything at all. A file not known here, or not as long as
/// it was, is written again whole.
static SAVED: LazyLock<Mutex<HashMap<PathBuf, Written>>> = LazyLock::new(Default::default);

fn saved() -> std::sync::MutexGuard<'static, HashMap<PathBuf, Written>> {
    SAVED.lock().unwrap_or_else(PoisonError::into_inner)
}

fn history_hash(events: &[Event]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for e in events {
        e.at.hash(&mut hasher);
        e.what.hash(&mut hasher);
    }
    hasher.finish()
}

fn bytes_hash(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    hasher.write(bytes);
    hasher.finish()
}

/// File `path`'s length, 0 when there is none.
fn len_of(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |m| m.len())
}

/// Save `feature`'s record, atomically (`projects::write_atomic`).
pub fn save(root: Option<&Path>, feature: &Feature) -> std::io::Result<()> {
    let outside = |name: &str| name.is_empty() || name.contains('/') || name.contains("..");
    if outside(&feature.slug) || outside(&feature.project) {
        return Err(std::io::Error::other(format!(
            "not saving feature {:?} of {:?}: not a folder name",
            feature.slug, feature.project
        )));
    }
    save_at(&dir(root, &feature.project, &feature.slug), feature)
}

/// Save `feature`'s record in folder `dir`, as `save` does: for a feature that
/// `dir` would still find elsewhere. What is kept beside the record is written
/// first: a record never names more than the files have.
pub fn save_at(dir: &Path, feature: &Feature) -> std::io::Result<()> {
    write_at(dir, feature)
}

/// `save_at`: the side files, then the record.
fn write_at(dir: &Path, feature: &Feature) -> std::io::Result<()> {
    save_history(dir, &feature.history)?;
    let rounds = feature
        .review
        .as_ref()
        .map_or(&[][..], |r| r.rounds.as_slice());
    save_rounds(dir, rounds)?;
    let mut feature = feature.clone();
    feature.version = VERSION;
    feature.history = Vec::new();
    if let Some(review) = &mut feature.review {
        review.rounds = Vec::new();
    }
    let bytes = serde_json::to_vec_pretty(&feature).map_err(std::io::Error::other)?;
    crate::projects::write_atomic(&dir.join(FILE), &bytes)
}

/// Write `events` to the history file in `dir`: only the new ones, when it is
/// still what was last written of them. No events never empty a file: history
/// only grows, and a record read without it would lose it all.
fn save_history(dir: &Path, events: &[Event]) -> std::io::Result<()> {
    if events.is_empty() {
        return Ok(());
    }
    let path = dir.join(HISTORY);
    let mut saved = saved();
    let known = saved.remove(&path);
    let from = known
        .filter(|w| w.count <= events.len() && w.len == len_of(&path))
        .filter(|w| history_hash(&events[..w.count]) == w.hash);
    let mut bytes = Vec::new();
    let start = from.map_or(0, |w| w.count);
    for e in &events[start..] {
        serde_json::to_writer(&mut bytes, e).map_err(std::io::Error::other)?;
        bytes.push(b'\n');
    }
    let len = match from {
        Some(w) if w.count == events.len() => w.len,
        Some(w) => {
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(&path)?;
            file.write_all(&bytes)?;
            w.len + bytes.len() as u64
        }
        None => {
            crate::projects::write_atomic(&path, &bytes)?;
            bytes.len() as u64
        }
    };
    let written = Written {
        count: events.len(),
        hash: history_hash(events),
        len,
    };
    saved.insert(path, written);
    Ok(())
}

/// Write `rounds` to the rounds file in `dir`, when they aren't what it holds;
/// none take it away.
fn save_rounds(dir: &Path, rounds: &[ReviewRound]) -> std::io::Result<()> {
    let path = dir.join(ROUNDS);
    let mut saved = saved();
    let known = saved.remove(&path);
    if rounds.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(err) if err.kind() != ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        };
    }
    let bytes = serde_json::to_vec_pretty(rounds).map_err(std::io::Error::other)?;
    let written = Written {
        count: rounds.len(),
        hash: bytes_hash(&bytes),
        len: bytes.len() as u64,
    };
    if known != Some(written) || len_of(&path) != written.len {
        crate::projects::write_atomic(&path, &bytes)?;
    }
    saved.insert(path, written);
    Ok(())
}

/// Make side file `name` (`HISTORY` or `ROUNDS`) in folder `dir` hold
/// `bytes`, as another machine has it; none take it away. What was known
/// of the file here is forgotten, so the next save writes it whole if it
/// must.
pub fn put_side(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let path = dir.join(name);
    saved().remove(&path);
    if bytes.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(err) if err.kind() != ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        };
    }
    crate::projects::write_atomic(&path, bytes)
}

/// How the files kept beside a record in a folder stand: when it changes, so
/// may the feature, with its record the same.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stamp([Option<(u64, SystemTime)>; 2]);

/// How the files beside the record in folder `dir` stand.
pub fn sides_stamp(dir: &Path) -> Stamp {
    let of = |name: &str| {
        let meta = std::fs::metadata(dir.join(name)).ok()?;
        Some((meta.len(), meta.modified().ok()?))
    };
    Stamp([of(HISTORY), of(ROUNDS)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::{Stage, tests::feature};

    struct Temp(PathBuf);

    impl Temp {
        fn new() -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "gerdr-feature-store-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_feature_shown_from_elsewhere_reads_from_its_copy() {
        let project = "seen-project-of-this-test";
        let copy = PathBuf::from("/copy/of/it");
        let seen = BTreeMap::from([((project.to_owned(), "a".to_owned()), copy.clone())]);
        set_seen(seen);
        assert_eq!(dir(None, project, "a"), copy);
        // Only that one, and not for a folder given.
        assert_ne!(dir(None, project, "b"), copy);
        assert_eq!(
            dir(Some(Path::new("/r")), project, "a"),
            Path::new("/r/features").join(project).join("a")
        );
        set_seen(BTreeMap::new());
        assert_ne!(dir(None, project, "a"), copy);
    }

    #[test]
    fn a_missing_folder_is_no_features() {
        let t = Temp::new();
        assert_eq!(load_all(Some(&t.0), "p"), Ok(Vec::new()));
    }

    #[test]
    fn saves_and_loads_oldest_first() {
        let t = Temp::new();
        let mut b = feature("b", 5);
        b.version = 0;
        b.tick(6, "you", None);
        let a = feature("a", 5);
        let c = feature("c", 1);
        for f in [&b, &a, &c] {
            save(Some(&t.0), f).unwrap();
        }
        let all = load_all(Some(&t.0), "p").unwrap();
        let slugs: Vec<_> = all.iter().map(|f| f.slug.as_str()).collect();
        assert_eq!(slugs, ["c", "a", "b"]);
        assert_eq!(all[2].version, VERSION);
        assert!(all[2].is_done(Stage::Discover));
        assert_eq!(all[2].history, b.history);
        assert!(t.0.join("features/p/b/feature.json").is_file());
    }

    #[test]
    fn a_feature_past_plan_keeps_its_place_once_and_for_all() {
        let t = Temp::new();
        let old = |slug: &str, done: Stage| {
            let mut f = feature(slug, 5);
            for stage in Stage::ALL.into_iter().filter(|s| *s <= done) {
                if stage != Stage::PlanReview {
                    f.marks.insert(
                        stage,
                        crate::feature::Mark {
                            at: 7,
                            by: "you".into(),
                            note: String::new(),
                            patch: None,
                        },
                    );
                }
            }
            f
        };
        let write_old = |f: &Feature| {
            let path = dir(Some(&t.0), "p", &f.slug).join(FILE);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let mut f = f.clone();
            f.version = 1;
            std::fs::write(path, serde_json::to_vec(&f).unwrap()).unwrap();
        };
        write_old(&old("built", Stage::Build));
        write_old(&old("planned", Stage::Plan));
        let mut started = old("started", Stage::Plan);
        started.runs.insert(Stage::Build, Default::default());
        write_old(&started);
        let all = load_all(Some(&t.0), "p").unwrap();
        let of = |slug: &str| all.iter().find(|f| f.slug == slug).unwrap();
        // Past Plan: not sent back to a stage it never had.
        assert_eq!(of("built").current(), Some(Stage::Verify));
        let mark = &of("built").marks[&Stage::PlanReview];
        assert_eq!((mark.at, mark.by.as_str()), (7, "gerdr"));
        // A Build agent had started, not marked done yet.
        assert_eq!(of("started").current(), Some(Stage::Build));
        // Plan approved, Build not started: it gets the review.
        assert_eq!(of("planned").current(), Some(Stage::PlanReview));
        // Saved: reading it again finds nothing to do.
        let again = load_all(Some(&t.0), "p").unwrap();
        assert_eq!(again, all);
    }

    #[test]
    fn a_bad_record_is_set_aside() {
        let t = Temp::new();
        save(Some(&t.0), &feature("good", 1)).unwrap();
        let bad = dir(Some(&t.0), "p", "bad");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join(FILE), "{ not json").unwrap();
        // A stray file in the project folder is no feature.
        std::fs::write(project_dir(Some(&t.0), "p").join("stray"), "x").unwrap();
        let all = load_all(Some(&t.0), "p").unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].slug, "good");
        assert!(!bad.join(FILE).exists());
        let kept = std::fs::read_dir(&bad)
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("feature.json.bad-")
            });
        assert!(kept);
    }

    use crate::feature::{
        Event,
        review_import::{AiReview, Finding, ReviewRound},
    };

    fn event(at: u64, what: &str) -> Event {
        Event {
            at,
            what: what.into(),
        }
    }

    fn round(n: u32, id: &str) -> ReviewRound {
        ReviewRound {
            n,
            findings: vec![Finding {
                id: id.into(),
                title: format!("Finding {id}"),
                ..Finding::default()
            }],
        }
    }

    /// A feature with a history and review rounds, as a gerdr writes it.
    fn busy(slug: &str) -> Feature {
        let mut f = feature(slug, 5);
        f.history = vec![
            event(1, "created"),
            event(2, "Brief ticked"),
            event(2, "Brief ticked"),
        ];
        f.review = Some(AiReview {
            n: 3,
            rounds: vec![round(1, "r1-1"), round(2, "r2-1")],
            ..AiReview::default()
        });
        f
    }

    /// The record as the file has it, nothing put back.
    fn record_only(dir: &Path) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(dir.join(FILE)).unwrap()).unwrap()
    }

    fn inode(path: &Path) -> u64 {
        std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(path).unwrap())
    }

    #[test]
    fn a_copied_folder_isnt_loaded_and_a_slug_cant_leave_its_folder() {
        let t = Temp::new();
        let f = feature("a", 5);
        save(Some(&t.0), &f).unwrap();
        let copy = dir(Some(&t.0), "p", "a-old");
        std::fs::create_dir_all(&copy).unwrap();
        std::fs::copy(dir(Some(&t.0), "p", "a").join(FILE), copy.join(FILE)).unwrap();
        assert_eq!(load_all(Some(&t.0), "p").unwrap(), [f.clone()]);
        assert!(copy.join(FILE).is_file(), "the copy is left alone");
        let escaping = Feature {
            slug: "../../x".into(),
            ..f
        };
        assert!(save(Some(&t.0), &escaping).is_err());
    }

    #[test]
    fn a_saved_record_keeps_its_history_and_rounds_beside_it() {
        let t = Temp::new();
        let f = busy("a");
        save(Some(&t.0), &f).unwrap();
        let dir = dir(Some(&t.0), "p", "a");
        let record = record_only(&dir);
        assert!(record.get("history").is_none(), "{record}");
        assert!(record["review"].get("rounds").is_none(), "{record}");
        assert_eq!(record["review"]["n"], 3);
        let lines = std::fs::read_to_string(dir.join(HISTORY)).unwrap();
        assert_eq!(lines.lines().count(), 3);
        assert!(dir.join(ROUNDS).is_file());
        assert_eq!(read_at(&dir).unwrap(), f);
        assert_eq!(load_all(Some(&t.0), "p").unwrap(), [f]);
    }

    #[test]
    fn an_older_record_moves_its_history_out_once() {
        let t = Temp::new();
        let mut f = busy("a");
        f.version = 2;
        let dir = dir(Some(&t.0), "p", "a");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE), serde_json::to_vec_pretty(&f).unwrap()).unwrap();
        // One with nothing to move isn't counted.
        let mut empty = feature("b", 6);
        empty.version = 2;
        let other = super::dir(Some(&t.0), "p", "b");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join(FILE), serde_json::to_vec(&empty).unwrap()).unwrap();
        let (all, moved) = load_dir(&project_dir(Some(&t.0), "p")).unwrap();
        assert_eq!(moved, 1);
        f.version = VERSION;
        empty.version = VERSION;
        assert_eq!(all, [f.clone(), empty]);
        let record = record_only(&dir);
        assert_eq!(record["version"], VERSION);
        assert!(record.get("history").is_none());
        assert!(record["review"].get("rounds").is_none());
        assert_eq!(read_at(&dir).unwrap(), f);
        // Moved: reading it again moves nothing.
        let (again, moved) = load_dir(&project_dir(Some(&t.0), "p")).unwrap();
        assert_eq!((moved, &again), (0, &all));
    }

    #[test]
    fn what_an_older_gerdr_writes_in_the_record_is_folded_back_in() {
        let t = Temp::new();
        let f = busy("a");
        save(Some(&t.0), &f).unwrap();
        let dir = dir(Some(&t.0), "p", "a");
        // It saw no history and no rounds, added its own, and wrote them in.
        let mut old = read_at(&dir).unwrap();
        old.version = 2;
        old.history = vec![event(9, "Plan ticked")];
        old.review.as_mut().unwrap().rounds = vec![round(3, "r3-1"), round(2, "r2-9")];
        std::fs::write(dir.join(FILE), serde_json::to_vec(&old).unwrap()).unwrap();
        let all = load_all(Some(&t.0), "p").unwrap();
        let history: Vec<_> = all[0].history.iter().map(|e| e.at).collect();
        assert_eq!(history, [1, 2, 2, 9]);
        let rounds = &all[0].review.as_ref().unwrap().rounds;
        let ids: Vec<_> = rounds.iter().map(|r| r.findings[0].id.as_str()).collect();
        // The record's round 2 goes over the file's: it was written last.
        assert_eq!(ids, ["r1-1", "r2-9", "r3-1"]);
        // Saved as this gerdr writes it.
        assert!(record_only(&dir).get("history").is_none());
        assert_eq!(read_at(&dir).unwrap(), all[0]);
    }

    #[test]
    fn history_both_in_the_file_and_the_record_counts_once() {
        let t = Temp::new();
        let f = busy("a");
        let dir = dir(Some(&t.0), "p", "a");
        // As a crash leaves the move: the file written, the record not yet.
        save_at(&dir, &f).unwrap();
        let mut old = f.clone();
        old.version = 2;
        old.history.push(event(4, "Plan ticked"));
        std::fs::write(dir.join(FILE), serde_json::to_vec(&old).unwrap()).unwrap();
        let back = read_at(&dir).unwrap();
        assert_eq!(back.history, old.history);
        assert_eq!(back.review, f.review);
    }

    /// A copy here newer by its time than another machine's is replaced all
    /// the same, and none there takes this one's away.
    #[test]
    fn sides_put_from_elsewhere_are_what_is_read_and_saved() {
        let t = Temp::new();
        let mut here = busy("a");
        here.log(99, "only here");
        let dir = dir(Some(&t.0), "p", "a");
        save_at(&dir, &here).unwrap();
        let there = busy("a");
        let mut lines = Vec::new();
        for e in &there.history {
            serde_json::to_writer(&mut lines, e).unwrap();
            lines.push(b'\n');
        }
        put_side(&dir, HISTORY, &lines).unwrap();
        put_side(&dir, ROUNDS, b"").unwrap();
        assert!(!dir.join(ROUNDS).exists());
        put_side(&dir, ROUNDS, b"").unwrap();
        let mut record = there.clone();
        record.history.clear();
        if let Some(review) = &mut record.review {
            review.rounds.clear();
        }
        attach_sides(&dir, &mut record);
        assert_eq!(record.history, there.history);
        record.log(100, "Taken over");
        save_at(&dir, &record).unwrap();
        assert_eq!(read_at(&dir).unwrap().history, record.history);
    }

    #[test]
    fn new_history_is_appended_and_anything_else_written_again() {
        let t = Temp::new();
        let mut f = busy("a");
        let dir = dir(Some(&t.0), "p", "a");
        let file = dir.join(HISTORY);
        save_at(&dir, &f).unwrap();
        let first = inode(&file);
        f.log(7, "Plan ticked");
        save_at(&dir, &f).unwrap();
        assert_eq!(inode(&file), first, "appended");
        assert_eq!(read_at(&dir).unwrap().history, f.history);
        // Nothing new: nothing written.
        let rounds = inode(&dir.join(ROUNDS));
        save_at(&dir, &f).unwrap();
        assert_eq!(inode(&file), first);
        assert_eq!(
            inode(&dir.join(ROUNDS)),
            rounds,
            "unchanged rounds aren't written"
        );
        // An earlier event that changed: written again whole.
        f.history[0].what = "made".into();
        save_at(&dir, &f).unwrap();
        let second = inode(&file);
        assert_ne!(second, first, "written again");
        assert_eq!(read_at(&dir).unwrap().history, f.history);
        // Shorter than what was written: written again.
        f.history.pop();
        save_at(&dir, &f).unwrap();
        let third = inode(&file);
        assert_ne!(third, second);
        assert_eq!(read_at(&dir).unwrap().history, f.history);
        // The file changed under it, as a copy from another machine does.
        let mut there = f.history.clone();
        there.push(event(8, "from there"));
        let lines: String = there
            .iter()
            .map(|e| serde_json::to_string(e).unwrap() + "\n")
            .collect();
        std::fs::write(&file, lines).unwrap();
        f.log(9, "here");
        save_at(&dir, &f).unwrap();
        assert_ne!(inode(&file), third);
        assert_eq!(read_at(&dir).unwrap().history, f.history);
        // Not known here, as after a restart: written again.
        saved().remove(&file);
        let fourth = inode(&file);
        f.log(10, "after a restart");
        save_at(&dir, &f).unwrap();
        assert_ne!(inode(&file), fourth);
        // Read whole, it is known again: the next is appended.
        let mut back = read_at(&dir).unwrap();
        let fifth = inode(&file);
        back.log(11, "once read");
        save_at(&dir, &back).unwrap();
        assert_eq!(inode(&file), fifth);
        assert_eq!(read_at(&dir).unwrap().history, back.history);
    }

    #[test]
    fn rounds_go_when_the_review_does() {
        let t = Temp::new();
        let mut f = busy("a");
        let dir = dir(Some(&t.0), "p", "a");
        save_at(&dir, &f).unwrap();
        f.review.as_mut().unwrap().rounds.push(round(3, "r3-1"));
        save_at(&dir, &f).unwrap();
        assert_eq!(read_at(&dir).unwrap(), f);
        // Started over: a review read later has none of these.
        f.review = None;
        save_at(&dir, &f).unwrap();
        assert!(!dir.join(ROUNDS).exists());
        f.review = Some(AiReview {
            n: 1,
            ..AiReview::default()
        });
        save_at(&dir, &f).unwrap();
        assert_eq!(read_at(&dir).unwrap(), f);
    }

    #[test]
    fn a_last_line_cut_short_is_skipped_and_mended() {
        let t = Temp::new();
        let mut f = busy("a");
        let dir = dir(Some(&t.0), "p", "a");
        save_at(&dir, &f).unwrap();
        let file = dir.join(HISTORY);
        let mut raw = std::fs::read(&file).unwrap();
        raw.extend_from_slice(br#"{"at":4,"wh"#);
        std::fs::write(&file, &raw).unwrap();
        saved().remove(&file);
        let mut back = read_at(&dir).unwrap();
        assert_eq!(back.history, f.history);
        // Not appended after the cut line: the file is written again whole.
        back.log(5, "Plan ticked");
        save_at(&dir, &back).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.ends_with('\n'));
        assert!(
            text.lines()
                .all(|l| serde_json::from_str::<Event>(l).is_ok()),
            "{text}"
        );
        f.log(5, "Plan ticked");
        assert_eq!(read_at(&dir).unwrap().history, f.history);
    }

    #[test]
    fn an_empty_history_never_empties_the_file() {
        let t = Temp::new();
        let f = busy("a");
        let dir = dir(Some(&t.0), "p", "a");
        save_at(&dir, &f).unwrap();
        // Read without what is beside it, by mistake, and saved.
        let bare: Feature = serde_json::from_value(record_only(&dir)).unwrap();
        save_at(&dir, &bare).unwrap();
        assert_eq!(read_at(&dir).unwrap().history, f.history);
    }

    #[test]
    fn a_rounds_file_that_cant_be_read_is_set_aside() {
        let t = Temp::new();
        let f = busy("a");
        let dir = dir(Some(&t.0), "p", "a");
        save_at(&dir, &f).unwrap();
        std::fs::write(dir.join(ROUNDS), "[ not json").unwrap();
        let back = read_at(&dir).unwrap();
        assert!(back.review.as_ref().unwrap().rounds.is_empty());
        assert!(!dir.join(ROUNDS).exists());
        let kept = (std::fs::read_dir(&dir).unwrap().filter_map(Result::ok)).any(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("review-rounds.json.bad-")
        });
        assert!(kept);
    }

    #[test]
    fn a_record_read_elsewhere_keeps_its_history() {
        use crate::feature::remote::{Paths, Side};
        let f = busy("a");
        let there = Side {
            home: "/home/ana".into(),
            data: "/home/ana/.local/share/gerdr".into(),
        };
        let here = Side {
            home: "/home/lg".into(),
            data: "/home/lg/.local/share/gerdr".into(),
        };
        let paths = Paths::new(&there, &here, &BTreeMap::new(), Some(&f));
        let shown = crate::feature::view::rebased(&f, &paths, "m1").unwrap();
        assert_eq!((&shown.history, &shown.review), (&f.history, &f.review));
        let taken = crate::feature::lease::taken(&f, &paths, "m1", "ana", "me", false, 9).unwrap();
        // Its own event added after.
        assert_eq!(taken.history[..f.history.len()], f.history);
        assert_eq!(taken.history.len(), f.history.len() + 1);
        assert_eq!(taken.review, f.review);
    }

    #[test]
    fn sides_stamp_changes_with_the_files() {
        let t = Temp::new();
        let mut f = busy("a");
        let dir = dir(Some(&t.0), "p", "a");
        assert_eq!(sides_stamp(&dir), Stamp::default());
        save_at(&dir, &f).unwrap();
        let before = sides_stamp(&dir);
        f.log(9, "Plan ticked");
        save_at(&dir, &f).unwrap();
        assert_ne!(sides_stamp(&dir), before);
    }
}

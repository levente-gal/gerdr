//! Where features are kept: `<data>/features/<project>/<slug>/feature.json`,
//! beside the stage files later increments write there (`brief.md`,
//! `plan.md`…). `<data>` is gerdr's data folder (`projects::data_dir`), or
//! a folder given in tests.

use std::{
    collections::BTreeMap,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::RwLock,
};

use super::{Feature, VERSION};

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

/// Every feature of project `project`, open or not, oldest first. A record
/// that can't be read is set aside (`feature.json.bad-<time>`, like the
/// other stores do) and logged, and the rest still load. A folder that
/// can't be listed is an error.
pub fn load_all(root: Option<&Path>, project: &str) -> Result<Vec<Feature>, String> {
    let folder = project_dir(root, project);
    let entries = match std::fs::read_dir(&folder) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(format!("couldn't list {}: {err}", folder.display())),
    };
    let mut features = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| format!("couldn't list {}: {err}", folder.display()))?;
        let path = entry.path().join(FILE);
        let raw = match std::fs::read(&path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == ErrorKind::NotFound => continue,
            Err(err) if err.kind() == ErrorKind::NotADirectory => continue,
            Err(err) => {
                tracing::warn!("reading {}: {err}", path.display());
                crate::projects::set_aside(&path);
                continue;
            }
        };
        match serde_json::from_slice::<Feature>(&raw) {
            Ok(mut feature) => {
                // Unsaved, it is only brought up to date again next time.
                if feature.migrate(super::now())
                    && let Err(err) = save(root, &feature)
                {
                    tracing::warn!("saving {} in its new format: {err}", path.display());
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
    Ok(features)
}

/// The record's file name in a feature's folder.
pub const FILE: &str = "feature.json";

/// Save `feature`'s record, atomically (`projects::write_atomic`).
pub fn save(root: Option<&Path>, feature: &Feature) -> std::io::Result<()> {
    save_at(&dir(root, &feature.project, &feature.slug), feature)
}

/// Save `feature`'s record in folder `dir`, as `save` does: for a feature that
/// `dir` would still find elsewhere.
pub fn save_at(dir: &Path, feature: &Feature) -> std::io::Result<()> {
    let path = dir.join(FILE);
    let mut feature = feature.clone();
    feature.version = VERSION;
    let bytes = serde_json::to_vec_pretty(&feature).map_err(std::io::Error::other)?;
    crate::projects::write_atomic(&path, &bytes)
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
}

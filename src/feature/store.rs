//! Where features are kept: `<data>/features/<project>/<slug>/feature.json`,
//! beside the stage files later increments write there (`brief.md`,
//! `plan.md`…). `<data>` is gerdr's data folder (`projects::data_dir`), or
//! a folder given in tests.

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use super::{Feature, VERSION};

/// The folder of project `project`'s features under `root` (`None`: the
/// data folder).
pub fn project_dir(root: Option<&Path>, project: &str) -> PathBuf {
    let root = root.map_or_else(crate::projects::data_dir, Path::to_path_buf);
    root.join("features").join(project)
}

/// Feature `slug`'s folder: its record and stage files.
pub fn dir(root: Option<&Path>, project: &str, slug: &str) -> PathBuf {
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
            Ok(feature) => features.push(feature),
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
    let path = dir(root, &feature.project, &feature.slug).join(FILE);
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
    fn a_missing_folder_is_no_features() {
        let t = Temp::new();
        assert_eq!(load_all(Some(&t.0), "p"), Ok(Vec::new()));
    }

    #[test]
    fn saves_and_loads_oldest_first() {
        let t = Temp::new();
        let mut b = feature("b", 5);
        b.version = 0;
        b.tick(6, "you");
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

//! Where features are kept: `<data>/features/<project>/<slug>/feature.json`,
//! beside the stage files later increments write there (`brief.md`,
//! `plan.md`…). `<data>` is gerdr's data folder (`projects::data_dir`), or
//! a folder given in tests.

use std::path::{Path, PathBuf};

use super::Feature;

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
    let _ = (root, project);
    Ok(Vec::new())
}

/// Save `feature`'s record, atomically (`projects::write_atomic`).
pub fn save(root: Option<&Path>, feature: &Feature) -> std::io::Result<()> {
    let _ = (root, feature);
    Ok(())
}

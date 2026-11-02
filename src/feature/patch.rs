//! A feature's patch-id: one id for the change it makes over all its
//! repositories, which stays the same while the change does (a rebase onto
//! a moved base keeps it) and differs once the code does.
//!
//! Per touched repository, `git diff <merge-base> HEAD | git patch-id
//! --stable` gives the id of its committed change against its base; the
//! feature's patch-id is `git hash-object` of the lines `<repo> <id>`, in
//! repository name order. Only commits count: uncommitted changes are
//! reported apart (`Reading::dirty`), and a bound stage can't be marked
//! while there are any.
//!
//! Runs git: call it off the UI thread (checklist D3).

use std::collections::BTreeMap;

use super::{
    Feature, Repo,
    layout::{base_ref, git, git_bytes, shown},
};

/// What reading a feature's worktrees found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reading {
    /// The feature's patch-id: 40 hex digits. The same for a feature
    /// without commits as for any other one without.
    pub patch: String,
    /// Touched repositories with uncommitted changes (untracked files
    /// count, ignored ones don't), by name.
    pub dirty: Vec<String>,
    /// Repositories the feature doesn't touch whose worktree has changes,
    /// by name: those wouldn't ship.
    pub stray: Vec<String>,
    /// Per touched repository, its commits not in its base.
    pub commits: BTreeMap<String, usize>,
}

/// Read `feature`'s worktrees: its patch-id, and what isn't committed. An
/// error says which repository couldn't be read and why.
///
/// A touched repository without a change against its base adds nothing to
/// the patch-id, so the id of a feature without commits is always the
/// same.
pub fn read(feature: &Feature) -> Result<Reading, String> {
    let mut reading = Reading::default();
    let mut repos: Vec<&Repo> = feature.repos.iter().collect();
    repos.sort_by(|a, b| a.name.cmp(&b.name));
    let mut lines = String::new();
    for repo in repos {
        let name = &repo.name;
        if !repo.touched {
            if repo.worktree.is_dir() && changed(repo)? {
                reading.stray.push(name.clone());
            }
            continue;
        }
        if !repo.worktree.is_dir() {
            return Err(format!("{name}: {} isn't there", shown(&repo.worktree)));
        }
        let (id, commits) = committed(repo)?;
        if !id.is_empty() {
            lines.push_str(&format!("{name} {id}\n"));
        }
        reading.commits.insert(name.clone(), commits);
        if changed(repo)? {
            reading.dirty.push(name.clone());
        }
    }
    let hash = git_bytes(
        &feature.root,
        &["hash-object", "--stdin"],
        Some(lines.into_bytes()),
    )
    .map_err(|err| format!("couldn't hash the feature's change: {err}"))?;
    reading.patch = String::from_utf8_lossy(&hash).trim().to_owned();
    Ok(reading)
}

/// The patch-id of `repo`'s committed change against its base (empty when
/// it changes nothing), and its commits not in the base.
fn committed(repo: &Repo) -> Result<(String, usize), String> {
    let name = &repo.name;
    let dir = &repo.worktree;
    let base = base_ref(repo);
    let mb = git(dir, &["merge-base", &base, "HEAD"])
        .map_err(|err| format!("{name}: couldn't find where it left {}: {err}", repo.base))?;
    let mb = mb.trim();
    let diff = git_bytes(
        dir,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--binary",
            mb,
            "HEAD",
        ],
        None,
    )
    .map_err(|err| format!("{name}: couldn't read its change: {err}"))?;
    let id = if diff.is_empty() {
        String::new()
    } else {
        let out = git_bytes(dir, &["patch-id", "--stable"], Some(diff))
            .map_err(|err| format!("{name}: couldn't read its change: {err}"))?;
        String::from_utf8_lossy(&out)
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_owned()
    };
    let range = format!("{mb}..HEAD");
    let commits = git(dir, &["rev-list", "--count", &range])
        .map_err(|err| format!("{name}: couldn't count its commits: {err}"))?
        .trim()
        .parse()
        .unwrap_or(0);
    Ok((id, commits))
}

/// Whether `repo`'s worktree has uncommitted changes, untracked files
/// included.
fn changed(repo: &Repo) -> Result<bool, String> {
    git(&repo.worktree, &["status", "--porcelain"])
        .map(|out| !out.trim().is_empty())
        .map_err(|err| format!("{}: couldn't tell what it has: {err}", repo.name))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::feature::tests::{Temp, commit, feature_over, run};

    fn wt(f: &Feature, name: &str) -> PathBuf {
        f.repos
            .iter()
            .find(|r| r.name == name)
            .unwrap()
            .worktree
            .clone()
    }

    #[test]
    fn the_patch_holds_over_a_rebase_and_moves_with_a_commit() {
        let t = Temp::new();
        let f = feature_over(&t, &["api", "web"], &["docs"]);
        let empty = read(&f).unwrap();
        assert_eq!(empty.patch.len(), 40);
        assert_eq!(empty.commits["api"], 0);
        assert!(empty.dirty.is_empty() && empty.stray.is_empty());

        commit(&wt(&f, "api"), "login", "log in");
        let one = read(&f).unwrap();
        assert_ne!(one.patch, empty.patch);
        assert_eq!(one.commits["api"], 1);
        assert_eq!(one.commits["web"], 0);

        // The base moves on and the branch is rebased onto it.
        let api = &f.repos[0].main;
        commit(api, "other", "elsewhere");
        run(&wt(&f, "api"), &["rebase", "-q", "main"]);
        assert_eq!(read(&f).unwrap().patch, one.patch);

        // Uncommitted changes and untracked files don't change it.
        std::fs::write(wt(&f, "api").join("login"), "changed").unwrap();
        std::fs::write(wt(&f, "web").join("new"), "x").unwrap();
        let dirty = read(&f).unwrap();
        assert_eq!(dirty.patch, one.patch);
        assert_eq!(dirty.dirty, ["api", "web"]);

        run(&wt(&f, "api"), &["commit", "-qam", "more"]);
        let two = read(&f).unwrap();
        assert_ne!(two.patch, one.patch);
        assert_eq!(two.commits["api"], 2);
        assert_eq!(two.dirty, ["web"]);
    }

    #[test]
    fn changes_where_the_feature_doesnt_go_are_stray() {
        let t = Temp::new();
        let f = feature_over(&t, &["api", "docs"], &["docs"]);
        assert!(read(&f).unwrap().stray.is_empty());
        std::fs::write(wt(&f, "docs").join("a"), "changed").unwrap();
        let r = read(&f).unwrap();
        assert_eq!(r.stray, ["docs"]);
        assert!(r.dirty.is_empty());
        assert!(!r.commits.contains_key("docs"));

        std::fs::remove_dir_all(wt(&f, "api")).unwrap();
        let err = read(&f).unwrap_err();
        assert!(err.starts_with("api: "), "{err}");
    }
}

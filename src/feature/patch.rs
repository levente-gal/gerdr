//! A feature's patch-id: one id for the change it makes over all its
//! repositories, which stays the same while the change does (a rebase onto
//! a moved base keeps it) and differs once the code does.
//!
//! Per touched repository, `git diff <merge-base> HEAD | git patch-id
//! --stable` gives the id of its committed change against its base; the
//! feature's patch-id is `git hash-object` of the lines `<repo> <id>`, in
//! `Repo::key` order (a home repository's key is its name; a linked
//! project's are `<project>:<name>`, so they span every project). Only commits count: uncommitted changes are
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
    /// count, ignored ones don't), by `Repo::key`.
    pub dirty: Vec<String>,
    /// Repositories the feature doesn't touch whose worktree has changes,
    /// by `Repo::key`: those wouldn't ship.
    pub stray: Vec<String>,
    /// Per touched repository by `Repo::key`, its commits not in its base.
    pub commits: BTreeMap<String, usize>,
    /// Per touched repository by `Repo::key`, its HEAD: where a fix round's
    /// diff starts and ends (`fix_round::Trail`).
    pub heads: BTreeMap<String, String>,
    /// Unix seconds it was read, before the first git call.
    pub at: u64,
    /// Per touched repository by `Repo::key` whose code was built on another
    /// branch than its base (`sits_on`): that branch and the commit it is at.
    pub sits_on: BTreeMap<String, (String, String)>,
}

/// Read `feature`'s worktrees: its patch-id, and what isn't committed. An
/// error says which repository couldn't be read and why.
///
/// A touched repository without a change against its base adds nothing to
/// the patch-id, so the id of a feature without commits is always the
/// same.
pub fn read(feature: &Feature) -> Result<Reading, String> {
    let mut reading = Reading {
        at: super::now(),
        ..Reading::default()
    };
    let mut lines = String::new();
    for repo in feature.repos_by_key() {
        let name = &repo.key();
        if !repo.touched {
            if repo.worktree.is_dir() && changed(repo)? {
                reading.stray.push(name.clone());
            }
            continue;
        }
        if !repo.worktree.is_dir() {
            return Err(format!("{name}: {} isn't there", shown(&repo.worktree)));
        }
        let (id, commits, mb) = committed(repo)?;
        if !id.is_empty() {
            lines.push_str(&format!("{name} {id}\n"));
        }
        reading.commits.insert(name.clone(), commits);
        if let Some(found) = sits_on(repo, &mb)? {
            reading.sits_on.insert(name.clone(), found);
        }
        let head = git(&repo.worktree, &["rev-parse", "HEAD"])
            .map_err(|err| format!("{name}: couldn't read its HEAD: {err}"))?;
        reading.heads.insert(name.clone(), head.trim().to_owned());
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

/// The patch-id of the change from `from` to `to` in `dir`: empty when it
/// changes nothing.
pub(super) fn diff_id(dir: &std::path::Path, from: &str, to: &str) -> Result<String, String> {
    let args = [
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--binary",
        from,
        to,
    ];
    let diff = git_bytes(dir, &args, None)?;
    if diff.is_empty() {
        return Ok(String::new());
    }
    let out = git_bytes(dir, &["patch-id", "--stable"], Some(diff))?;
    Ok(String::from_utf8_lossy(&out)
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_owned())
}

/// The patch-id of `repo`'s committed change against its base (empty when
/// it changes nothing), its commits not in the base, and where it left the
/// base.
fn committed(repo: &Repo) -> Result<(String, usize, String), String> {
    let name = &repo.key();
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
    Ok((id, commits, mb.to_owned()))
}

/// The branch `repo`'s code was built on in place of its base, and the
/// commit it is at: a branch in the worktree's history past `mb`, where it
/// left the base, that isn't the base gone further. Of those, the one
/// furthest from `mb`, as the branches merged into it count too; the
/// feature's own branch, pushed or not, doesn't count.
// shortcut: a copy of the feature's own branch under another name counts as
// the furthest, upgrade if one shows up in practice.
fn sits_on(repo: &Repo, mb: &str) -> Result<Option<(String, String)>, String> {
    let name = repo.key();
    let base = base_ref(repo);
    let refs = git(
        &repo.worktree,
        &[
            "for-each-ref",
            "--format=%(objectname) %(refname:short)",
            "--merged=HEAD",
            &format!("--no-merged={mb}"),
            &format!("--no-contains={base}"),
            "refs/heads",
            "refs/remotes",
        ],
    )
    .map_err(|err| format!("{name}: couldn't list the branches it holds: {err}"))?;
    let own = repo.branch.as_deref().unwrap_or_default();
    let mut furthest: Option<(usize, String, String)> = None;
    for (oid, branch) in refs.lines().filter_map(|l| l.split_once(' ')) {
        let ours = branch == own || branch.ends_with(&format!("/{own}"));
        if ours || branch.ends_with("/HEAD") {
            continue;
        }
        let past = git(
            &repo.worktree,
            &["rev-list", "--count", &format!("{mb}..{oid}")],
        )
        .map_err(|err| format!("{name}: couldn't count the commits of {branch}: {err}"))?;
        let past = past.trim().parse().unwrap_or(0);
        if furthest.as_ref().is_none_or(|(n, _, _)| past > *n) {
            furthest = Some((past, branch.to_owned(), oid.to_owned()));
        }
    }
    Ok(furthest.map(|(_, branch, oid)| (branch, oid)))
}

/// Whether `repo`'s worktree has uncommitted changes, untracked files
/// included.
fn changed(repo: &Repo) -> Result<bool, String> {
    git(&repo.worktree, &["status", "--porcelain"])
        .map(|out| !out.trim().is_empty())
        .map_err(|err| format!("{}: couldn't tell what it has: {err}", repo.key()))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::feature::tests::{Temp, commit, feature_over, link_over, run};

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
    fn a_linked_projects_repos_count_by_their_keys() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api"], &[]);
        let alone = read(&f).unwrap();
        link_over(&t, &mut f, "other", &["api", "docs"], &["docs"]);
        let linked = read(&f).unwrap();
        // Linked repositories without commits change nothing.
        assert_eq!(linked.patch, alone.patch);
        assert_eq!(
            linked.commits.keys().collect::<Vec<_>>(),
            ["api", "other:api"]
        );
        commit(&f.repos[1].worktree.clone(), "x", "linked change");
        let one = read(&f).unwrap();
        assert_ne!(one.patch, alone.patch);
        assert_eq!(one.commits["other:api"], 1);
        assert_eq!(one.commits["api"], 0);
        // The same change in the home repository is another patch.
        let u = Temp::new();
        let g = feature_over(&u, &["api"], &[]);
        commit(&g.repos[0].worktree.clone(), "x", "linked change");
        assert_ne!(read(&g).unwrap().patch, one.patch);
        std::fs::write(f.repos[2].worktree.join("a"), "changed").unwrap();
        std::fs::write(f.repos[1].worktree.join("b"), "new").unwrap();
        let r = read(&f).unwrap();
        assert_eq!(r.stray, ["other:docs"]);
        assert_eq!(r.dirty, ["other:api"]);
    }

    #[test]
    fn a_branch_built_on_another_than_its_base_says_which() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api"], &[]);
        let (main, w) = (f.repos[0].main.clone(), wt(&f, "api"));
        commit(&w, "login", "log in");
        // Its base going on, a pushed copy of its own branch, a backup of it:
        // none of them is another base.
        commit(&main, "other", "elsewhere");
        run(&w, &["rebase", "-q", "main"]);
        run(&main, &["update-ref", "refs/remotes/origin/main", "main"]);
        run(&w, &["update-ref", "refs/remotes/origin/feature/f", "HEAD"]);
        assert!(read(&f).unwrap().sits_on.is_empty());

        // Rebased onto another branch: that one, not one merged into it.
        run(&main, &["checkout", "-qb", "port", "main~1"]);
        commit(&main, "fix", "fix work");
        run(&main, &["branch", "fix"]);
        commit(&main, "port", "port work");
        run(&main, &["checkout", "-q", "main"]);
        run(&w, &["rebase", "-q", "--onto", "port", "main"]);
        let port = run(&main, &["rev-parse", "port"]).trim().to_owned();
        let r = read(&f).unwrap();
        assert_eq!(r.sits_on["api"], ("port".to_owned(), port.clone()));

        // With that branch its base, all is well.
        (f.repos[0].base, f.repos[0].base_commit) = ("port".into(), port);
        assert!(read(&f).unwrap().sits_on.is_empty());
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

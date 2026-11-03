//! Shipping a feature, and what stops it.
//!
//! `probe` reads a feature for the panel, off the UI thread: its patch-id,
//! its guards, and when Ship is its stage, what blocks shipping or else
//! what shipping does. Shipping then goes one of two ways, per repository
//! (`ShipMode`):
//!
//! - pull requests (`open_prs`): for each touched repository with commits,
//!   in name order, allow the push (`hook::allow`), push `feature/<slug>`
//!   with its upstream, guard it again, and open a pull request into its
//!   base with `gh pr create`, or keep the one open for the branch already.
//!   With more than one, every body links the others (`gh pr edit`).
//! - squash (`squash_command`): a command for a tab in the main worktree
//!   that squashes the branch into the base and commits, the message
//!   prefilled with the title, for the user to edit. The feature is done
//!   once every branch reads as merged (`Probe::merged`).
//!
//! Runs git and gh: call it off the UI thread (checklist D3).

use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use super::{
    Feature, Pr, Repo, ShipMode, Stage, Status,
    hook::{self, Guard},
    layout::{git, shown},
    patch::{self, Reading},
};
use crate::{git as gitinfo, git_ops};

/// One step of shipping, in the order they run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Push `feature/<slug>` of `repo` to origin.
    Push { repo: String },
    /// Open a pull request of `repo` into `base`.
    OpenPr { repo: String, base: String },
    /// Keep `repo`'s pull request open for the branch already: the push
    /// updates it.
    KeepPr { repo: String, url: String },
    /// Link the pull requests to each other in their bodies.
    LinkPrs,
    /// Squash `feature/<slug>` of `repo` into `base` in its main worktree.
    Squash { repo: String, base: String },
}

impl Action {
    /// In words, for the Ship sheet: "Push services-api", "Open a pull
    /// request into dev for services-api". `Action` doesn't carry the
    /// branch: the sheet says it once, above the list.
    pub fn label(&self) -> String {
        match self {
            Self::Push { repo } => format!("Push {repo}"),
            Self::OpenPr { repo, base } => {
                format!("Open a pull request into {base} for {repo}")
            }
            Self::KeepPr { repo, url } => {
                format!("Keep the pull request open for {repo}: {url}")
            }
            Self::LinkPrs => "Link the pull requests to each other".into(),
            Self::Squash { repo, base } => format!("Squash the feature into {base} in {repo}"),
        }
    }
}

/// Whether a feature can ship now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShipCheck {
    /// What stops it, one line each ("services-api has uncommitted
    /// changes"); empty when it can ship.
    pub blockers: Vec<String>,
    /// What shipping does, in order; empty while blocked.
    pub actions: Vec<Action>,
    /// What is worth knowing but stops nothing ("aft-backend has no
    /// commits: left out").
    pub notes: Vec<String>,
}

/// What `probe` read of a feature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    /// Its patch-id and uncommitted changes, or why they couldn't be read.
    pub reading: Result<Reading, String>,
    /// Per touched repository, by name: whether pushes from its worktree
    /// are guarded (after making sure of the hook there).
    pub guards: BTreeMap<String, Guard>,
    /// When Ship is its current stage: whether it can ship.
    pub ship: Option<ShipCheck>,
    /// When a squash started (`Shipping::squash_at`): whether every
    /// touched repository with commits reads as merged into its base.
    pub merged: bool,
}

/// How `repo` of `feature` ships: `mode` when the project sets one, else
/// a pull request when its `origin` is on GitHub, else a squash.
pub fn mode_of(feature: &Feature, repo: &str, mode: Option<ShipMode>) -> ShipMode {
    if let Some(mode) = mode {
        return mode;
    }
    let on_github = feature
        .repos
        .iter()
        .find(|r| r.name == repo)
        .and_then(|r| gitinfo::web_url(&r.main))
        .is_some_and(|url| url.starts_with("https://github.com/"));
    if on_github {
        ShipMode::Pr
    } else {
        ShipMode::Squash
    }
}

/// Read `feature` for the panel (see `Probe`); `mode` is the project's
/// setting, if any. Makes sure of every touched repository's hook and
/// guard file on the way (`hook::ensure`, `hook::guard_if_missing`).
pub fn probe(feature: &Feature, mode: Option<ShipMode>) -> Probe {
    let reading = patch::read(feature);
    let branch = feature.branch();
    let guards = feature
        .touched()
        .map(|repo| (repo.name.clone(), guard_of(repo, &branch)))
        .collect();
    let ship = (feature.status == Status::Open && feature.current() == Some(Stage::Ship))
        .then(|| check(feature, &reading, mode));
    let merged =
        feature.shipping.squash_at.is_some() && reading.as_ref().is_ok_and(|r| merged(feature, r));
    Probe {
        reading,
        guards,
        ship,
        merged,
    }
}

/// Whether pushes of `branch` from `repo`'s worktree are guarded, making
/// sure of its hook and guard file.
fn guard_of(repo: &Repo, branch: &str) -> Guard {
    if !repo.worktree.is_dir() {
        return Guard::Off(format!(
            "its worktree {} isn't there",
            shown(&repo.worktree)
        ));
    }
    if let Err(err) = hook::guard_if_missing(&repo.main, branch) {
        return Guard::Off(err);
    }
    hook::ensure(&repo.worktree)
}

/// Whether every touched repository of `feature` with commits has its
/// branch merged into its base.
fn merged(feature: &Feature, reading: &Reading) -> bool {
    let branch = feature.branch();
    let mut shipped = feature
        .touched()
        .filter(|r| reading.commits.get(&r.name).is_some_and(|n| *n > 0))
        .peekable();
    if shipped.peek().is_none() {
        return false;
    }
    shipped.all(|repo| {
        // As a commit: `git::merged` keeps its answers per pair of names.
        let spec = format!("{}^{{commit}}", repo.base);
        let Ok(base) = git(&repo.main, &["rev-parse", "--verify", "--quiet", &spec]) else {
            return false;
        };
        gitinfo::merged(&repo.main, &branch, &[base.trim()])
    })
}

/// Whether `feature`, at Ship, can ship as `reading` found it.
fn check(
    feature: &Feature,
    reading: &Result<Reading, String>,
    mode: Option<ShipMode>,
) -> ShipCheck {
    let mut out = ShipCheck::default();
    for stage in Stage::ALL.into_iter().filter(|s| *s < Stage::Ship) {
        if !feature.is_done(stage) {
            out.blockers.push(format!("{} isn't done", stage.label()));
        }
    }
    let open = feature.review.as_ref().map_or(0, |r| r.open());
    match open {
        0 => {}
        1 => out.blockers.push("1 AI review finding open".into()),
        n => out.blockers.push(format!("{n} AI review findings open")),
    }
    let reading = match reading {
        Ok(reading) => reading,
        Err(err) => {
            out.blockers.push(format!("Couldn't read the code: {err}"));
            return out;
        }
    };
    if let Some(stage) = feature.stale(&reading.patch).filter(|s| *s < Stage::Ship) {
        out.blockers
            .push(format!("Code changed since {} was marked", stage.label()));
    }
    for name in &reading.dirty {
        out.blockers.push(format!("{name} has uncommitted changes"));
    }
    for name in &reading.stray {
        out.blockers.push(format!(
            "{name} has changes, but the feature doesn't change it"
        ));
    }
    let branch = feature.branch();
    let commits = |r: &Repo| reading.commits.get(&r.name).copied().unwrap_or(0);
    let shipped: Vec<&Repo> = feature.touched().filter(|r| commits(r) > 0).collect();
    if shipped.is_empty() {
        out.blockers
            .push(format!("Nothing to ship: no commits on {branch}"));
        return out;
    }
    for repo in feature.touched().filter(|r| commits(r) == 0) {
        out.notes
            .push(format!("{} has no commits: left out", repo.name));
    }
    let mut prs = Vec::new();
    let mut squashes = Vec::new();
    for repo in shipped {
        match mode_of(feature, &repo.name, mode) {
            ShipMode::Pr => {
                if git(&repo.main, &["remote", "get-url", "origin"]).is_err() {
                    out.blockers.push(format!(
                        "{} has no origin remote to open a pull request on",
                        repo.name
                    ));
                }
                prs.push(repo);
            }
            ShipMode::Squash => {
                out.blockers.extend(squash_blocker(repo, &branch));
                squashes.push(repo);
            }
        }
    }
    if !out.blockers.is_empty() {
        return out;
    }
    for repo in &prs {
        out.actions.push(Action::Push {
            repo: repo.name.clone(),
        });
        let kept = feature.shipping.prs.iter().find(|p| p.repo == repo.name);
        out.actions.push(match kept {
            Some(pr) => Action::KeepPr {
                repo: repo.name.clone(),
                url: pr.url.clone(),
            },
            None => Action::OpenPr {
                repo: repo.name.clone(),
                base: repo.base.clone(),
            },
        });
    }
    if prs.len() >= 2 {
        out.actions.push(Action::LinkPrs);
    }
    for repo in squashes {
        out.actions.push(Action::Squash {
            repo: repo.name.clone(),
            base: repo.base.clone(),
        });
    }
    out
}

/// What stops squashing `branch` into `repo`'s base in its main worktree.
fn squash_blocker(repo: &Repo, branch: &str) -> Option<String> {
    let (name, base) = (&repo.name, &repo.base);
    let on = git(&repo.main, &["symbolic-ref", "-q", "--short", "HEAD"])
        .map(|b| b.trim().to_owned())
        .unwrap_or_default();
    if on.is_empty() {
        return Some(format!(
            "{name}'s main worktree is on no branch: switch it to {base}"
        ));
    }
    if &on != base {
        return Some(format!(
            "{name}'s main worktree is on {on}: switch it to {base}"
        ));
    }
    match git(
        &repo.main,
        &["status", "--porcelain", "--untracked-files=no"],
    ) {
        Ok(status) if !status.trim().is_empty() => {
            return Some(format!(
                "{name}'s main worktree has uncommitted changes on {base}"
            ));
        }
        Ok(_) => {}
        Err(err) => {
            return Some(format!(
                "{name}: couldn't tell what its main worktree has: {err}"
            ));
        }
    }
    match git_ops::predict_merge(&repo.main, base, branch) {
        git_ops::Verdict::Clean => None,
        git_ops::Verdict::Conflicts(files) => Some(format!(
            "{name}: squashing into {base} conflicts in {}",
            files.join(", ")
        )),
        git_ops::Verdict::Blocked(why) => Some(format!(
            "{name}: couldn't tell whether squashing into {base} conflicts: {why}"
        )),
        git_ops::Verdict::Unmerged(_) => None,
    }
}

/// Run `actions` of a pull-request ship of `feature`, with `gh` the
/// program to run as gh. Returns the pull requests, or why it stopped with
/// those opened by then: running it again goes on from there.
///
/// Every push is allowed right before it (`hook::allow`) and guarded again
/// after, whether it went or not.
pub fn open_prs(
    feature: &Feature,
    actions: &[Action],
    gh: &str,
) -> Result<Vec<Pr>, (String, Vec<Pr>)> {
    let branch = feature.branch();
    let mut prs: Vec<Pr> = Vec::new();
    let repo_of = |name: &str| feature.repos.iter().find(|r| r.name == name);
    for action in actions {
        let fail = |why: String, prs: &Vec<Pr>| Err((why, prs.clone()));
        match action {
            Action::Push { repo } => {
                let Some(r) = repo_of(repo) else {
                    return fail(format!("{} has no repository {repo}", feature.title), &prs);
                };
                if let Err(why) = push(r, &branch) {
                    return fail(format!("Couldn't push {branch} of {repo}: {why}"), &prs);
                }
            }
            Action::KeepPr { repo, url } => prs.push(Pr {
                repo: repo.clone(),
                url: url.clone(),
            }),
            Action::OpenPr { repo, base } => {
                let Some(r) = repo_of(repo) else {
                    return fail(format!("{} has no repository {repo}", feature.title), &prs);
                };
                match open_pr(feature, r, base, &branch, gh) {
                    Ok(url) => prs.push(Pr {
                        repo: repo.clone(),
                        url,
                    }),
                    Err(why) => {
                        return fail(
                            format!("Couldn't open a pull request of {repo}: {why}"),
                            &prs,
                        );
                    }
                }
            }
            Action::LinkPrs => {
                if prs.len() < 2 {
                    continue;
                }
                for pr in &prs {
                    let Some(r) = repo_of(&pr.repo) else { continue };
                    let mut body = format!("{}\n\nThe others:\n", pr_body(feature));
                    for other in prs.iter().filter(|o| o.url != pr.url) {
                        body.push_str(&format!("- {}: {}\n", other.repo, other.url));
                    }
                    let args = ["pr", "edit", pr.url.as_str(), "--body", body.as_str()];
                    if let Err(why) = run_gh(gh, &r.worktree, &args) {
                        return fail(
                            format!("Couldn't link the pull request of {}: {why}", pr.repo),
                            &prs,
                        );
                    }
                }
            }
            Action::Squash { .. } => {}
        }
    }
    Ok(prs)
}

/// The body a feature's pull request starts with.
fn pr_body(feature: &Feature) -> String {
    format!("Part of “{}”.", feature.title)
}

/// Push `branch` of `repo` to origin with its upstream, allowed for its
/// HEAD alone; guarded again after.
fn push(repo: &Repo, branch: &str) -> Result<(), String> {
    let head = git(&repo.worktree, &["rev-parse", "HEAD"])?;
    hook::allow(&repo.main, branch, head.trim())?;
    let pushed = git_ops::push(&repo.worktree, Some("origin"), false);
    let guarded = hook::guard(&repo.main, branch);
    if let Err(err) = &guarded {
        tracing::warn!("guarding {branch} of {} again: {err}", repo.name);
    }
    pushed.map(|_| ()).map_err(|failed| push_why(&failed))
}

/// One line on why a push failed: git's own last line only says "failed
/// to push some refs" when a hook refused it, so the first line of what
/// was printed then.
fn push_why(failed: &git_ops::Failed) -> String {
    if !failed.why.starts_with("failed to push some refs") {
        return failed.why.clone();
    }
    let output = failed.log.last().map(|r| r.output.as_str()).unwrap_or("");
    output
        .lines()
        .map(str::trim)
        .find(|l| {
            !l.is_empty()
                && !l.starts_with("To ")
                && !l.starts_with("error: failed to push")
                && !l.starts_with("hint:")
        })
        .unwrap_or(&failed.why)
        .to_owned()
}

/// The open pull request of `branch` of `repo`, or a new one into `base`:
/// its address.
fn open_pr(
    feature: &Feature,
    repo: &Repo,
    base: &str,
    branch: &str,
    gh: &str,
) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    struct View {
        url: String,
        state: String,
    }
    // An error here is mostly "no pull requests found": creating one says
    // what is wrong otherwise.
    if let Ok(out) = run_gh(
        gh,
        &repo.worktree,
        &["pr", "view", branch, "--json", "url,state"],
    ) && let Ok(view) = serde_json::from_str::<View>(&out)
        && view.state == "OPEN"
    {
        return Ok(view.url);
    }
    let body = pr_body(feature);
    let args = [
        "pr",
        "create",
        "--base",
        base,
        "--head",
        branch,
        "--title",
        feature.title.as_str(),
        "--body",
        body.as_str(),
    ];
    let out = run_gh(gh, &repo.worktree, &args)?;
    out.lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| "gh pr create printed no address".to_owned())
}

/// How long a gh command may take.
const GH_TIMEOUT: Duration = Duration::from_secs(120);

/// What `gh args`, run in `dir`, prints, or one line on why it failed.
fn run_gh(gh: &str, dir: &Path, args: &[&str]) -> Result<String, String> {
    let what = args.get(..2).unwrap_or(args).join(" ");
    let mut command = Command::new(gh);
    // The real gh, not a test's.
    if gh == "gh" {
        crate::ci::signed_in(&mut command)?;
    }
    let mut child = command
        .args(args)
        .current_dir(dir)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => "gh is not installed".to_owned(),
            _ => format!("couldn't run gh: {err}"),
        })?;
    let read = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut pipe) = pipe {
                // Cut short when gh dies: what came is what there is.
                let _ = pipe.read_to_string(&mut text);
            }
            text
        })
    };
    let stdout = read(child.stdout.take().map(|p| Box::new(p) as _));
    let stderr = read(child.stderr.take().map(|p| Box::new(p) as _));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if start.elapsed() < GH_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) | Err(_) => {
                // Killed: a gh gone already is fine.
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    match status {
        None => Err(format!(
            "gh {what} took longer than {} s",
            GH_TIMEOUT.as_secs()
        )),
        Some(status) if status.success() => Ok(stdout),
        Some(_) => {
            let why = stderr.lines().map(str::trim).find(|l| !l.is_empty());
            Err(why.map_or_else(|| format!("gh {what} failed"), str::to_owned))
        }
    }
}

/// The folder and the shell command of a squash ship of `feature`'s
/// `actions`, for a tab: every `Squash` action's repository in turn.
pub fn squash_command(feature: &Feature, actions: &[Action]) -> (PathBuf, String) {
    let branch = feature.branch();
    let mains: Vec<&Path> = actions
        .iter()
        .filter_map(|a| match a {
            Action::Squash { repo, .. } => feature.repos.iter().find(|r| &r.name == repo),
            _ => None,
        })
        .map(|r| r.main.as_path())
        .collect();
    let Some(first) = mains.first() else {
        return (feature.root.clone(), String::new());
    };
    let command = mains
        .iter()
        .map(|main| {
            format!(
                "cd {} && git merge --squash {} && git commit -e -m {}",
                quote(&main.to_string_lossy()),
                quote(&branch),
                quote(&feature.title)
            )
        })
        .collect::<Vec<_>>()
        .join(" && ");
    (first.to_path_buf(), command)
}

/// `text` quoted for a POSIX shell.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::feature::tests::{Temp, commit, feature_over, run, try_run};

    /// Give each of `f`'s repositories a bare origin with main on it.
    fn origins(t: &Temp, f: &Feature) {
        for repo in &f.repos {
            let origin = t.0.join(format!("{}.git", repo.name));
            std::fs::create_dir_all(&origin).unwrap();
            run(&origin, &["init", "-q", "--bare", "-b", "main"]);
            run(
                &repo.main,
                &["remote", "add", "origin", origin.to_str().unwrap()],
            );
            run(&repo.main, &["push", "-q", "origin", "main"]);
        }
    }

    /// Mark every stage before Ship on `f`'s code as it is now.
    fn mark_all(f: &mut Feature) {
        let patch = patch::read(f).unwrap().patch;
        while f.current() != Some(Stage::Ship) {
            f.tick(1, "you", Some(&patch));
        }
    }

    fn wt(f: &Feature, name: &str) -> PathBuf {
        f.repos
            .iter()
            .find(|r| r.name == name)
            .unwrap()
            .worktree
            .clone()
    }

    /// A fake gh in `t` logging its arguments to `gh.log`: `pr view`
    /// finds a pull request only once `open` is there, `pr create`
    /// prints an address naming the folder it ran in, or fails there when
    /// `refuse` names that folder.
    fn fake_gh(t: &Temp) -> String {
        let gh = t.0.join("gh");
        let script = format!(
            r#"#!/bin/sh
dir={dir}
printf '%s|' "$@" >> "$dir/gh.log"
echo >> "$dir/gh.log"
case "$1 $2" in
"pr view")
	if [ -e "$dir/open" ]; then
		echo '{{"url":"https://github.com/o/r/pull/9","state":"OPEN"}}'
	else
		echo "no pull requests found for branch \"$3\"" >&2
		exit 1
	fi ;;
"pr create")
	[ "$(cat "$dir/refuse" 2>/dev/null)" = "$(basename "$PWD")" ] && {{ echo "GraphQL: refused" >&2; echo more >&2; exit 1; }}
	echo "Creating pull request"
	echo "https://github.com/o/$(basename "$PWD")/pull/1" ;;
esac
"#,
            dir = t.0.display()
        );
        std::fs::write(&gh, script).unwrap();
        std::fs::set_permissions(&gh, PermissionsExt::from_mode(0o755)).unwrap();
        gh.to_string_lossy().into_owned()
    }

    fn gh_log(t: &Temp) -> String {
        std::fs::read_to_string(t.0.join("gh.log")).unwrap_or_default()
    }

    #[test]
    fn one_repository_ships_as_a_pull_request_and_again_keeps_it() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api"], &[]);
        origins(&t, &f);
        commit(&wt(&f, "api"), "login", "log in");
        mark_all(&mut f);
        let gh = fake_gh(&t);

        let p = probe(&f, Some(ShipMode::Pr));
        assert_eq!(p.guards["api"], Guard::On);
        assert!(hook::guarded(&f.repos[0].main, "feature/f"));
        let check = p.ship.unwrap();
        assert!(check.blockers.is_empty(), "{:?}", check.blockers);
        assert_eq!(
            check.actions,
            [
                Action::Push { repo: "api".into() },
                Action::OpenPr {
                    repo: "api".into(),
                    base: "main".into()
                }
            ]
        );
        // Guarded: a push by hand is refused.
        let err = try_run(&wt(&f, "api"), &["push", "-q", "origin", "HEAD"]).unwrap_err();
        assert!(err.contains("gerdr"), "{err}");

        // The hook that was there refuses it: its words, guarded after.
        let chained = f.repos[0].main.join(".git/hooks").join(hook::CHAINED);
        std::fs::write(&chained, "#!/bin/sh\necho 'lint failed' >&2\nexit 1\n").unwrap();
        std::fs::set_permissions(&chained, PermissionsExt::from_mode(0o755)).unwrap();
        let (why, so_far) = open_prs(&f, &check.actions, &gh).unwrap_err();
        assert_eq!(why, "Couldn't push feature/f of api: lint failed");
        assert!(so_far.is_empty());
        let guard_file = f.repos[0].main.join(".git/gerdr/guard/feature/f");
        assert_eq!(std::fs::read_to_string(&guard_file).unwrap(), "");
        std::fs::remove_file(&chained).unwrap();

        let prs = open_prs(&f, &check.actions, &gh).unwrap();
        assert_eq!(
            prs,
            [Pr {
                repo: "api".into(),
                url: "https://github.com/o/api/pull/1".into()
            }]
        );
        let origin = t.0.join("api.git");
        assert_eq!(
            run(&origin, &["rev-parse", "feature/f"]),
            run(&wt(&f, "api"), &["rev-parse", "HEAD"])
        );
        // Guarded again after.
        assert_eq!(std::fs::read_to_string(&guard_file).unwrap(), "");
        let log = gh_log(&t);
        assert!(log.contains("pr|view|feature/f|--json|url,state|"), "{log}");
        assert!(
            log.contains(
                "pr|create|--base|main|--head|feature/f|--title|Log in|--body|Part of “Log in”.|"
            ),
            "{log}"
        );

        // Shipped again: the pull request is kept, the push updates it.
        f.shipping.prs = prs.clone();
        let check = probe(&f, Some(ShipMode::Pr)).ship.unwrap();
        assert_eq!(
            check.actions[1],
            Action::KeepPr {
                repo: "api".into(),
                url: prs[0].url.clone()
            }
        );
        std::fs::remove_file(t.0.join("gh.log")).unwrap();
        assert_eq!(open_prs(&f, &check.actions, &gh).unwrap(), prs);
        assert_eq!(gh_log(&t), "");
    }

    #[test]
    fn two_repositories_link_their_pull_requests() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api", "docs", "web"], &[]);
        origins(&t, &f);
        commit(&wt(&f, "api"), "a1", "api change");
        commit(&wt(&f, "web"), "w1", "web change");
        mark_all(&mut f);
        let gh = fake_gh(&t);
        let check = probe(&f, Some(ShipMode::Pr)).ship.unwrap();
        assert_eq!(check.notes, ["docs has no commits: left out"]);
        assert_eq!(check.actions.len(), 5);
        assert_eq!(check.actions[4], Action::LinkPrs);
        assert_eq!(
            check.actions[4].label(),
            "Link the pull requests to each other"
        );

        // web's create fails: api's is kept for the run again.
        std::fs::write(t.0.join("refuse"), "web").unwrap();
        let (why, so_far) = open_prs(&f, &check.actions, &gh).unwrap_err();
        assert_eq!(why, "Couldn't open a pull request of web: GraphQL: refused");
        assert_eq!(so_far.len(), 1);
        std::fs::remove_file(t.0.join("refuse")).unwrap();

        f.shipping.prs = so_far;
        let check = probe(&f, Some(ShipMode::Pr)).ship.unwrap();
        assert!(matches!(check.actions[1], Action::KeepPr { .. }));
        let prs = open_prs(&f, &check.actions, &gh).unwrap();
        let urls: Vec<&str> = prs.iter().map(|p| p.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://github.com/o/api/pull/1",
                "https://github.com/o/web/pull/1"
            ]
        );
        let log = gh_log(&t);
        assert!(
            log.contains(
                "pr|edit|https://github.com/o/api/pull/1|--body|Part of “Log in”.\n\n\
                 The others:\n- web: https://github.com/o/web/pull/1\n|"
            ),
            "{log}"
        );
        assert!(
            log.contains("- api: https://github.com/o/api/pull/1\n|"),
            "{log}"
        );

        // One open for the branch already is used, not made again.
        std::fs::write(t.0.join("open"), "").unwrap();
        let fresh = probe(&feature_over_again(&f), Some(ShipMode::Pr))
            .ship
            .unwrap();
        let prs = open_prs(&f, &fresh.actions, &gh).unwrap();
        assert_eq!(prs[0].url, "https://github.com/o/r/pull/9");
    }

    /// `f` with no pull requests kept.
    fn feature_over_again(f: &Feature) -> Feature {
        let mut f = f.clone();
        f.shipping.prs.clear();
        f
    }

    #[test]
    fn what_stops_shipping() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api", "docs"], &["docs"]);
        origins(&t, &f);
        mark_all(&mut f);
        let blockers = |f: &Feature| probe(f, Some(ShipMode::Pr)).ship.unwrap().blockers;
        assert_eq!(blockers(&f), ["Nothing to ship: no commits on feature/f"]);

        // The marks were given on code without commits.
        commit(&wt(&f, "api"), "a1", "change");
        std::fs::write(wt(&f, "api").join("a1"), "more").unwrap();
        std::fs::write(wt(&f, "docs").join("a"), "x").unwrap();
        let check = probe(&f, Some(ShipMode::Pr)).ship.unwrap();
        assert_eq!(
            check.blockers,
            [
                "Code changed since Build was marked",
                "api has uncommitted changes",
                "docs has changes, but the feature doesn't change it",
            ]
        );
        assert!(check.actions.is_empty());

        // Not at Ship: no check.
        f.untick(Stage::Verify, 2);
        assert!(probe(&f, None).ship.is_none());
    }

    #[test]
    fn open_findings_stop_shipping() {
        use crate::feature::review_import::{AiReview, Finding, FindingState};
        let mut f = crate::features::tests::feature("a", vec![], 6);
        let finding = |state| Finding {
            state,
            ..Finding::default()
        };
        f.review = Some(AiReview {
            findings: vec![
                finding(FindingState::Open),
                finding(FindingState::Sent),
                finding(FindingState::Dismissed),
            ],
            ..AiReview::default()
        });
        let err = Err("gone".to_owned());
        let blockers = check(&f, &err, None).blockers;
        assert!(
            blockers.contains(&"2 AI review findings open".to_owned()),
            "{blockers:?}"
        );
        f.review.as_mut().unwrap().findings.truncate(1);
        let blockers = check(&f, &err, None).blockers;
        assert!(
            blockers.contains(&"1 AI review finding open".to_owned()),
            "{blockers:?}"
        );
        f.review = None;
        let blockers = check(&f, &err, None).blockers;
        assert!(
            !blockers.iter().any(|b| b.contains("AI review")),
            "{blockers:?}"
        );
    }

    #[test]
    fn a_squash_needs_its_main_worktree_on_the_base_and_clean() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api"], &[]);
        let main = f.repos[0].main.clone();
        commit(&wt(&f, "api"), "a", "feature side");
        mark_all(&mut f);
        // No origin on GitHub: a squash.
        assert_eq!(mode_of(&f, "api", None), ShipMode::Squash);
        let check = |f: &Feature| probe(f, None).ship.unwrap();
        assert_eq!(
            check(&f).actions,
            [Action::Squash {
                repo: "api".into(),
                base: "main".into()
            }]
        );

        std::fs::write(main.join("a"), "edited").unwrap();
        assert_eq!(
            check(&f).blockers,
            ["api's main worktree has uncommitted changes on main"]
        );
        run(&main, &["checkout", "-q", "--", "a"]);
        // Untracked files don't stop it.
        std::fs::write(main.join("loose"), "x").unwrap();
        assert!(check(&f).blockers.is_empty());

        run(&main, &["switch", "-q", "-c", "other"]);
        assert_eq!(
            check(&f).blockers,
            ["api's main worktree is on other: switch it to main"]
        );
        run(&main, &["switch", "-q", "main"]);

        commit(&main, "a", "main side");
        assert_eq!(
            check(&f).blockers,
            ["api: squashing into main conflicts in a"]
        );
    }

    #[test]
    fn a_squash_runs_in_a_tab_and_reads_as_merged_once_committed() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api", "web"], &[]);
        f.title = "Don't break".into();
        commit(&wt(&f, "api"), "x", "feature");
        mark_all(&mut f);
        let check = probe(&f, None).ship.unwrap();
        let (dir, command) = squash_command(&f, &check.actions);
        let main = f.repos[0].main.clone();
        assert_eq!(dir, main);
        assert_eq!(
            command,
            format!(
                "cd '{}' && git merge --squash 'feature/f' && git commit -e -m 'Don'\\''t break'",
                main.display()
            )
        );
        assert_eq!(squash_command(&f, &[]), (f.root.clone(), String::new()));

        f.shipping.squash_at = Some(5);
        assert!(!probe(&f, None).merged);
        run(&main, &["merge", "-q", "--squash", "feature/f"]);
        run(&main, &["commit", "-q", "-m", "Don't break"]);
        let p = probe(&f, None);
        assert!(p.merged);
        f.shipping.squash_at = None;
        assert!(!probe(&f, None).merged);
    }

    #[test]
    fn actions_read_as_words() {
        let api = || "api".to_owned();
        let labels: Vec<String> = [
            Action::Push { repo: api() },
            Action::OpenPr {
                repo: api(),
                base: "dev".into(),
            },
            Action::KeepPr {
                repo: api(),
                url: "https://x/1".into(),
            },
            Action::Squash {
                repo: api(),
                base: "main".into(),
            },
        ]
        .iter()
        .map(Action::label)
        .collect();
        assert_eq!(
            labels,
            [
                "Push api",
                "Open a pull request into dev for api",
                "Keep the pull request open for api: https://x/1",
                "Squash the feature into main in api",
            ]
        );
    }
}

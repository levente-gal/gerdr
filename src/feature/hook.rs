//! The `pre-push` hook guarding features' branches.
//!
//! gerdr installs one `pre-push` hook per hooks folder a feature's worktree
//! uses (`git rev-parse --git-path hooks`, which follows `core.hooksPath`).
//! It chains to a hook that was there. For each ref pushed it looks for a
//! guard file, `<git common dir>/gerdr/guard/<branch>`: none, and the push
//! goes on (a branch that isn't a gerdr feature's isn't guarded); there,
//! and the push goes on only when the commit pushed is the one the file
//! names. gerdr writes that commit right before it pushes the branch when
//! shipping (`allow`) and empties the file after (`guard`), so any other
//! push of the branch is refused with a line saying to ship it from gerdr.
//!
//! Runs git and writes in the repository's git folder: call it off the UI
//! thread (checklist D3).

use std::{
    io::ErrorKind,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use super::layout::{git, shown};

/// The line that marks a hook as gerdr's, without its version.
const MARK: &str = "# gerdr feature guard v";

/// The version of `HOOK`: bumped when the script changes, so `ensure`
/// rewrites older ones.
const HOOK_VERSION: u32 = 1;

/// The name a hook that was in the folder before gerdr's is kept under;
/// gerdr's hook runs it after its own check.
pub const CHAINED: &str = "pre-push.gerdr-chained";

/// The `pre-push` hook. A chained hook written for sh is sourced, with
/// `$0` still this hook's path: husky's stubs find the user's hook by
/// their own name (`.husky/_/pre-push` → `.husky/pre-push`).
pub const HOOK: &str = r#"#!/bin/sh
# gerdr feature guard v1
# Installed by gerdr: refuses a push of a gerdr feature's branch that gerdr
# didn't clear, then runs the hook that was here before it, kept as
# pre-push.gerdr-chained.

input=$(cat)
common=$(git rev-parse --git-common-dir) || exit 1

# Whether branch $1 may be pushed at commit $2.
gerdr_allowed() {
	guard="$common/gerdr/guard/$1"
	[ -f "$guard" ] || return 0
	want=$(tr -d ' \t\r\n' < "$guard")
	[ -n "$want" ] && [ "$want" = "$2" ] && return 0
	echo "gerdr: $1 is a gerdr feature's branch; ship it from the feature panel in gerdr" >&2
	return 1
}

while read -r local_ref local_sha remote_ref remote_sha; do
	[ -n "$local_ref" ] || continue
	# All zeros: the branch is being deleted.
	case $local_sha in *[!0]*) ;; *) continue ;; esac
	# `git push origin HEAD:x` names HEAD: the branch it is on.
	[ "$local_ref" = HEAD ] && local_ref=$(git symbolic-ref -q HEAD)
	for ref in "$remote_ref" "$local_ref"; do
		case $ref in
		refs/heads/*) gerdr_allowed "${ref#refs/heads/}" "$local_sha" || exit 1 ;;
		esac
	done
done <<EOF
$input
EOF

chained="$(dirname "$0")/pre-push.gerdr-chained"
[ -f "$chained" ] && [ -x "$chained" ] || exit 0

# Whether the chained hook is a sh script: no #! line, or one naming sh.
gerdr_is_sh() {
	line=$(head -n 1 "$1")
	case $line in '#!'*) ;; *) return 0 ;; esac
	for word in ${line#??}; do
		case ${word##*/} in
		env | -*) ;;
		sh | dash | ash) return 0 ;;
		*) return 1 ;;
		esac
	done
	return 1
}

gerdr_input() {
	[ -z "$input" ] || printf '%s\n' "$input"
}

if gerdr_is_sh "$chained"; then
	gerdr_input | sh -c 'gerdr_hook=$1; shift; . "$gerdr_hook"' "$0" "$chained" "$@"
else
	gerdr_input | "$chained" "$@"
fi
"#;

/// Whether pushes from a worktree are guarded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Guard {
    On,
    /// Not guarded, and why, in words: "its hooks folder .husky/_ isn't
    /// there yet".
    Off(String),
}

/// Make sure the hook is in the hooks folder `worktree` uses, chaining to
/// a hook that was there: installs it, updates an older one of gerdr's, or
/// leaves it. A hook folder whose `pre-push` git tracks is not touched.
pub fn ensure(worktree: &Path) -> Guard {
    match install(worktree) {
        Ok(()) => Guard::On,
        Err(why) => Guard::Off(why),
    }
}

/// `ensure`'s work: why it isn't guarded, if it isn't.
fn install(worktree: &Path) -> Result<(), String> {
    let hooks = hooks_dir(worktree)?;
    if !hooks.is_dir() {
        // git's own hooks folder only goes missing when deleted: made
        // again. One that core.hooksPath names is the tool's to make
        // (husky makes .husky/_ on install).
        let common = common_dir(worktree)?;
        if hooks != common.join("hooks") {
            return Err(format!(
                "its hooks folder {} isn't there yet",
                shown(&hooks)
            ));
        }
        std::fs::create_dir_all(&hooks)
            .map_err(|err| format!("couldn't make {}: {err}", shown(&hooks)))?;
    }
    let path = hooks.join("pre-push");
    let text = match std::fs::read(&path) {
        Ok(text) => Some(String::from_utf8_lossy(&text).into_owned()),
        Err(err) if err.kind() == ErrorKind::NotFound => None,
        Err(err) => return Err(format!("couldn't read {}: {err}", shown(&path))),
    };
    if text.is_some() && tracked(worktree, &path) {
        return Err(format!(
            "its pre-push hook {} is in git, so gerdr leaves it",
            shown(&path)
        ));
    }
    match text.as_deref().map(version) {
        Some(Some(v)) if v >= HOOK_VERSION => return Ok(()),
        // An older one of gerdr's: rewritten, the chained hook kept.
        Some(Some(_)) | None => {}
        Some(None) => {
            let chained = hooks.join(CHAINED);
            std::fs::rename(&path, &chained)
                .map_err(|err| format!("couldn't move {} aside: {err}", shown(&path)))?;
        }
    }
    write_hook(&hooks, &path)
}

/// Write `HOOK` to `path` in `hooks`, executable, all at once.
fn write_hook(hooks: &Path, path: &Path) -> Result<(), String> {
    // Its own name per write, so two threads writing at once can't mix.
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = hooks.join(format!(".pre-push.gerdr-{}-{n}", std::process::id()));
    let write = || -> std::io::Result<()> {
        std::fs::write(&tmp, HOOK)?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
        std::fs::rename(&tmp, path)
    };
    write().map_err(|err| {
        // Only the half-written copy: the error below says what failed.
        let _ = std::fs::remove_file(&tmp);
        format!("couldn't write {}: {err}", shown(path))
    })
}

/// The version of gerdr's hook `text` is, or `None` when it isn't gerdr's.
fn version(text: &str) -> Option<u32> {
    text.lines()
        .find_map(|l| l.trim().strip_prefix(MARK))
        .map(|v| v.trim().parse().unwrap_or(0))
}

/// The hooks folder git runs `worktree`'s hooks from, absolute.
fn hooks_dir(worktree: &Path) -> Result<PathBuf, String> {
    let out = git(worktree, &["rev-parse", "--git-path", "hooks"])?;
    // Relative to the folder git ran in: a relative core.hooksPath is
    // the worktree's own.
    Ok(worktree.join(out.trim()))
}

/// Whether git tracks `path` in `worktree`; a path outside it isn't.
fn tracked(worktree: &Path, path: &Path) -> bool {
    let top = worktree
        .canonicalize()
        .unwrap_or_else(|_| worktree.to_path_buf());
    let full = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let Ok(rel) = full.strip_prefix(&top) else {
        return false;
    };
    let rel = rel.to_string_lossy();
    git(worktree, &["ls-files", "--", &rel]).is_ok_and(|out| !out.trim().is_empty())
}

/// The git folder every worktree of the repository `dir` is in shares.
fn common_dir(dir: &Path) -> Result<PathBuf, String> {
    let out = git(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    Ok(PathBuf::from(out.trim()))
}

/// The guard file of `branch` of the repository `dir` is in.
fn guard_file(dir: &Path, branch: &str) -> Result<PathBuf, String> {
    let bad = branch.is_empty()
        || branch
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..");
    if bad {
        return Err(format!("\"{branch}\" can't name a guard file"));
    }
    Ok(common_dir(dir)?.join("gerdr").join("guard").join(branch))
}

/// Write `text` to `branch`'s guard file, making its folder.
fn write_guard(dir: &Path, branch: &str, text: &str) -> Result<(), String> {
    let file = guard_file(dir, branch)?;
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("couldn't make {}: {err}", shown(parent)))?;
    }
    crate::projects::write_atomic(&file, text.as_bytes())
        .map_err(|err| format!("couldn't write {}: {err}", shown(&file)))
}

/// Guard `branch` of the repository `dir` is in: a push of it is refused
/// until `allow`.
pub fn guard(dir: &Path, branch: &str) -> Result<(), String> {
    write_guard(dir, branch, "")
}

/// Let `branch` be pushed at commit `commit` (a full hash), and only at it.
pub fn allow(dir: &Path, branch: &str, commit: &str) -> Result<(), String> {
    write_guard(dir, branch, &format!("{commit}\n"))
}

/// Stop guarding `branch`: its feature is gone. Nothing to remove is fine.
pub fn unguard(dir: &Path, branch: &str) -> Result<(), String> {
    let file = guard_file(dir, branch)?;
    match std::fs::remove_file(&file) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("couldn't remove {}: {err}", shown(&file))),
    }
}

/// Whether `branch` of the repository `dir` is in has a guard file (empty
/// or allowing a commit).
pub fn guarded(dir: &Path, branch: &str) -> bool {
    guard_file(dir, branch).is_ok_and(|f| f.is_file())
}

/// Guard `branch` when it has no guard file, as `guard` does, but leave
/// one that is there: it may allow a push under way. Returns whether it
/// made one.
pub fn guard_if_missing(dir: &Path, branch: &str) -> Result<bool, String> {
    let file = guard_file(dir, branch)?;
    if file.is_file() {
        return Ok(false);
    }
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("couldn't make {}: {err}", shown(parent)))?;
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&file)
    {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == ErrorKind::AlreadyExists => Ok(false),
        Err(err) => Err(format!("couldn't write {}: {err}", shown(&file))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::tests::{Temp, commit, run};

    /// A repository `app` on main with a bare origin, and a worktree `wt`
    /// on branch `feature/x` with a commit of its own.
    fn setup(t: &Temp) -> (PathBuf, PathBuf) {
        let origin = t.0.join("origin.git");
        std::fs::create_dir_all(&origin).unwrap();
        run(&origin, &["init", "-q", "--bare", "-b", "main"]);
        let app = t.repo("app", "main");
        run(&app, &["remote", "add", "origin", origin.to_str().unwrap()]);
        run(&app, &["push", "-q", "origin", "main"]);
        let wt = t.0.join("wt");
        run(
            &app,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature/x",
                wt.to_str().unwrap(),
            ],
        );
        commit(&wt, "f", "feature");
        (app, wt)
    }

    /// Push HEAD from `wt` to `to` on origin: everything git and the hooks
    /// said when it fails.
    fn push_to(wt: &Path, to: &str) -> Result<(), String> {
        let out = crate::git::command(wt, &["push", "-q", "origin", to])
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).into_owned())
        }
    }

    fn push(wt: &Path) -> Result<(), String> {
        push_to(wt, "HEAD")
    }

    #[test]
    fn a_guarded_branch_pushes_only_at_the_allowed_commit() {
        let t = Temp::new();
        let (app, wt) = setup(&t);
        assert_eq!(ensure(&wt), Guard::On);
        let hook = app.join(".git/hooks/pre-push");
        assert_eq!(std::fs::read_to_string(&hook).unwrap(), HOOK);
        assert_eq!(
            std::fs::metadata(&hook).unwrap().permissions().mode() & 0o777,
            0o755
        );
        // Unguarded: goes.
        push(&wt).unwrap();

        commit(&wt, "g", "more");
        assert!(!guarded(&app, "feature/x"));
        guard(&wt, "feature/x").unwrap();
        assert!(guarded(&app, "feature/x"));
        let err = push(&wt).unwrap_err();
        assert!(err.contains("ship it from the feature panel"), "{err}");

        let head = run(&wt, &["rev-parse", "HEAD"]).trim().to_owned();
        allow(
            &app,
            "feature/x",
            "0123456789012345678901234567890123456789",
        )
        .unwrap();
        assert!(push(&wt).is_err());
        allow(&app, "feature/x", &head).unwrap();
        // Not truncated by a repair.
        assert!(!guard_if_missing(&app, "feature/x").unwrap());
        push(&wt).unwrap();
        // Under another name too.
        guard(&app, "feature/x").unwrap();
        commit(&wt, "h", "again");
        assert!(push_to(&wt, "HEAD:other").is_err());
        // Deleting it goes.
        push_to(&wt, ":feature/x").unwrap();

        // Other branches aren't guarded.
        run(&wt, &["switch", "-q", "-c", "plain"]);
        push(&wt).unwrap();

        unguard(&app, "feature/x").unwrap();
        unguard(&app, "feature/x").unwrap();
        assert!(!guarded(&app, "feature/x"));
        assert!(guard_if_missing(&app, "feature/x").unwrap());
        assert!(guarded(&app, "feature/x"));
        assert!(guard(&app, "../x").is_err());
    }

    #[test]
    fn a_hook_that_was_there_runs_after_and_can_still_refuse() {
        let t = Temp::new();
        let (app, wt) = setup(&t);
        let hooks = app.join(".git/hooks");
        let log = t.0.join("log");
        let old = format!(
            "#!/bin/sh\necho \"$0 $1\" > {log}\ncat >> {log}\n[ ! -e {stop} ]\n",
            log = log.display(),
            stop = t.0.join("stop").display()
        );
        std::fs::write(hooks.join("pre-push"), &old).unwrap();
        std::fs::set_permissions(hooks.join("pre-push"), PermissionsExt::from_mode(0o755)).unwrap();
        assert_eq!(ensure(&wt), Guard::On);
        assert_eq!(std::fs::read_to_string(hooks.join(CHAINED)).unwrap(), old);
        // Again: left as it is.
        assert_eq!(ensure(&wt), Guard::On);
        assert_eq!(std::fs::read_to_string(hooks.join(CHAINED)).unwrap(), old);

        push(&wt).unwrap();
        let said = std::fs::read_to_string(&log).unwrap();
        let mut lines = said.lines();
        // $0 is still the hook git ran.
        let first = lines.next().unwrap();
        assert!(first.ends_with("/pre-push origin"), "{said}");
        // The refs pushed, as git gave them.
        let refs = lines.next().unwrap();
        assert!(refs.contains(" refs/heads/feature/x "), "{said}");

        std::fs::write(t.0.join("stop"), "").unwrap();
        commit(&wt, "g", "more");
        assert!(push(&wt).is_err());

        // An older one of gerdr's is rewritten, the chained one kept.
        std::fs::write(hooks.join("pre-push"), HOOK.replace("guard v1", "guard v0")).unwrap();
        assert_eq!(ensure(&wt), Guard::On);
        assert_eq!(
            std::fs::read_to_string(hooks.join("pre-push")).unwrap(),
            HOOK
        );
        assert_eq!(std::fs::read_to_string(hooks.join(CHAINED)).unwrap(), old);
    }

    #[test]
    fn a_relative_hooks_path_is_the_worktrees_own_and_husky_still_finds_its_hook() {
        let t = Temp::new();
        let (app, wt) = setup(&t);
        run(&app, &["config", "core.hooksPath", ".husky/_"]);
        let off = ensure(&wt);
        assert!(
            matches!(&off, Guard::Off(why) if why.contains(".husky/_ isn't there yet")),
            "{off:?}"
        );
        // husky v9's layout: a stub per hook that sources `h`, which runs
        // .husky/<the stub's name>.
        let husky = wt.join(".husky");
        std::fs::create_dir_all(husky.join("_")).unwrap();
        std::fs::write(husky.join("_/.gitignore"), "*\n").unwrap();
        let stub = "#!/usr/bin/env sh\n. \"$(dirname \"$0\")/h\"\n";
        std::fs::write(husky.join("_/pre-push"), stub).unwrap();
        std::fs::write(
            husky.join("_/h"),
            "#!/usr/bin/env sh\nn=$(basename \"$0\")\ns=$(dirname \"$(dirname \"$0\")\")/$n\n\
             [ ! -f \"$s\" ] && exit 0\nsh -e \"$s\" \"$@\"\n",
        )
        .unwrap();
        for f in ["_/pre-push", "_/h"] {
            std::fs::set_permissions(husky.join(f), PermissionsExt::from_mode(0o755)).unwrap();
        }
        let log = t.0.join("log");
        std::fs::write(
            husky.join("pre-push"),
            format!("echo \"ran $1\" > {}\nexit 3\n", log.display()),
        )
        .unwrap();
        assert_eq!(ensure(&wt), Guard::On);
        assert_eq!(
            std::fs::read_to_string(husky.join("_").join(CHAINED)).unwrap(),
            stub
        );
        // The main worktree's hooks are its own: untouched.
        assert!(!app.join(".husky").exists());
        let err = push(&wt).unwrap_err();
        assert!(!err.contains("gerdr:"), "{err}");
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "ran origin\n");

        // Tracked: left alone.
        let t2 = Temp::new();
        let (app2, wt2) = setup(&t2);
        run(&app2, &["config", "core.hooksPath", "hooks"]);
        std::fs::create_dir_all(wt2.join("hooks")).unwrap();
        std::fs::write(wt2.join("hooks/pre-push"), "#!/bin/sh\n").unwrap();
        run(&wt2, &["add", "hooks/pre-push"]);
        let off = ensure(&wt2);
        assert!(
            matches!(&off, Guard::Off(why) if why.contains("is in git")),
            "{off:?}"
        );
        assert_eq!(
            std::fs::read_to_string(wt2.join("hooks/pre-push")).unwrap(),
            "#!/bin/sh\n"
        );
    }
}

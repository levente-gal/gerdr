//! The commands a repository's files point to, offered in the project
//! commands dialog (see `project_commands`) where it has none yet.
//!
//! It only reads files: nothing of the repository runs. Call it off the UI
//! thread (checklist D3).

use std::path::{Path, PathBuf};

/// What a repository's files suggest, each list in the order they would
/// run. Empty when nothing there says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Suggested {
    /// For Verify's checks.
    pub checks: Vec<String>,
    /// For setting up a new worktree.
    pub setup: Vec<String>,
    /// For what a sandbox runs.
    pub run: Vec<String>,
    /// For what a sandbox runs once first.
    pub prepare: Vec<String>,
}

/// Words of a command that does work on a schedule: copied data would make it
/// do real work again, so it is suggested switched off.
const SCHEDULED: &[&str] = &["beat", "cron", "scheduler"];

/// Make what was seen running from `main` the suggested run commands: `seen`
/// holds each with its folder. It is what really runs, so it goes over what the
/// files suggest. A scheduler starts as a comment, to be switched on by hand.
pub fn with_seen(out: &mut Suggested, main: &Path, seen: &[(PathBuf, String)]) {
    let mut commands: Vec<String> = Vec::new();
    for (dir, command) in seen {
        if !dir.starts_with(main) || dir.components().any(|c| c.as_os_str() == ".features") {
            continue;
        }
        let scheduled = command
            .split_whitespace()
            .any(|word| SCHEDULED.iter().any(|s| word.contains(s)));
        let line = match scheduled {
            true => format!("# {command}"),
            false => command.clone(),
        };
        if !commands.contains(&line) {
            commands.push(line);
        }
    }
    if !commands.is_empty() {
        out.run = commands;
    }
}

/// What npm writes as the test script of a package that has no tests.
const NO_TESTS: &str = "no test specified";

/// What the files in `dir`, a repository's top folder, suggest: a command
/// for each kind of project found there.
pub fn suggest(dir: &Path) -> Suggested {
    let mut out = Suggested::default();
    if dir.join("Cargo.toml").is_file() {
        out.checks.push("cargo test".into());
    }
    node(dir, &mut out);
    if dir.join("go.mod").is_file() {
        out.checks.push("go test ./...".into());
    }
    python(dir, &mut out);
    if has_make_test(dir) {
        out.checks.push("make test".into());
    }
    out
}

/// A `package.json`: its test script, and what installs its packages, by
/// the lockfile there.
fn node(dir: &Path, out: &mut Suggested) {
    let Ok(raw) = std::fs::read(dir.join("package.json")) else {
        return;
    };
    let (test, install) = node_tools(dir);
    out.setup.push(install.into());
    let package: Option<serde_json::Value> = serde_json::from_slice(&raw).ok();
    let runner = test.split(' ').next().unwrap_or("npm");
    for name in ["dev", "start"] {
        if package
            .as_ref()
            .is_some_and(|p| p["scripts"][name].is_string())
        {
            out.run.push(format!("{runner} run {name}"));
            break;
        }
    }
    let script = package.as_ref().and_then(|p| p["scripts"]["test"].as_str());
    if script.is_some_and(|s| !s.contains(NO_TESTS)) {
        out.checks.push(test.into());
    }
}

/// How the packages of the `package.json` in `dir` are tested and installed,
/// by the lockfile there.
pub fn node_tools(dir: &Path) -> (&'static str, &'static str) {
    if dir.join("pnpm-lock.yaml").is_file() {
        ("pnpm test", "pnpm install --frozen-lockfile")
    } else if dir.join("yarn.lock").is_file() {
        ("yarn test", "yarn install --immutable")
    } else if dir.join("package-lock.json").is_file() {
        ("npm test", "npm ci")
    } else {
        ("npm test", "npm install")
    }
}

/// A `pyproject.toml`: pytest and what installs its packages, by the tool
/// that manages it.
fn python(dir: &Path, out: &mut Suggested) {
    if dir.join("manage.py").is_file() {
        out.prepare.push("python manage.py migrate".into());
        out.run.push("python manage.py runserver".into());
    }
    if !dir.join("pyproject.toml").is_file() {
        return;
    }
    if crate::prepare::is_poetry_project(dir) {
        out.checks.push("poetry run pytest".into());
        out.setup.push("poetry install".into());
    } else if dir.join("uv.lock").is_file() {
        out.checks.push("uv run pytest".into());
        out.setup.push("uv sync".into());
    } else {
        out.checks.push("pytest".into());
    }
}

/// Whether the `Makefile` in `dir` has a `test` target.
fn has_make_test(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join("Makefile")).is_ok_and(|text| {
        text.lines().any(|line| {
            line.strip_prefix("test")
                .is_some_and(|rest| rest.trim_start().starts_with(':'))
        })
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// What a folder holding these files suggests.
    fn with(files: &[(&str, &str)]) -> Suggested {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let name = format!("gerdr-suggest-{}-{n}", std::process::id());
        let dir = std::env::temp_dir().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, text) in files {
            std::fs::write(dir.join(name), text).unwrap();
        }
        let out = suggest(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    const TESTED: &str = r#"{"scripts": {"test": "vitest"}}"#;

    #[test]
    fn what_runs_is_suggested_from_the_files() {
        let dev = r#"{"scripts": {"dev": "vite", "start": "node ."}}"#;
        assert_eq!(with(&[("package.json", dev)]).run, ["npm run dev"]);
        let served = with(&[("manage.py", "")]);
        assert_eq!(served.run, ["python manage.py runserver"]);
        assert_eq!(served.prepare, ["python manage.py migrate"]);
    }

    #[test]
    fn what_was_seen_running_goes_over_the_files_a_scheduler_commented() {
        let mut out = Suggested {
            run: vec!["npm run dev".into()],
            ..Suggested::default()
        };
        let main = Path::new("/ao/api");
        let seen = [
            (PathBuf::from("/ao/api"), "./runlocal.sh".to_owned()),
            (PathBuf::from("/ao/api"), "./runlocal.sh".to_owned()),
            (
                PathBuf::from("/ao/api/sub"),
                "celery -A x beat -l INFO".to_owned(),
            ),
            (PathBuf::from("/ao/web"), "nx serve".to_owned()),
            (PathBuf::from("/ao/api/.features/f"), "./old.sh".to_owned()),
        ];
        with_seen(&mut out, main, &seen);
        assert_eq!(out.run, ["./runlocal.sh", "# celery -A x beat -l INFO"]);
        let mut kept = Suggested {
            run: vec!["npm run dev".into()],
            ..Suggested::default()
        };
        with_seen(&mut kept, main, &[]);
        assert_eq!(kept.run, ["npm run dev"]);
    }

    #[test]
    fn a_folder_with_nothing_known_suggests_nothing() {
        assert_eq!(with(&[("README.md", "hi")]), Suggested::default());
    }

    #[test]
    fn rust_and_go_have_their_tests() {
        assert_eq!(with(&[("Cargo.toml", "")]).checks, ["cargo test"]);
        assert_eq!(with(&[("go.mod", "")]).checks, ["go test ./..."]);
        assert!(with(&[("Cargo.toml", "")]).setup.is_empty());
    }

    #[test]
    fn a_package_runs_by_its_lockfile() {
        let of = |lock: &str| with(&[("package.json", TESTED), (lock, "")]);
        let npm = of("package-lock.json");
        assert_eq!(
            (npm.checks, npm.setup),
            (vec!["npm test".to_owned()], vec!["npm ci".to_owned()])
        );
        let pnpm = of("pnpm-lock.yaml");
        assert_eq!(pnpm.checks, ["pnpm test"]);
        assert_eq!(pnpm.setup, ["pnpm install --frozen-lockfile"]);
        let yarn = of("yarn.lock");
        assert_eq!(yarn.checks, ["yarn test"]);
        assert_eq!(yarn.setup, ["yarn install --immutable"]);
        assert_eq!(with(&[("package.json", TESTED)]).setup, ["npm install"]);
    }

    #[test]
    fn a_package_without_a_test_script_has_no_check() {
        let none = with(&[("package.json", r#"{"scripts": {"dev": "vite"}}"#)]);
        assert!(none.checks.is_empty());
        assert_eq!(none.setup, ["npm install"]);
        let placeholder = r#"{"scripts": {"test": "echo \"Error: no test specified\" && exit 1"}}"#;
        assert!(with(&[("package.json", placeholder)]).checks.is_empty());
        // Not JSON: nothing to say about its tests, and no failure.
        assert!(with(&[("package.json", "{")]).checks.is_empty());
    }

    #[test]
    fn python_goes_by_the_tool_that_manages_it() {
        let plain = with(&[("pyproject.toml", "")]);
        assert_eq!(plain.checks, ["pytest"]);
        assert!(plain.setup.is_empty());
        let poetry = with(&[("pyproject.toml", ""), ("poetry.lock", "")]);
        assert_eq!(poetry.checks, ["poetry run pytest"]);
        assert_eq!(poetry.setup, ["poetry install"]);
        let uv = with(&[("pyproject.toml", ""), ("uv.lock", "")]);
        assert_eq!(uv.checks, ["uv run pytest"]);
        assert_eq!(uv.setup, ["uv sync"]);
    }

    #[test]
    fn only_a_makefile_with_a_test_target_counts() {
        assert_eq!(
            with(&[("Makefile", "all:\n\ntest : all\n\tgo\n")]).checks,
            ["make test"]
        );
        assert!(
            with(&[("Makefile", "all:\n\ntests:\n\t# test: no\n")])
                .checks
                .is_empty()
        );
    }

    #[test]
    fn each_kind_of_project_found_gets_its_own() {
        let both = with(&[
            ("Cargo.toml", ""),
            ("package.json", TESTED),
            ("package-lock.json", ""),
        ]);
        assert_eq!(both.checks, ["cargo test", "npm test"]);
    }
}

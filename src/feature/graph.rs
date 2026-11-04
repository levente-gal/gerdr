//! A code graph for each of a feature's folders, for the projects that
//! keep one with graphify: their agents ask it before searching the code.
//!
//! Graphs are built, not copied: `graphify update` takes about as long
//! with a copied graph as without one, and a copy is large. There is one
//! graph per feature folder rather than per repository, so it holds what
//! the repositories call of each other. A project whose root or repository
//! has no graph gets none here either.
//!
//! A repository that commits its graph keeps it, in the worktree already.
//! Any other gets `graphify-out` kept out of git first: gerdr would count
//! it as work a removal loses, and an agent could commit it.
//!
//! A project's graphify settings apply here too. A committed ignore file is
//! in the worktree already; one that git doesn't carry is linked in, and the
//! `--exclude`s graphify kept beside the project's graph are kept beside this
//! one. So a feature's graph leaves out what the project's leaves out.
//!
//! Runs processes: call `build` off the UI thread (checklist D3).

use std::{
    fs::File,
    path::{Path, PathBuf},
    process::Stdio,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use serde_json::{Map, Value};

use super::{
    Feature,
    layout::{FEATURES_DIR, child_repos, exclude_line, git, shown},
    setup::{Exit, shell_until},
};

/// The folder graphify writes in.
pub const OUT_DIR: &str = "graphify-out";

/// The file of patterns graphify leaves out, in any folder it scans.
const IGNORE_FILE: &str = ".graphifyignore";

/// Where graphify keeps a graph's build options, in `OUT_DIR`: the
/// `--exclude`s it was first built with, for each later build to apply.
const BUILD_OPTIONS: &str = ".graphify_build.json";

/// What builds or refreshes the graph of the folder it runs in. `--force`:
/// a feature may delete code, and graphify otherwise keeps the larger graph.
const COMMAND: &str = "nice -n 10 graphify update . --force";

/// How long a folder's graph may take to build.
const TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// The exit code of a shell, or of `nice`, that can't find the command.
const NOT_FOUND: i32 = 127;

/// What graphify says of a folder without code, exiting with 1: no failure.
const NO_CODE: &str = "No code files found";

/// How long `Run::stop_and_wait` waits: past the 2 s a stopped command gets
/// before it is killed.
const STOP_WAIT: Duration = Duration::from_secs(5);

/// The graph file of feature folder `dir`.
pub fn file(dir: &Path) -> PathBuf {
    dir.join(OUT_DIR).join("graph.json")
}

/// A feature folder to keep a graph in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folder {
    pub dir: PathBuf,
    /// Whether it holds several repositories rather than being the one.
    pub multi: bool,
    /// The project's root, whose settings count for the folder.
    pub root: PathBuf,
    /// The repositories' worktrees in it.
    pub repos: Vec<Tree>,
}

/// A repository of a feature folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tree {
    /// Its main worktree, with the project's settings.
    pub main: PathBuf,
    /// The feature's worktree of it.
    pub worktree: PathBuf,
}

/// The folders of `feature` holding code whose project keeps a graph, at its
/// root or in the main worktree of one of its repositories, linked to the
/// feature or not. A folder with no repository in it has no code to graph,
/// and naming its graph would send agents to a file that never comes.
pub fn folders(feature: &Feature) -> Vec<Folder> {
    let home = (
        None,
        feature.root.as_path(),
        feature.dir.as_path(),
        feature.multi,
    );
    let links = feature.links.iter().map(|l| {
        (
            Some(l.project.as_str()),
            l.root.as_path(),
            l.dir.as_path(),
            l.multi,
        )
    });
    std::iter::once(home)
        .chain(links)
        .filter(|(project, ..)| feature.repos_of(*project).next().is_some())
        .filter(|(project, root, _, multi)| {
            file(root).exists()
                || feature.repos_of(*project).any(|r| file(&r.main).exists())
                || (*multi
                    && child_repos(root)
                        .iter()
                        .any(|(_, main)| file(main).exists()))
        })
        .map(|(project, root, dir, multi)| Folder {
            dir: dir.to_path_buf(),
            multi,
            root: root.to_path_buf(),
            repos: feature
                .repos_of(project)
                .map(|r| Tree {
                    main: r.main.clone(),
                    worktree: r.worktree.clone(),
                })
                .collect(),
        })
        .collect()
}

/// A build's stop switch: taking a feature's folder down stops it first.
#[derive(Debug, Default)]
pub struct Run {
    stop: AtomicBool,
    ended: AtomicBool,
}

impl Run {
    /// Stop the build and wait for it to end, so nothing writes in the
    /// folder any more. Blocks: call it off the UI thread.
    pub fn stop_and_wait(&self) {
        self.stop.store(true, Ordering::Relaxed);
        let started = Instant::now();
        while !self.ended.load(Ordering::Relaxed) && started.elapsed() < STOP_WAIT {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// What a build came to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Built {
    /// The git exclude files `graphify-out` was added to.
    pub excludes: Vec<PathBuf>,
    /// The ignore files linked in from the project.
    pub linked: Vec<PathBuf>,
    /// graphify isn't installed.
    pub missing: bool,
    /// Why a folder has no fresh graph.
    pub failed: Option<String>,
}

/// Build or refresh the graphs of `folders`, one after the other, with
/// graphify's output in `log`.
pub fn build(folders: &[Folder], slug: &str, log: &Path, run: &Run) -> Built {
    let built = build_with(COMMAND, folders, slug, log, run);
    run.ended.store(true, Ordering::Relaxed);
    built
}

fn build_with(command: &str, folders: &[Folder], slug: &str, log: &Path, run: &Run) -> Built {
    let mut built = Built::default();
    let file = log
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| File::create(log));
    let file = match file {
        Ok(file) => file,
        Err(err) => {
            built.failed = Some(format!("couldn't open {}: {err}", shown(log)));
            return built;
        }
    };
    for folder in folders {
        if run.stop.load(Ordering::Relaxed) {
            break;
        }
        if !folder.dir.is_dir() || (!folder.multi && tracked(&folder.dir)) {
            continue;
        }
        if let Err(why) = keep_out_of_git(folder, &mut built.excludes) {
            built.failed = Some(why);
            continue;
        }
        // Without them the graph would hold what the project leaves out, so
        // no graph is better than that one.
        let settings =
            link_ignore_files(folder, &mut built.linked).and_then(|()| write_build_options(folder));
        if let Err(why) = settings {
            built.failed = Some(why);
            continue;
        }
        let (Ok(out), Ok(err)) = (file.try_clone(), file.try_clone()) else {
            built.failed = Some(format!("couldn't write {}", shown(log)));
            break;
        };
        let start = file.metadata().map_or(0, |m| m.len());
        let exit = shell_until(
            command,
            &folder.dir,
            slug,
            Stdio::from(out),
            Stdio::from(err),
            TIMEOUT,
            &run.stop,
        );
        // Stopped on purpose: nobody is waiting for what it came to.
        if run.stop.load(Ordering::Relaxed) {
            break;
        }
        match exit {
            Exit::Code(0) => {}
            Exit::Code(NOT_FOUND) => {
                built.missing = true;
                break;
            }
            Exit::Code(1) if said(log, start, NO_CODE) => {}
            other => {
                built.failed = Some(format!(
                    "graphify in {} {}",
                    shown(&folder.dir),
                    other.describe()
                ));
            }
        }
    }
    built
}

/// Whether the repository at `dir` commits its graph.
fn tracked(dir: &Path) -> bool {
    let path = format!("{OUT_DIR}/");
    git(dir, &["ls-files", "--", &path]).is_ok_and(|files| !files.trim().is_empty())
}

/// Keep `graphify-out` out of git in each of `folder`'s repositories, in
/// one under a multi-repository folder too: an agent may run graphify
/// there. Notes the exclude files changed.
fn keep_out_of_git(folder: &Folder, excludes: &mut Vec<PathBuf>) -> Result<(), String> {
    let ignored = format!("{OUT_DIR}/");
    let line = format!("/{OUT_DIR}/");
    for repo in folder.repos.iter().filter(|r| r.worktree.is_dir()) {
        if git(&repo.worktree, &["check-ignore", "-q", &ignored]).is_ok() {
            continue;
        }
        if let Some(exclude) = exclude_line(&repo.worktree, &line)? {
            excludes.push(exclude);
        }
    }
    Ok(())
}

/// Link in the project's ignore files that `folder` lacks: the root's of a
/// multi-repository folder, which may be newer than the folder, and those
/// of its repositories that git doesn't bring, being uncommitted or newer
/// than the feature's branch. One the branch has, or deleted, stays its own.
/// The exclude file is shared with the main worktree: an untracked ignore
/// file drops out of `git status` there too.
fn link_ignore_files(folder: &Folder, linked: &mut Vec<PathBuf>) -> Result<(), String> {
    let mut link = |from: &Path, to: &Path| {
        if !from.exists() || to.symlink_metadata().is_ok() {
            return Ok(());
        }
        std::os::unix::fs::symlink(from, to)
            .map_err(|err| format!("couldn't link {}: {err}", shown(to)))?;
        linked.push(to.to_path_buf());
        Ok::<(), String>(())
    };
    if folder.multi && folder.dir.is_dir() {
        link(
            &folder.root.join(IGNORE_FILE),
            &folder.dir.join(IGNORE_FILE),
        )?;
    }
    for repo in &folder.repos {
        if !repo.main.is_dir() || !repo.worktree.is_dir() {
            continue;
        }
        for path in ignore_files(&repo.main)? {
            let (from, to) = (repo.main.join(&path), repo.worktree.join(&path));
            if !from.exists()
                || to.symlink_metadata().is_ok()
                // Only in a folder of the main worktree's own.
                || !to.parent().is_some_and(Path::is_dir)
                || !git(
                    &repo.worktree,
                    &["ls-files", "--", &format!(":(literal){path}")],
                )?
                .trim()
                .is_empty()
            {
                continue;
            }
            if git(&repo.worktree, &["check-ignore", "-q", &path]).is_err() {
                exclude_line(&repo.worktree, &format!("/{path}"))?;
            }
            link(&from, &to)?;
        }
    }
    Ok(())
}

/// The ignore files in the main worktree at `main`, committed or not, by
/// their paths in it.
fn ignore_files(main: &Path) -> Result<Vec<String>, String> {
    let glob = format!(":(glob)**/{IGNORE_FILE}");
    let features = format!("--exclude={FEATURES_DIR}");
    let args = [
        "ls-files",
        "-z",
        "-c",
        "-o",
        "--exclude=node_modules",
        features.as_str(),
        "--",
        glob.as_str(),
    ];
    let listed = git(main, &args)?;
    Ok(listed
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect())
}

/// Give `folder`'s graph the build options of the project's: graphify reads
/// them from beside the graph. It has none once the project has none.
fn write_build_options(folder: &Folder) -> Result<(), String> {
    let path = folder.dir.join(OUT_DIR).join(BUILD_OPTIONS);
    let options = build_options(folder);
    if options.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                Err(format!("couldn't remove {}: {err}", shown(&path)))
            }
            _ => Ok(()),
        };
    }
    if let Some(out) = path.parent() {
        std::fs::create_dir_all(out)
            .map_err(|err| format!("couldn't make {}: {err}", shown(out)))?;
    }
    std::fs::write(&path, Value::Object(options).to_string())
        .map_err(|err| format!("couldn't write {}: {err}", shown(&path)))
}

/// The build options of the project's graph, with the `--exclude`s of its
/// repositories' graphs moved under their places in `folder`. A repository's
/// other options have no meaning for a graph of the whole folder.
fn build_options(folder: &Folder) -> Map<String, Value> {
    let mut options = options_of(&folder.root);
    for repo in folder.repos.iter().filter(|r| r.main != folder.root) {
        let Ok(inside) = repo.worktree.strip_prefix(&folder.dir) else {
            continue;
        };
        let theirs = options_of(&repo.main);
        let excludes: Vec<Value> = theirs
            .get("excludes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|pattern| under(inside, pattern).into())
            .collect();
        if excludes.is_empty() {
            continue;
        }
        let ours = options
            .entry("excludes")
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Value::Array(ours) = ours {
            ours.extend(excludes);
        }
    }
    options
}

/// The build options graphify keeps beside the graph of `main`. One it can't
/// read counts as none, as it does for graphify.
fn options_of(main: &Path) -> Map<String, Value> {
    let path = main.join(OUT_DIR).join(BUILD_OPTIONS);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Map::new();
    };
    serde_json::from_str(&text).unwrap_or_else(|err| {
        tracing::warn!("reading {}: {err}", path.display());
        Map::new()
    })
}

/// Ignore `pattern`, written for folder `inside`, as one for the folder
/// above it. A pattern without a `/` matches at any depth.
fn under(inside: &Path, pattern: &str) -> String {
    if inside.as_os_str().is_empty() {
        return pattern.to_owned();
    }
    let (not, rest) = pattern
        .strip_prefix('!')
        .map_or(("", pattern), |rest| ("!", rest));
    let anywhere = !rest.trim_end_matches('/').contains('/');
    let joint = if anywhere { "/**/" } else { "/" };
    format!(
        "{not}{}{joint}{}",
        inside.display(),
        rest.trim_start_matches('/')
    )
}

/// Whether `log` says `what` past byte `start`.
fn said(log: &Path, start: u64, what: &str) -> bool {
    std::fs::read(log).is_ok_and(|bytes| {
        let start = usize::try_from(start)
            .unwrap_or(usize::MAX)
            .min(bytes.len());
        String::from_utf8_lossy(&bytes[start..]).contains(what)
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::feature::{Link, Repo};

    /// A folder of its own for a test, outside any repository.
    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("gerdr-graph-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn repo(&self, name: &str) -> PathBuf {
            let dir = self.0.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            git(&dir, &["init", "-q", "-b", "main"]).unwrap();
            dir
        }

        fn log(&self) -> PathBuf {
            self.0.join("data").join("graph.log")
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn single(dir: &Path) -> Vec<Folder> {
        vec![Folder {
            dir: dir.to_path_buf(),
            multi: false,
            root: dir.to_path_buf(),
            repos: vec![tree(dir, dir)],
        }]
    }

    fn tree(main: &Path, worktree: &Path) -> Tree {
        Tree {
            main: main.to_path_buf(),
            worktree: worktree.to_path_buf(),
        }
    }

    /// A commit in the repository at `dir`, of all in it.
    fn commit(dir: &Path) {
        git(dir, &["add", "-A"]).unwrap();
        git(
            dir,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "c",
                "--allow-empty",
            ],
        )
        .unwrap();
    }

    /// A worktree of `repo` at `at`, on a new branch.
    fn worktree(repo: &Path, at: &Path) {
        let at = at.to_str().unwrap();
        git(repo, &["worktree", "add", "-q", "-b", "f", at]).unwrap();
    }

    fn built(t: &Temp, command: &str, folders: &[Folder]) -> Built {
        build_with(command, folders, "s", &t.log(), &Run::default())
    }

    fn status(dir: &Path) -> String {
        git(dir, &["status", "--porcelain", "--untracked-files=all"]).unwrap()
    }

    const MAKES: &str = "mkdir -p graphify-out && echo '{}' > graphify-out/graph.json";

    #[test]
    fn a_graph_is_built_and_git_never_sees_it() {
        let t = Temp::new("built");
        let repo = t.repo("app");
        let result = built(&t, MAKES, &single(&repo));
        assert_eq!(result.failed, None);
        assert!(file(&repo).exists());
        assert_eq!(result.excludes.len(), 1);
        assert!(result.excludes[0].ends_with(".git/info/exclude"));
        assert_eq!(status(&repo), "");
        // Once only.
        assert!(built(&t, MAKES, &single(&repo)).excludes.is_empty());
    }

    #[test]
    fn a_repository_that_ignores_the_folder_has_its_exclude_file_left_alone() {
        let t = Temp::new("ignores");
        let repo = t.repo("app");
        std::fs::write(repo.join(".gitignore"), "/graphify-out/\n").unwrap();
        let result = built(&t, MAKES, &single(&repo));
        assert_eq!(result, Built::default());
        assert!(file(&repo).exists());
    }

    #[test]
    fn a_repository_that_commits_its_graph_keeps_it_as_it_is() {
        let t = Temp::new("tracked");
        let repo = t.repo("app");
        std::fs::create_dir_all(repo.join(OUT_DIR)).unwrap();
        std::fs::write(file(&repo), "old").unwrap();
        git(&repo, &["add", "."]).unwrap();
        let result = built(&t, MAKES, &single(&repo));
        assert_eq!(result, Built::default());
        assert_eq!(std::fs::read_to_string(file(&repo)).unwrap(), "old");
    }

    #[test]
    fn a_multi_repository_folder_has_one_graph_on_top_and_none_in_git() {
        let t = Temp::new("multi");
        let dir = t.0.join("feature");
        std::fs::create_dir_all(&dir).unwrap();
        let (api, web) = (t.repo("feature/api"), t.repo("feature/web"));
        let folders = [Folder {
            dir: dir.clone(),
            multi: true,
            root: t.0.clone(),
            repos: vec![tree(&api, &api), tree(&web, &web)],
        }];
        let result = built(&t, MAKES, &folders);
        assert_eq!(result.failed, None);
        assert!(file(&dir).exists());
        assert_eq!(result.excludes.len(), 2);
        // A stray graph in a repository stays out of git too.
        std::fs::create_dir_all(api.join(OUT_DIR)).unwrap();
        std::fs::write(file(&api), "{}").unwrap();
        assert_eq!(status(&api), "");
    }

    #[test]
    fn an_uncommitted_ignore_file_reaches_the_worktree_and_git_never_sees_it() {
        let t = Temp::new("ignore-untracked");
        let main = t.repo("main");
        std::fs::create_dir_all(main.join("sub")).unwrap();
        std::fs::write(main.join("sub/keep"), "").unwrap();
        commit(&main);
        let tree_dir = t.0.join("tree");
        worktree(&main, &tree_dir);
        std::fs::write(main.join(IGNORE_FILE), "data/\n").unwrap();
        std::fs::write(main.join("sub").join(IGNORE_FILE), "x\n").unwrap();
        // A folder the worktree doesn't have gets nothing.
        std::fs::create_dir_all(main.join("local")).unwrap();
        std::fs::write(main.join("local").join(IGNORE_FILE), "y\n").unwrap();
        let folders = [Folder {
            dir: tree_dir.clone(),
            multi: false,
            root: main.clone(),
            repos: vec![tree(&main, &tree_dir)],
        }];
        let result = built(&t, MAKES, &folders);
        assert_eq!(result.failed, None);
        assert_eq!(result.linked.len(), 2);
        assert_eq!(
            std::fs::read_to_string(tree_dir.join(IGNORE_FILE)).unwrap(),
            "data/\n"
        );
        assert!(tree_dir.join("sub").join(IGNORE_FILE).exists());
        assert!(!tree_dir.join("local").exists());
        assert_eq!(status(&tree_dir), "");
        // Once only.
        assert!(built(&t, MAKES, &folders).linked.is_empty());
    }

    #[test]
    fn a_branch_keeps_the_ignore_file_it_has_or_deleted() {
        let t = Temp::new("ignore-tracked");
        let main = t.repo("main");
        std::fs::write(main.join(IGNORE_FILE), "main\n").unwrap();
        commit(&main);
        let tree_dir = t.0.join("tree");
        worktree(&main, &tree_dir);
        std::fs::write(tree_dir.join(IGNORE_FILE), "branch\n").unwrap();
        let folders = [Folder {
            dir: tree_dir.clone(),
            multi: false,
            root: main.clone(),
            repos: vec![tree(&main, &tree_dir)],
        }];
        assert!(built(&t, MAKES, &folders).linked.is_empty());
        assert_eq!(
            std::fs::read_to_string(tree_dir.join(IGNORE_FILE)).unwrap(),
            "branch\n"
        );
        std::fs::remove_file(tree_dir.join(IGNORE_FILE)).unwrap();
        assert!(built(&t, MAKES, &folders).linked.is_empty());
        assert!(tree_dir.join(IGNORE_FILE).symlink_metadata().is_err());
    }

    #[test]
    fn a_root_ignore_file_newer_than_the_folder_is_linked() {
        let t = Temp::new("ignore-root");
        let root = t.0.join("root");
        let dir = root.join(".features/a");
        std::fs::create_dir_all(&dir).unwrap();
        let main = t.repo("root/api");
        commit(&main);
        let tree_dir = dir.join("api");
        worktree(&main, &tree_dir);
        std::fs::write(root.join(IGNORE_FILE), "api/data/\n").unwrap();
        let folders = [Folder {
            dir: dir.clone(),
            multi: true,
            root: root.clone(),
            repos: vec![tree(&main, &tree_dir)],
        }];
        let result = built(&t, MAKES, &folders);
        assert_eq!(result.failed, None);
        assert_eq!(result.linked, [dir.join(IGNORE_FILE)]);
        assert_eq!(result.excludes.len(), 1);
    }

    #[test]
    fn a_folder_graph_takes_the_excludes_of_the_project_and_its_repositories() {
        let t = Temp::new("options");
        let root = t.0.join("root");
        let dir = root.join(".features/a");
        std::fs::create_dir_all(&dir).unwrap();
        let (api, web) = (t.repo("root/api"), t.repo("root/web"));
        let (api_tree, web_tree) = (dir.join("api"), dir.join("web"));
        commit(&api);
        commit(&web);
        worktree(&api, &api_tree);
        worktree(&web, &web_tree);
        for (main, text) in [
            (&root, r#"{"excludes":["big/"],"gitignore":false}"#),
            (
                &api,
                r#"{"excludes":["*.csv","data/raw/","!keep.csv"],"gitignore":true}"#,
            ),
            (&web, "not json"),
        ] {
            std::fs::create_dir_all(main.join(OUT_DIR)).unwrap();
            std::fs::write(main.join(OUT_DIR).join(BUILD_OPTIONS), text).unwrap();
        }
        let folders = [Folder {
            dir: dir.clone(),
            multi: true,
            root: root.clone(),
            repos: vec![tree(&api, &api_tree), tree(&web, &web_tree)],
        }];
        assert_eq!(built(&t, MAKES, &folders).failed, None);
        let written = dir.join(OUT_DIR).join(BUILD_OPTIONS);
        let options: Value =
            serde_json::from_str(&std::fs::read_to_string(&written).unwrap()).unwrap();
        assert_eq!(
            options,
            serde_json::json!({
                "excludes": ["big/", "api/**/*.csv", "api/data/raw/", "!api/**/keep.csv"],
                "gitignore": false,
            })
        );
        // None any more in the project: none here.
        for main in [&root, &api, &web] {
            std::fs::remove_file(main.join(OUT_DIR).join(BUILD_OPTIONS)).unwrap();
        }
        assert_eq!(built(&t, MAKES, &folders).failed, None);
        assert!(!written.exists());
    }

    #[test]
    fn a_pattern_moves_under_a_folder_keeping_what_it_matches() {
        let at = Path::new("api");
        assert_eq!(under(at, "*.csv"), "api/**/*.csv");
        assert_eq!(under(at, "out/"), "api/**/out/");
        assert_eq!(under(at, "/top"), "api/top");
        assert_eq!(under(at, "data/raw/"), "api/data/raw/");
        assert_eq!(under(at, "!keep"), "!api/**/keep");
        assert_eq!(under(Path::new(""), "*.csv"), "*.csv");
    }

    #[test]
    fn a_missing_graphify_is_told_apart_from_a_failure() {
        let t = Temp::new("missing");
        let repo = t.repo("app");
        let result = built(&t, "exit 127", &single(&repo));
        assert!(result.missing);
        assert_eq!(result.failed, None);
    }

    #[test]
    fn a_failure_names_the_folder_and_how_it_ended() {
        let t = Temp::new("failed");
        let repo = t.repo("app");
        let result = built(&t, "exit 3", &single(&repo));
        let why = result.failed.unwrap();
        assert!(
            why.contains("app") && why.contains("exited with 3"),
            "{why}"
        );
    }

    #[test]
    fn a_folder_without_code_is_no_failure() {
        let t = Temp::new("nocode");
        let repo = t.repo("app");
        let command = "echo 'No code files found - nothing to rebuild.'; exit 1";
        assert_eq!(built(&t, command, &single(&repo)).failed, None);
    }

    #[test]
    fn stopping_ends_the_command_at_once_and_says_nothing() {
        let t = Temp::new("stop");
        let repo = t.repo("app");
        let run = Arc::new(Run::default());
        let (folders, log, thread_run) = (single(&repo), t.log(), run.clone());
        let thread = std::thread::spawn(move || {
            let built = build_with("exec sleep 30", &folders, "s", &log, &thread_run);
            thread_run.ended.store(true, Ordering::Relaxed);
            built
        });
        std::thread::sleep(Duration::from_millis(100));
        let started = Instant::now();
        run.stop_and_wait();
        assert!(started.elapsed() < STOP_WAIT);
        assert_eq!(thread.join().unwrap().failed, None);
    }

    /// A feature over `repos`, each `(name, main, worktree)`.
    fn feature_of(root: &Path, dir: &Path, repos: &[(&str, &Path, &Path)]) -> Feature {
        let mut feature = crate::feature::tests::feature("a", 0);
        feature.root = root.to_path_buf();
        feature.dir = dir.to_path_buf();
        feature.multi = repos.len() > 1;
        feature.repos = repos
            .iter()
            .map(|(name, main, worktree)| Repo {
                name: (*name).into(),
                main: main.to_path_buf(),
                base: "main".into(),
                base_commit: String::new(),
                worktree: worktree.to_path_buf(),
                touched: true,
                branch: None,
                project: None,
            })
            .collect();
        feature
    }

    #[test]
    fn only_a_project_that_keeps_a_graph_gets_one() {
        let t = Temp::new("wanted");
        let (root, dir, main, tree) = (
            t.0.join("root"),
            t.0.join("root/.features/a"),
            t.0.join("root/api"),
            t.0.join("root/.features/a/api"),
        );
        std::fs::create_dir_all(&main).unwrap();
        let mut feature = feature_of(&root, &dir, &[("api", &main, &tree)]);
        assert!(folders(&feature).is_empty());
        // In a repository of the project.
        std::fs::create_dir_all(main.join(OUT_DIR)).unwrap();
        std::fs::write(file(&main), "{}").unwrap();
        assert_eq!(
            folders(&feature),
            [Folder {
                dir: dir.clone(),
                multi: false,
                root: root.clone(),
                repos: vec![super::Tree {
                    main: main.clone(),
                    worktree: tree,
                }],
            }]
        );
        // A linked project has a folder and a say of its own.
        let (other, other_dir) = (t.0.join("other"), t.0.join("other/.features/a"));
        feature.links.push(Link {
            project: "o".into(),
            root: other.clone(),
            dir: other_dir.clone(),
            multi: true,
        });
        feature.repos.push(Repo {
            name: "lib".into(),
            main: other.join("lib"),
            base: "main".into(),
            base_commit: String::new(),
            worktree: other_dir.join("lib"),
            touched: true,
            branch: None,
            project: Some("o".into()),
        });
        assert_eq!(folders(&feature).len(), 1);
        std::fs::create_dir_all(other.join(OUT_DIR)).unwrap();
        std::fs::write(file(&other), "{}").unwrap();
        let found = folders(&feature);
        assert_eq!(found.len(), 2);
        assert_eq!(
            (found[1].dir.as_path(), found[1].multi),
            (other_dir.as_path(), true)
        );
    }

    #[test]
    fn a_graph_in_a_repository_left_out_still_counts_for_the_project() {
        let t = Temp::new("left-out");
        let root = t.0.join("root");
        std::fs::create_dir_all(&root).unwrap();
        let web = root.join("web");
        std::fs::create_dir_all(&web).unwrap();
        git(&web, &["init", "-q", "-b", "main"]).unwrap();
        let api = root.join("api");
        std::fs::create_dir_all(&api).unwrap();
        git(&api, &["init", "-q", "-b", "main"]).unwrap();
        let dir = root.join(".features/a");
        let tree = dir.join("web");
        let mut feature = feature_of(&root, &dir, &[("web", &web, &tree)]);
        feature.multi = true;
        assert!(folders(&feature).is_empty());
        std::fs::create_dir_all(api.join(OUT_DIR)).unwrap();
        std::fs::write(file(&api), "{}").unwrap();
        let found = folders(&feature);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].repos[0].worktree, tree);
    }

    #[test]
    fn a_folder_without_a_repository_has_no_graph_to_name() {
        let t = Temp::new("no-repos");
        let root = t.0.join("root");
        std::fs::create_dir_all(root.join(OUT_DIR)).unwrap();
        std::fs::write(file(&root), "{}").unwrap();
        let mut feature = feature_of(&root, &root.join(".features/a"), &[]);
        feature.multi = true;
        assert!(folders(&feature).is_empty());
    }
}

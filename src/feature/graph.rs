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
//! Runs processes: call `build` off the UI thread (checklist D3).

use std::{
    fs::File,
    path::{Path, PathBuf},
    process::Stdio,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use super::{
    Feature,
    layout::{child_repos, exclude_line, git, shown},
    setup::{Exit, shell_until},
};

/// The folder graphify writes in.
pub const OUT_DIR: &str = "graphify-out";

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
    /// The repositories' worktrees in it.
    pub worktrees: Vec<PathBuf>,
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
                || (*multi && child_repos(root).iter().any(|(_, main)| file(main).exists()))
        })
        .map(|(project, _, dir, multi)| Folder {
            dir: dir.to_path_buf(),
            multi,
            worktrees: feature
                .repos_of(project)
                .map(|r| r.worktree.clone())
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
    for worktree in folder.worktrees.iter().filter(|w| w.is_dir()) {
        if git(worktree, &["check-ignore", "-q", &ignored]).is_ok() {
            continue;
        }
        if let Some(exclude) = exclude_line(worktree, &line)? {
            excludes.push(exclude);
        }
    }
    Ok(())
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
            worktrees: vec![dir.to_path_buf()],
        }]
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
            worktrees: vec![api.clone(), web.clone()],
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
                worktrees: vec![tree],
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
}

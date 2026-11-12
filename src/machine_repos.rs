//! Finding a project's repositories on another machine under other folder
//! names, before gerdr makes empty ones where it thinks they go.
//!
//! A repository goes to the same place under the home there unless the
//! project's file names another (`machine_folders`). A project cloned under
//! other names there got repositories of gerdr's own beside the real ones, and
//! ignored files like `.env` never came with them. Once a link is up, gerdr
//! lists the repositories under the home on both machines with their remotes
//! and first commits. Each one the project uses on one side, with no clone of
//! it where the other side looks, is paired with the clone there: by a remote
//! both have (as `git@host:o/r.git` and `https://host/o/r` are one), else by a
//! first commit only one of them has. What is found goes into the project's
//! file here, for the user to see and change. Two clones that both fit are
//! said, and left for the user to name.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

use crate::{
    Gerdr,
    desktop::shell_quote,
    feature::{
        Status,
        remote::{self, Paths, Reach},
    },
    features::Done,
};

/// How deep under a home repositories are looked for: `~/Work/team/app` is
/// three folders down.
const DEPTH: u32 = 5;

/// Folders a clone of a project isn't in, and that are big to walk.
const SKIPPED: &[&str] = &[
    "node_modules",
    ".venv",
    "venv",
    ".cache",
    ".local",
    ".cargo",
    ".rustup",
    ".npm",
    ".features",
    "target",
];

/// A repository found on a machine.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub path: String,
    /// Its remotes' URLs, as `same_url` reads them.
    pub urls: BTreeSet<String>,
    /// Its first commits.
    pub roots: BTreeSet<String>,
}

impl Found {
    /// Whether `other` is a clone of the same repository: a remote, or else
    /// a first commit, in common.
    fn same_as(&self, other: &Found) -> bool {
        !self.urls.is_disjoint(&other.urls) || !self.roots.is_disjoint(&other.roots)
    }
}

/// `url` as one of the same repository reads, whichever way it is written:
/// no scheme, user, `.git` or slash at its end, `host:path` as `host/path`,
/// in lower case.
pub fn same_url(url: &str) -> String {
    let url = url.trim();
    let (rest, scp) = match url.split_once("://") {
        Some((_, rest)) => (rest, false),
        None => (url, !url.starts_with('/') && !url.starts_with('.')),
    };
    let rest = rest.rsplit_once('@').map_or(rest, |(_, after)| after);
    let rest = match scp {
        true => rest.replacen(':', "/", 1),
        false => rest.to_owned(),
    };
    let rest = rest.trim_end_matches('/');
    rest.strip_suffix(".git").unwrap_or(rest).to_lowercase()
}

/// The script listing the repositories under `root`, and those at `extra`
/// wherever they are: `R <path>`, then a `U <url>` per remote and a `C <commit>`
/// per first commit.
pub fn scan_script(root: &str, extra: &[String]) -> String {
    let skipped: Vec<String> = SKIPPED.iter().map(|name| format!("-name {name}")).collect();
    let mut script = String::from(
        r#"describe() {
  [ -e "$1/.git" ] || return 0
  printf 'R %s\n' "$1"
  git -C "$1" remote -v 2>/dev/null | while read -r _ u _; do printf 'U %s\n' "$u"; done | sort -u
  git -C "$1" rev-list --max-parents=0 HEAD 2>/dev/null | sed 's/^/C /'
}
"#,
    );
    for path in extra {
        script.push_str(&format!("describe {}\n", shell_quote(path)));
    }
    script.push_str(&format!(
        "find {} -maxdepth {DEPTH} \\( {} \\) -prune -o -type d -name .git -print -prune 2>/dev/null | while IFS= read -r g; do describe \"${{g%/.git}}\"; done\nexit 0\n",
        shell_quote(root),
        skipped.join(" -o "),
    ));
    script
}

/// What `scan_script` said, by path.
pub fn parse_scan(text: &str) -> BTreeMap<String, Found> {
    let mut found: BTreeMap<String, Found> = BTreeMap::new();
    let mut at: Option<String> = None;
    for line in text.lines() {
        if let Some(path) = line.strip_prefix("R ") {
            found.entry(path.to_owned()).or_insert_with(|| Found {
                path: path.to_owned(),
                ..Found::default()
            });
            at = Some(path.to_owned());
            continue;
        }
        let Some(clone) = at.as_ref().and_then(|path| found.get_mut(path)) else {
            continue;
        };
        if let Some(url) = line.strip_prefix("U ") {
            clone.urls.insert(same_url(url));
        } else if let Some(commit) = line.strip_prefix("C ") {
            clone.roots.insert(commit.trim().to_owned());
        }
    }
    found
}

/// What looking for a project's repositories on a machine found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Paired {
    /// Folders here, and the clone of each there.
    pub pairs: Vec<(String, String)>,
    /// A folder (here or there), and the clones that fit it on the other side.
    pub unsure: Vec<(String, Vec<String>)>,
}

/// The one clone in `clones` that `of` is the same as: by a remote first, by a
/// first commit only when no remote fits. Several that fit are `Err`.
fn match_in<'a>(of: &Found, clones: &'a BTreeMap<String, Found>) -> Result<Option<&'a Found>, Vec<String>> {
    let by_url: Vec<&Found> = (clones.values())
        .filter(|c| !of.urls.is_disjoint(&c.urls))
        .collect();
    let fits: Vec<&Found> = match by_url.is_empty() {
        true => (clones.values())
            .filter(|c| !of.roots.is_disjoint(&c.roots))
            .collect(),
        false => by_url,
    };
    match fits.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(one)),
        many => Err(many.iter().map(|c| c.path.clone()).collect()),
    }
}

/// Pair what the project uses on each side with its clone on the other.
/// `used_here`: its repositories here; `seen`: those of features another
/// machine writes, as `paths` takes them there. `here` and `there`: what
/// `scan_script` found on each machine.
pub fn pair(
    used_here: &[PathBuf],
    seen: &[PathBuf],
    here: &BTreeMap<String, Found>,
    there: &BTreeMap<String, Found>,
    paths: &Paths,
) -> Paired {
    let mut out = Paired::default();
    let add = |out: &mut Paired, pair: (String, String)| {
        if !out.pairs.contains(&pair) {
            out.pairs.push(pair);
        }
    };
    for local in used_here {
        let key = local.to_string_lossy().into_owned();
        let Some(mine) = here.get(&key) else {
            continue;
        };
        let target = paths.there(local);
        if there.get(&target).is_some_and(|c| c.same_as(mine)) {
            continue;
        }
        match match_in(mine, there) {
            Ok(Some(clone)) if clone.path != target => add(&mut out, (key, clone.path.clone())),
            Ok(_) => {}
            Err(many) => out.unsure.push((key, many)),
        }
    }
    for local in seen {
        let key = local.to_string_lossy().into_owned();
        if here.contains_key(&key) {
            continue;
        }
        let target = paths.there(local);
        let Some(theirs) = there.get(&target) else {
            continue;
        };
        match match_in(theirs, here) {
            Ok(Some(clone)) => add(&mut out, (clone.path.clone(), target)),
            Ok(None) => {}
            Err(many) => out.unsure.push((target, many)),
        }
    }
    out
}

/// What a look for repositories needs off the thread.
struct Look {
    reach: Reach,
    home_there: String,
    paths: Paths,
    used_here: Vec<PathBuf>,
    seen: Vec<PathBuf>,
}

impl Look {
    fn go(&self) -> Result<Paired, String> {
        let never = AtomicBool::new(false);
        let home_here = crate::home_dir()
            .ok_or_else(|| "this machine's home isn't known".to_owned())?
            .to_string_lossy()
            .into_owned();
        let extra: Vec<String> = (self.used_here.iter())
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let here = remote::local_script(&scan_script(&home_here, &extra), &never)
            .map_err(|trouble| trouble.text().to_owned())?;
        let wanted: Vec<String> = (self.used_here.iter().chain(&self.seen))
            .map(|p| self.paths.there(p))
            .collect();
        let there = (self.reach)
            .script(&scan_script(&self.home_there, &wanted), &never)
            .map_err(|trouble| trouble.text().to_owned())?;
        Ok(pair(
            &self.used_here,
            &self.seen,
            &parse_scan(&here),
            &parse_scan(&there),
            &self.paths,
        ))
    }
}

/// `path` with `home` as `~`.
fn tilde(path: &str, home: &str) -> String {
    // An empty home is one not known: every path would be under it.
    crate::paths::tilde(Path::new(path), (!home.is_empty()).then(|| Path::new(home)))
}

impl Gerdr {
    /// The repositories the open project uses here, and those of the features
    /// other machines write that aren't here under the names they come as.
    fn project_repos(&self, project: &str) -> (Vec<PathBuf>, Vec<PathBuf>) {
        let is_repo = |dir: &Path| dir.join(".git").exists();
        let mut used: BTreeSet<PathBuf> = (self.workspaces.iter())
            .map(|ws| ws.dir.clone())
            .filter(|dir| is_repo(dir))
            .collect();
        let open = |f: &&crate::feature::Feature| f.status == Status::Open;
        let mains = |f: &crate::feature::Feature| -> Vec<PathBuf> {
            (f.repos.iter().map(|r| r.main.clone()))
                .chain(f.context.iter().map(|c| c.main.clone()))
                .collect()
        };
        for feature in self.features.all.get(project).into_iter().flatten().filter(open) {
            used.extend(mains(feature).into_iter().filter(|dir| is_repo(dir)));
        }
        let seen: BTreeSet<PathBuf> = (self.viewed_of(project).iter())
            .filter(open)
            .flat_map(mains)
            .filter(|dir| !is_repo(dir))
            .collect();
        (used.into_iter().collect(), seen.into_iter().collect())
    }

    /// Now and then: on each machine connected, look for the open project's
    /// repositories under other names, once for what the project uses.
    pub(crate) fn same_repos_tick(&mut self) {
        let Some(project) = self.project.clone() else {
            return;
        };
        let (used, seen) = self.project_repos(&project);
        if used.is_empty() && seen.is_empty() {
            return;
        }
        let asked: Vec<PathBuf> = used.iter().chain(&seen).cloned().collect();
        let ids: Vec<String> = (self.machines.links.values())
            .filter_map(|link| link.work.as_ref())
            .map(|work| work.id.clone())
            .collect();
        for id in ids {
            let key = (id.clone(), project.clone());
            if self.machines.repos_looked.get(&key) == Some(&asked) {
                continue;
            }
            let (Some(work), Some(paths)) =
                (self.machines.work_of(&id), self.machine_paths(&id, None))
            else {
                continue;
            };
            let look = Look {
                reach: work.reach(),
                home_there: work.info.site.home.clone(),
                paths,
                used_here: used.clone(),
                seen: seen.clone(),
            };
            self.machines.repos_looked.insert(key, asked.clone());
            let project = project.clone();
            self.feature_work(None, move |_| Done::SameRepos {
                id,
                project,
                result: look.go(),
            });
        }
    }

    /// A look for the project's repositories on machine `id` ended: what it
    /// paired goes into the project's file, and the user hears of it.
    pub(crate) fn same_repos_found(
        &mut self,
        id: &str,
        project: &str,
        result: Result<Paired, String>,
    ) {
        let name = self.machines.machine_name(id);
        let paired = match result {
            Ok(paired) => paired,
            Err(why) => {
                tracing::warn!("{project}: looking for its repositories on {name}: {why}");
                return;
            }
        };
        let home_here = crate::home_dir()
            .map(|h| h.to_string_lossy().into_owned())
            .unwrap_or_default();
        let home_there = (self.machines.work_of(id))
            .map(|w| w.info.site.home.clone())
            .unwrap_or_default();
        let known = (self.projects.project(project))
            .and_then(|p| p.workflow.machine_folders.get(&name))
            .cloned()
            .unwrap_or_default();
        let new: Vec<(String, String)> = (paired.pairs.into_iter())
            .filter(|(here, _)| !known.contains_key(here))
            .collect();
        if !new.is_empty() {
            if self.projects.file_changed(project) {
                // Asked again once the file is read back.
                self.machines.repos_looked.remove(&(id.to_owned(), project.to_owned()));
                self.show_warning(format!(
                    "The project's file changed outside gerdr: reload it, and gerdr pairs its repositories on {name} again"
                ));
                return;
            }
            if let Some(p) = self.projects.project_mut(project) {
                let folders = p.workflow.machine_folders.entry(name.clone()).or_default();
                folders.extend(new.iter().cloned());
            }
            let said: Vec<String> = (new.iter())
                .map(|(here, there)| {
                    format!("{} is {}", tilde(here, &home_here), tilde(there, &home_there))
                })
                .collect();
            tracing::info!("{project}: paired on {name}: {new:?}");
            if self.projects.save_projects(&[project.to_owned()]) {
                self.show_success(format!(
                    "Same repositories on {name}: {}. Kept under machine_folders in the project's file",
                    said.join(", ")
                ));
            } else {
                self.show_error(format!(
                    "Same repositories on {name} ({}) couldn't be saved in the project's file",
                    said.join(", ")
                ));
            }
        }
        for (folder, many) in paired.unsure {
            let list: Vec<String> = many.iter().map(|p| tilde(p, &home_there)).collect();
            tracing::info!("{project}: more than one clone of {folder} on {name}: {many:?}");
            self.show_warning(format!(
                "{} has more than one clone on {name} ({}): name one under machine_folders in the project's file",
                tilde(&folder, &home_here),
                list.join(", ")
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::remote::Side;

    #[test]
    fn a_remote_reads_the_same_however_it_is_written() {
        let one = "github.com/assettag-technologies/assettag-platform";
        for url in [
            "git@github.com:AssetTag-Technologies/assettag-platform.git",
            "https://github.com/AssetTag-Technologies/assettag-platform",
            "https://user@github.com/AssetTag-Technologies/assettag-platform.git/",
            "ssh://git@github.com/AssetTag-Technologies/assettag-platform.git",
        ] {
            assert_eq!(same_url(url), one, "{url}");
        }
        assert_eq!(same_url("/srv/git/app.git"), "/srv/git/app");
        assert_ne!(same_url("git@github.com:o/a.git"), same_url("git@github.com:o/b.git"));
    }

    fn clone(path: &str, urls: &[&str], roots: &[&str]) -> (String, Found) {
        let clone = Found {
            path: path.into(),
            urls: urls.iter().map(|u| same_url(u)).collect(),
            roots: roots.iter().map(|r| r.to_string()).collect(),
        };
        (path.into(), clone)
    }

    fn paths() -> Paths {
        let side = |home: &str| Side {
            home: home.into(),
            data: format!("{home}/.local/share/gerdr"),
        };
        Paths::new(&side("/home/lg"), &side("/home/zb"), &BTreeMap::new(), None)
    }

    const ORIGIN: &str = "git@github.com:o/platform.git";

    #[test]
    fn a_repository_used_here_is_paired_with_its_clone_there() {
        let here = BTreeMap::from([clone("/home/lg/Work/AssetTag/platform", &[ORIGIN], &["r1"])]);
        let there = BTreeMap::from([
            clone("/home/zb/Work/Asset-Track", &["https://github.com/o/platform"], &["r1"]),
            clone("/home/zb/Work/other", &["git@github.com:o/other.git"], &["r9"]),
            // What gerdr made where it thought it goes: no commits, no remote.
            clone("/home/zb/Work/AssetTag/platform", &[], &[]),
        ]);
        let used = [PathBuf::from("/home/lg/Work/AssetTag/platform")];
        let paired = pair(&used, &[], &here, &there, &paths());
        assert_eq!(
            paired.pairs,
            [("/home/lg/Work/AssetTag/platform".into(), "/home/zb/Work/Asset-Track".into())]
        );
        assert!(paired.unsure.is_empty());
    }

    #[test]
    fn a_clone_where_it_is_looked_for_needs_nothing() {
        let here = BTreeMap::from([clone("/home/lg/Work/app", &[ORIGIN], &["r1"])]);
        let there = BTreeMap::from([
            clone("/home/zb/Work/app", &[ORIGIN], &["r1"]),
            clone("/home/zb/old/app", &[ORIGIN], &["r1"]),
        ]);
        let paired = pair(&[PathBuf::from("/home/lg/Work/app")], &[], &here, &there, &paths());
        assert_eq!(paired, Paired::default());
    }

    #[test]
    fn a_first_commit_pairs_only_without_a_remote_and_only_one() {
        let here = BTreeMap::from([clone("/home/lg/app", &[], &["r1"])]);
        let one = BTreeMap::from([clone("/home/zb/code/app", &[], &["r1"])]);
        let used = [PathBuf::from("/home/lg/app")];
        assert_eq!(
            pair(&used, &[], &here, &one, &paths()).pairs,
            [("/home/lg/app".into(), "/home/zb/code/app".into())]
        );
        let two = BTreeMap::from([
            clone("/home/zb/code/app", &[], &["r1"]),
            clone("/home/zb/fork/app", &[], &["r1"]),
        ]);
        let paired = pair(&used, &[], &here, &two, &paths());
        assert!(paired.pairs.is_empty());
        assert_eq!(
            paired.unsure,
            [("/home/lg/app".into(), vec!["/home/zb/code/app".into(), "/home/zb/fork/app".into()])]
        );
        // A remote in common goes over first commits only.
        let here = BTreeMap::from([clone("/home/lg/app", &[ORIGIN], &["r1"])]);
        let both = BTreeMap::from([
            clone("/home/zb/code/app", &[ORIGIN], &["r1"]),
            clone("/home/zb/fork/app", &["git@github.com:me/platform.git"], &["r1"]),
        ]);
        assert_eq!(
            pair(&used, &[], &here, &both, &paths()).pairs,
            [("/home/lg/app".into(), "/home/zb/code/app".into())]
        );
    }

    #[test]
    fn a_repository_of_a_feature_written_there_is_paired_with_its_clone_here() {
        // Written there in ~/Work/Asset-Track, which here is a clone of another name.
        let here = BTreeMap::from([clone("/home/lg/Work/AssetTag/platform", &[ORIGIN], &["r1"])]);
        let there = BTreeMap::from([clone("/home/zb/Work/Asset-Track", &[ORIGIN], &["r1"])]);
        let seen = [PathBuf::from("/home/lg/Work/Asset-Track")];
        let paired = pair(&[], &seen, &here, &there, &paths());
        assert_eq!(
            paired.pairs,
            [("/home/lg/Work/AssetTag/platform".into(), "/home/zb/Work/Asset-Track".into())]
        );
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = crate::git::command(dir, args).output().unwrap();
        assert!(out.status.success(), "{out:?}");
    }

    #[test]
    fn the_scan_finds_clones_with_their_remotes_and_first_commits() {
        let t = crate::feature::tests::Temp::new();
        let home = t.0.as_path();
        let app = home.join("Work/team/app");
        std::fs::create_dir_all(&app).unwrap();
        git(&app, &["init", "-q"]);
        git(&app, &["-c", "user.name=a", "-c", "user.email=a@b", "commit", "-q", "--allow-empty", "-m", "one"]);
        git(&app, &["remote", "add", "origin", ORIGIN]);
        git(&app, &["remote", "add", "mine", "https://github.com/me/platform.git"]);
        // Not looked in, and not a project's clone.
        let hidden = home.join("Work/team/app/node_modules/dep");
        std::fs::create_dir_all(&hidden).unwrap();
        git(&hidden, &["init", "-q"]);
        let empty = home.join("Work/empty");
        std::fs::create_dir_all(&empty).unwrap();
        git(&empty, &["init", "-q"]);
        let outside = t.0.join("elsewhere");
        let never = AtomicBool::new(false);
        let script = scan_script(&home.to_string_lossy(), &[app.to_string_lossy().into_owned(), outside.to_string_lossy().into_owned()]);
        let found = parse_scan(&remote::local_script(&script, &never).unwrap());
        let paths: Vec<&str> = found.keys().map(String::as_str).collect();
        let app_key = app.to_string_lossy().into_owned();
        let empty_key = empty.to_string_lossy().into_owned();
        assert_eq!(paths, [empty_key.as_str(), app_key.as_str()]);
        let app = &found[&app_key];
        assert_eq!(
            app.urls,
            BTreeSet::from([same_url(ORIGIN), same_url("https://github.com/me/platform.git")])
        );
        assert_eq!(app.roots.len(), 1);
        assert!(found[&empty_key].roots.is_empty() && found[&empty_key].urls.is_empty());
    }

    #[test]
    fn a_home_reads_as_a_tilde() {
        assert_eq!(tilde("/home/zb/Work/x", "/home/zb"), "~/Work/x");
        assert_eq!(tilde("/home/zbx/Work", "/home/zb"), "/home/zbx/Work");
        assert_eq!(tilde("/srv/x", ""), "/srv/x");
    }
}

//! A feature's own workspace: the workspace of its home project open on the
//! feature folder. Its agents, squash tab, Zed and pull request pages open
//! there, not in whichever workspace is active. It is a normal Terminal
//! workspace in every other way (close, rename, restart, agent cards,
//! inbox), found again by its folder: no record says it is a feature's.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use smithay::reexports::calloop::timer::{TimeoutAction, Timer};

use crate::{
    Gerdr, Workspace, WorkspaceKind,
    feature::{Feature, Status},
};

/// How often a removal waiting for a closed feature workspace's panes to
/// go looks again, and how long it waits at most.
const GONE_POLL: Duration = Duration::from_millis(100);
const GONE_WAIT: Duration = Duration::from_secs(5);

/// Whether folders `a` and `b` are the same, through symlinks too. Only
/// folders of the same name are resolved, so most pairs cost nothing.
pub(crate) fn same_dir(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    if a.file_name() != b.file_name() {
        return false;
    }
    matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

/// The first of `dirs` (workspace folders, in order) that is `dir`.
pub(crate) fn workspace_on<'a>(
    dirs: impl IntoIterator<Item = &'a Path>,
    dir: &Path,
) -> Option<usize> {
    dirs.into_iter().position(|d| same_dir(d, dir))
}

/// The feature of `all` (one project's) whose workspace is the one on
/// `dir`: any not abandoned, a shipped one whose workspace stayed open too.
pub(crate) fn feature_on<'a>(all: &'a [Feature], dir: &Path) -> Option<&'a Feature> {
    all.iter()
        .filter(|f| f.status != Status::Abandoned)
        .find(|f| same_dir(&f.dir, dir))
}

/// Those of `features` with no workspace among `dirs`: the sidebar's
/// Features section lists them, the others are their workspace's header.
pub(crate) fn without_workspace<'a>(
    features: Vec<&'a Feature>,
    dirs: &[&Path],
) -> Vec<&'a Feature> {
    features
        .into_iter()
        .filter(|f| workspace_on(dirs.iter().copied(), &f.dir).is_none())
        .collect()
}

/// What the sidebar calls a workspace on `dir` named `name`, feature
/// `title`'s: the title, unless it was renamed from its folder's name.
pub(crate) fn feature_label(name: &str, dir: &Path, title: &str) -> String {
    if name == Workspace::new(dir.to_path_buf(), WorkspaceKind::Terminal).name {
        title.to_owned()
    } else {
        name.to_owned()
    }
}

/// What a toast says of a workspace kept open because `running` runs in
/// it.
pub(crate) fn kept_text(running: &[String]) -> String {
    match running {
        [one] => format!("its workspace stays open: \u{201c}{one}\u{201d} runs in it"),
        more => format!(
            "its workspace stays open: {} commands run in it",
            more.len()
        ),
    }
}

/// What the confirm to remove a feature says of its open workspace, with
/// `running` running in it.
pub(crate) fn closing_text(running: &[String]) -> String {
    match running {
        [] => "Its workspace closes".to_owned(),
        [one] => format!("Its workspace closes, and \u{201c}{one}\u{201d} running in it stops"),
        more => format!(
            "Its workspace closes, and the {} commands running in it stop",
            more.len()
        ),
    }
}

impl Gerdr {
    /// The index of feature `slug` of `project`'s workspace, if one is open:
    /// a workspace of the shown project on the feature's folder.
    pub fn feature_workspace(&self, project: &str, slug: &str) -> Option<usize> {
        if self.project.as_deref() != Some(project) {
            return None;
        }
        let dir = &self.feature(project, slug)?.dir;
        workspace_on(self.workspaces.iter().map(|ws| ws.dir.as_path()), dir)
    }

    /// The feature whose workspace workspace `w` is, if any.
    pub(crate) fn workspace_feature(&self, w: usize) -> Option<&Feature> {
        let ws = self.workspaces.get(w)?;
        let all = self.features.all.get(self.project.as_ref()?)?;
        feature_on(all, &ws.dir)
    }

    /// What runs in workspace `w`'s panes, as their bars say it.
    pub(crate) fn workspace_running(&self, w: usize) -> Vec<String> {
        let Some(ws) = self.workspaces.get(w) else {
            return Vec::new();
        };
        let panes = ws.tabs.iter().flat_map(|tab| &tab.panes);
        panes.filter_map(|p| self.running_in(p)).collect()
    }

    /// Run `open` on feature `slug` of `project`'s workspace, which adds a
    /// tab to it and shows it. With none open, a Terminal workspace on the
    /// feature's folder, called after the feature, opens first, empty, so
    /// that `open`'s tab is its first; it goes again if `open` added none.
    /// Only the shown project's features have workspaces: another's says
    /// which project to switch to. `None` (with a toast) when it couldn't.
    pub(crate) fn in_feature_workspace<R>(
        &mut self,
        project: &str,
        slug: &str,
        open: impl FnOnce(&mut Self, usize) -> R,
    ) -> Option<R> {
        let Some(feature) = self.feature(project, slug) else {
            self.show_warning("That feature is gone".into());
            return None;
        };
        let (title, dir) = (feature.title.clone(), feature.dir.clone());
        if self.project.as_deref() != Some(project) {
            let name = self
                .projects
                .project(project)
                .map_or_else(|| project.to_owned(), |p| p.name.clone());
            self.show_error(format!(
                "\u{201c}{title}\u{201d} is a feature of {name}: switch to that project to open its tabs"
            ));
            return None;
        }
        if let Some(w) = self.feature_workspace(project, slug) {
            return Some(open(self, w));
        }
        if !dir.is_dir() {
            self.show_error(format!(
                "Couldn't open the workspace of \u{201c}{title}\u{201d}: {} is gone",
                dir.display()
            ));
            return None;
        }
        let before = self.active_workspace;
        let record = self.open_feature_workspace(dir, &title);
        let w = self.workspaces.len() - 1;
        let out = open(self, w);
        // Nothing opened in it: it goes, its record too, and what was
        // shown shows again.
        let kept = self
            .workspaces
            .iter()
            .any(|ws| ws.record == record && !ws.tabs.is_empty());
        if !kept {
            self.prune();
            self.forget_workspace(&record);
            self.switch_workspace(before);
        }
        Some(out)
    }

    /// Add an empty Terminal workspace on feature folder `dir` called
    /// `title`, last, with a record of it: the record's id.
    fn open_feature_workspace(&mut self, dir: PathBuf, title: &str) -> String {
        let dir = dir.canonicalize().unwrap_or(dir);
        let kind = WorkspaceKind::Terminal;
        let mut ws = Workspace::new(dir, kind);
        ws.name = title.to_owned();
        ws.record = self.remember_workspace(&ws.name, kind, &ws.dir, None);
        let record = ws.record.clone();
        self.workspaces.push(ws);
        record
    }

    /// Open a tab in feature `slug` of `project`'s workspace, whose shell
    /// starts in `cd` running `command` if any, and show it; the workspace
    /// opens with that tab first when none is open. The herdr pane of the
    /// tab's terminal, when herdr says; `None` (with a toast) when it
    /// couldn't open.
    pub fn open_feature_tab(
        &mut self,
        project: &str,
        slug: &str,
        cd: &Path,
        command: Option<String>,
    ) -> Option<String> {
        self.in_feature_workspace(project, slug, |gerdr, w| gerdr.new_tab_pane(w, cd, command))
            .flatten()
    }

    /// Open a tab laid out like a project's first (Zed over a terminal,
    /// both on `dir`) in feature `slug` of `project`'s workspace, as
    /// `open_feature_tab` opens one.
    pub(crate) fn open_feature_project_tab(&mut self, project: &str, slug: &str, dir: &Path) {
        self.in_feature_workspace(project, slug, |gerdr, w| gerdr.new_project_tab(w, dir));
    }

    /// Open page `url` in feature `slug` of `project`'s workspace (a tab
    /// showing it already, else a new one), as `open_feature_tab` opens a
    /// tab.
    pub(crate) fn open_feature_page(&mut self, project: &str, slug: &str, url: String) {
        self.in_feature_workspace(project, slug, |gerdr, w| gerdr.open_page(w, url));
    }

    /// What is in feature `slug`'s folder other than the feature's own
    /// workspace, if anything (see `folder_in_use`).
    pub(crate) fn feature_folder_in_use(&self, project: &str, slug: &str) -> Option<String> {
        let dir = &self.feature(project, slug)?.dir;
        self.folder_in_use_except(dir, self.feature_workspace(project, slug))
    }

    /// Close feature `slug` of `project`'s workspace if open, without
    /// asking, then run `then` once nothing is in its folder any more: its
    /// panes closed, their shells gone. Removing its worktrees under a
    /// shell would work, but an agent still writing there could leave
    /// files behind and fail it. After `GONE_WAIT`, what is still there
    /// (another terminal cd'd there meanwhile) stops it, and says so.
    pub(crate) fn close_feature_workspace_then(
        &mut self,
        project: &str,
        slug: &str,
        then: impl FnOnce(&mut Gerdr) + 'static,
    ) {
        let Some((title, dir)) = self
            .feature(project, slug)
            .map(|f| (f.title.clone(), f.dir.clone()))
        else {
            return;
        };
        let Some(w) = self.feature_workspace(project, slug) else {
            return then(self);
        };
        self.close_workspace_now(w);
        if self.folder_in_use(&dir).is_none() {
            return then(self);
        }
        let until = Instant::now() + GONE_WAIT;
        let mut then = Some(then);
        let wait = move |_, _: &mut (), gerdr: &mut Gerdr| {
            let user = gerdr.folder_in_use(&dir);
            if user.is_some() && Instant::now() < until {
                return TimeoutAction::ToDuration(GONE_POLL);
            }
            match user {
                Some(user) => {
                    tracing::warn!("{} still in use after closing its workspace", dir.display());
                    gerdr.show_warning(format!(
                        "\u{201c}{title}\u{201d} has {user} in it; close it first"
                    ));
                }
                None => {
                    if let Some(then) = then.take() {
                        then(gerdr);
                    }
                }
            }
            TimeoutAction::Drop
        };
        if let Err(err) = self
            .loop_handle
            .insert_source(Timer::from_duration(GONE_POLL), wait)
        {
            tracing::warn!("waiting for a feature workspace to close: {err}");
            self.show_error(format!(
                "Couldn't wait for the workspace of \u{201c}{}\u{201d} to close: {err}",
                self.feature(project, slug).map_or("", |f| f.title.as_str())
            ));
        }
    }

    /// Ship of feature `slug` of `project` is done: its workspace closes
    /// when nothing runs in it. What the success toast adds when it stays
    /// open, if anything.
    pub(crate) fn close_shipped_workspace(&mut self, project: &str, slug: &str) -> Option<String> {
        let w = self.feature_workspace(project, slug)?;
        let running = self.workspace_running(w);
        if running.is_empty() {
            self.close_workspace_now(w);
            return None;
        }
        Some(kept_text(&running))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::tests::{feature, repo};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gerdr-fws-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_feature_workspace_is_found_by_its_folder_through_symlinks() {
        let root = scratch("find");
        let dir = root.join(".features").join("bulk");
        std::fs::create_dir_all(&dir).unwrap();
        let link = root.join("bulk");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        let other = root.join("other");
        std::fs::create_dir_all(&other).unwrap();
        let dirs = [other.as_path(), link.as_path()];
        assert_eq!(workspace_on(dirs, &dir), Some(1));
        assert_eq!(workspace_on([other.as_path()], &dir), None);
        // Gone folders still match by their path.
        let gone = root.join("gone");
        assert_eq!(workspace_on([gone.as_path()], &gone), Some(0));
        assert!(!same_dir(&gone, &root.join("x").join("gone")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn features_with_a_workspace_leave_the_features_section() {
        let mut a = feature("a", vec![repo("api", true)], 1);
        a.dir = "/work/.features/a".into();
        let mut b = feature("b", vec![repo("api", true)], 1);
        b.dir = "/work/.features/b".into();
        let all = [a, b];
        let dirs = [Path::new("/home"), Path::new("/work/.features/b")];
        let left: Vec<&str> = without_workspace(all.iter().collect(), &dirs)
            .iter()
            .map(|f| f.slug.as_str())
            .collect();
        assert_eq!(left, ["a"]);
        assert_eq!(
            feature_on(&all, Path::new("/work/.features/b")).map(|f| f.slug.as_str()),
            Some("b")
        );
        let mut abandoned = all.clone();
        abandoned[1].status = Status::Abandoned;
        assert!(feature_on(&abandoned, Path::new("/work/.features/b")).is_none());
    }

    #[test]
    fn a_feature_workspace_is_called_after_the_feature_unless_renamed() {
        let dir = Path::new("/work/.features/bulk");
        assert_eq!(feature_label("bulk", dir, "Bulk export"), "Bulk export");
        assert_eq!(feature_label("mine", dir, "Bulk export"), "mine");
        assert_eq!(
            kept_text(&["claude".into()]),
            "its workspace stays open: \u{201c}claude\u{201d} runs in it"
        );
        assert_eq!(
            kept_text(&["claude".into(), "npm".into()]),
            "its workspace stays open: 2 commands run in it"
        );
        assert_eq!(closing_text(&[]), "Its workspace closes");
        assert_eq!(
            closing_text(&["claude".into()]),
            "Its workspace closes, and \u{201c}claude\u{201d} running in it stops"
        );
    }
}

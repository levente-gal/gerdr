//! A feature's own workspace: the workspace of its home project open on the
//! feature folder. Its agents, squash tab, Zed and pull request pages open
//! there, not in whichever workspace is active. It is a normal Terminal
//! workspace in every other way (close, rename, restart, agent cards,
//! inbox), found again by its folder: no record says it is a feature's.
//!
//! Its first tab is pinned: it holds the feature panel, takes no panes,
//! and can't be closed or moved, nor another tab put before it. The
//! workspace stays open with only that tab; closing the workspace, Ship
//! and Abandon close it.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use smithay::reexports::calloop::timer::{TimeoutAction, Timer};

use crate::{
    Gerdr, Tab, TabKind, Workspace, WorkspaceKind,
    feature::{AfterStep, Feature, Status, layout, store},
    features::Done,
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

/// Give `tabs` a pinned first tab if `want`, else take every pinned one
/// out, keeping `active` on the tab it was on. Whether they changed.
pub(crate) fn pin_tab(tabs: &mut Vec<Tab>, active: &mut usize, want: bool) -> bool {
    let has = tabs.first().is_some_and(Tab::is_feature);
    if want && !has {
        let had = !tabs.is_empty();
        tabs.insert(
            0,
            Tab {
                kind: TabKind::Feature,
                ..Tab::default()
            },
        );
        if had {
            *active += 1;
        }
        return true;
    }
    if !want && tabs.iter().any(|t| !t.takes_panes()) {
        let before = tabs.len();
        let mut i = 0;
        tabs.retain(|t| {
            let keep = t.takes_panes();
            if !keep && i < *active {
                *active -= 1;
            }
            i += 1;
            keep
        });
        *active = (*active).min(tabs.len().saturating_sub(1));
        return tabs.len() != before;
    }
    false
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
        let project = self.project.as_ref()?;
        let own = self.features.all.get(project).map(|all| feature_on(all, &ws.dir));
        own.flatten()
            .or_else(|| feature_on(self.viewed_of(project), &ws.dir))
    }

    /// What runs in workspace `w`'s panes, as their bars say it.
    pub(crate) fn workspace_running(&self, w: usize) -> Vec<String> {
        self.workspace_running_but(w, None)
    }

    /// What runs in workspace `w`'s panes but herdr pane `but`.
    fn workspace_running_but(&self, w: usize, but: Option<&str>) -> Vec<String> {
        let Some(ws) = self.workspaces.get(w) else {
            return Vec::new();
        };
        let panes = ws.tabs.iter().flat_map(|tab| &tab.panes);
        let left =
            panes.filter(|p| but.is_none() || p.herdr.as_ref().map(|h| h.pane_id.as_str()) != but);
        left.filter_map(|p| self.running_in(p)).collect()
    }

    /// Feature `slug` of `project`'s workspace, opened on the feature's
    /// folder and called after the feature when none is open, with
    /// whether it was just opened. Only the shown project's features have
    /// workspaces: another's says which project to switch to. `None` (with
    /// a toast) when it couldn't.
    pub(crate) fn ensure_feature_workspace(
        &mut self,
        project: &str,
        slug: &str,
    ) -> Option<(usize, bool)> {
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
            return Some((w, false));
        }
        // Written elsewhere, its folder here comes with its code, once asked for.
        let seen = self.feature_writer(project, slug).is_some();
        if seen && let Err(err) = std::fs::create_dir_all(&dir) {
            tracing::warn!("making {}: {err}", dir.display());
        }
        if !dir.is_dir() {
            self.show_error(format!(
                "Couldn't open the workspace of \u{201c}{title}\u{201d}: {} is gone",
                dir.display()
            ));
            return None;
        }
        self.open_feature_workspace(dir, &title);
        Some((self.workspaces.len() - 1, true))
    }

    /// Run `open` on feature `slug` of `project`'s workspace, which adds a
    /// tab to it. With none open, one opens first with only
    /// its panel tab (see `ensure_feature_workspace`); it goes again if
    /// `open` added none. `None` (with a toast) when it couldn't.
    pub(crate) fn in_feature_workspace<R>(
        &mut self,
        project: &str,
        slug: &str,
        open: impl FnOnce(&mut Self, usize) -> R,
    ) -> Option<R> {
        let before = self.active_workspace;
        let (w, new) = self.ensure_feature_workspace(project, slug)?;
        let record = self.workspaces[w].record.clone();
        let out = open(self, w);
        // Nothing opened in it: it goes, its record too, and what was
        // shown shows again.
        let kept = self
            .workspaces
            .iter()
            .any(|ws| ws.record == record && ws.tabs.iter().any(|t| !t.is_feature()));
        if new && !kept {
            self.closing_records.insert(record.clone());
            self.prune();
            self.forget_workspace(&record);
            self.switch_workspace(before);
        }
        Some(out)
    }

    /// Show feature `slug` of `project`'s panel tab, opening its workspace
    /// if none is. Whether it shows.
    pub(crate) fn show_feature_tab(&mut self, project: &str, slug: &str) -> bool {
        let Some((w, _)) = self.ensure_feature_workspace(project, slug) else {
            return false;
        };
        self.switch_workspace(w);
        self.switch_tab(0);
        true
    }

    /// Give every feature workspace its panel tab, first, and take it from
    /// every other workspace and from one closing. Whether a tab came or
    /// went.
    pub(crate) fn pin_feature_tabs(&mut self) -> bool {
        let mut changed = false;
        for w in 0..self.workspaces.len() {
            let closing = self.closing_records.contains(&self.workspaces[w].record);
            let want = !closing && self.workspace_feature(w).is_some();
            // A file's tab goes with its feature: its page's window too.
            if !want {
                let pages: Vec<_> = (self.workspaces[w].tabs.iter())
                    .filter(|tab| tab.review().is_some())
                    .flat_map(Self::closing)
                    .collect();
                self.close_panes(pages);
            }
            let ws = &mut self.workspaces[w];
            changed |= pin_tab(&mut ws.tabs, &mut ws.active_tab, want);
        }
        changed
    }

    /// Link each feature workspace's folder to its stage files, once a run
    /// and off the thread: any agent started there can name them, in a
    /// feature made before there was a link too (see `layout::link_docs`).
    pub(crate) fn link_feature_docs(&mut self) {
        let Some(project) = self.project.clone() else {
            return;
        };
        for w in 0..self.workspaces.len() {
            let Some(feature) = self.workspace_feature(w).filter(|f| f.writer.is_none()) else {
                continue;
            };
            let (slug, title) = (feature.slug.clone(), feature.title.clone());
            let (dir, multi) = (feature.dir.clone(), feature.multi);
            let roots: Vec<PathBuf> = std::iter::once(feature.root.clone())
                .chain(feature.links.iter().map(|l| l.root.clone()))
                .collect();
            for root in roots {
                if self.features.cursor_freed.insert(root.clone()) {
                    self.feature_work(None, move |_| Done::CursorFreed {
                        result: layout::free_for_cursor(&root),
                        root,
                    });
                }
            }
            if !self.features.docs_linked.insert(dir.clone()) {
                continue;
            }
            let data = store::dir(None, &project, &slug);
            self.feature_work(None, move |_| Done::DocsLinked {
                title,
                result: layout::link_docs(&dir, multi, &data),
            });
            // A feature made before there were graphs, or before its
            // project had one.
            self.refresh_feature_graphs(&project, &slug, true);
        }
    }

    /// Put the features' workspaces after all the others, each kind in the
    /// order it had, the active one kept shown: the sidebar's Features
    /// section holds them, under its heading. herdr is told, so the order
    /// holds when gerdr starts again. Whether any moved.
    pub(crate) fn features_last(&mut self) -> bool {
        let n = self.workspaces.len();
        let feature: Vec<bool> = (0..n)
            .map(|w| self.workspace_feature(w).is_some())
            .collect();
        if feature.windows(2).all(|pair| !pair[0] || pair[1]) {
            return false;
        }
        let order: Vec<usize> = ((0..n).filter(|&w| !feature[w]))
            .chain((0..n).filter(|&w| feature[w]))
            .collect();
        let mut old: Vec<Option<Workspace>> = std::mem::take(&mut self.workspaces)
            .into_iter()
            .map(Some)
            .collect();
        self.workspaces = order.iter().filter_map(|&w| old[w].take()).collect();
        if let Some(active) = order.iter().position(|&w| w == self.active_workspace) {
            self.active_workspace = active;
        }
        // Each feature's moved to the end in turn keeps them in order.
        let first = n - feature.iter().filter(|f| **f).count();
        if let Some(herdr) = self.herdr.as_mut() {
            for id in self.workspaces[first..]
                .iter()
                .filter_map(Workspace::herdr_id)
            {
                if let Err(err) = herdr.workspace_move(id, None) {
                    tracing::warn!("herdr: {err}");
                }
            }
        }
        true
    }

    /// Slot `to` held to the slots of workspace `w`'s kind, for a drag of
    /// it: a feature's among the features' workspaces, any other before
    /// them (see `features_last`).
    pub(crate) fn kept_slot(&self, w: usize, to: usize) -> usize {
        let n = self.workspaces.len();
        let first = (0..n)
            .find(|&w| self.workspace_feature(w).is_some())
            .unwrap_or(n);
        if self.workspace_feature(w).is_some() {
            to.max(first)
        } else {
            to.min(first.saturating_sub(1))
        }
    }

    /// Say why a feature's panel tab doesn't close.
    pub(crate) fn refuse_pinned_close(&mut self) {
        self.show_warning("The feature's tab stays open: close its workspace to close it".into());
    }

    /// Add a Terminal workspace on feature folder `dir` called `title`,
    /// last, with only its panel tab and a record of it: the record's id.
    fn open_feature_workspace(&mut self, dir: PathBuf, title: &str) -> String {
        let dir = dir.canonicalize().unwrap_or(dir);
        let kind = WorkspaceKind::Terminal;
        let mut ws = Workspace::new(dir, kind);
        ws.name = title.to_owned();
        ws.record = self.remember_workspace(&ws.name, kind, &ws.dir, None);
        let record = ws.record.clone();
        self.workspaces.push(ws);
        self.pin_feature_tabs();
        self.link_feature_docs();
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

    /// Open a tab running `command` in feature `slug` of `project`'s
    /// workspace, as `open_feature_tab` does, and leave what shows as it is.
    pub(crate) fn open_feature_tab_behind(
        &mut self,
        project: &str,
        slug: &str,
        cd: &Path,
        command: String,
    ) -> Option<String> {
        self.in_feature_workspace(project, slug, |gerdr, w| {
            gerdr.new_tab_pane_behind(w, cd, command)
        })
        .flatten()
    }

    /// Bring feature `slug` of `project`'s stage page to the front of its
    /// workspace, on the stage it is at. Only there does it take the screen:
    /// in another workspace it shows once the user comes back.
    pub(crate) fn show_stage_page(&mut self, project: &str, slug: &str) {
        let Some(w) = self.feature_workspace(project, slug) else {
            return;
        };
        if w != self.active_workspace {
            self.workspaces[w].active_tab = 0;
            return;
        }
        self.switch_tab(0);
        self.feature_cursor_to_current_of(project, slug);
        self.sync_feature_page();
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
    /// The agent writing the summary is gerdr's own and its work is used by
    /// now: it doesn't keep the workspace open.
    pub(crate) fn close_shipped_workspace(&mut self, project: &str, slug: &str) -> Option<String> {
        let w = self.feature_workspace(project, slug)?;
        let summary = (self.feature(project, slug))
            .and_then(|f| f.shipping.summary.as_ref())
            .and_then(|run| run.pane.clone());
        let running = self.workspace_running_but(w, summary.as_deref());
        if running.is_empty() {
            let tabs = self.kept_tabs(w);
            self.close_workspace_now(w);
            self.note_after_ship(project, slug, AfterStep::Close, tabs, Vec::new());
            self.features.just_closed = Some((project.to_owned(), slug.to_owned()));
            return None;
        }
        Some(kept_text(&running))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::tests::{feature, repo};

    fn tabs(n: usize) -> Vec<Tab> {
        (0..n).map(|_| Tab::default()).collect()
    }

    #[test]
    fn a_feature_workspace_gets_its_panel_tab_first_and_keeps_the_shown_tab() {
        let mut t = tabs(2);
        let mut active = 1;
        assert!(pin_tab(&mut t, &mut active, true));
        assert!(t[0].is_feature() && !t[1].is_feature() && !t[2].is_feature());
        assert_eq!(active, 2);
        // Once is enough.
        assert!(!pin_tab(&mut t, &mut active, true));
        assert_eq!(t.len(), 3);
        // An empty workspace gets it shown.
        let (mut empty, mut active) = (Vec::new(), 0);
        assert!(pin_tab(&mut empty, &mut active, true));
        assert_eq!((empty.len(), active), (1, 0));
    }

    #[test]
    fn a_workspace_no_more_a_features_loses_its_panel_tab() {
        let mut t = tabs(2);
        let mut active = 1;
        pin_tab(&mut t, &mut active, true);
        assert!(pin_tab(&mut t, &mut active, false));
        assert!(t.iter().all(|t| !t.is_feature()));
        assert_eq!((t.len(), active), (2, 1));
        // Shown, it gives way to the next.
        let mut t = tabs(1);
        let mut active = 0;
        pin_tab(&mut t, &mut active, true);
        active = 0;
        pin_tab(&mut t, &mut active, false);
        assert_eq!((t.len(), active), (1, 0));
        assert!(!pin_tab(&mut t, &mut active, false));
    }

    #[test]
    fn the_panel_tab_neither_moves_nor_has_tabs_put_before_it() {
        let mut t = tabs(3);
        let mut active = 0;
        pin_tab(&mut t, &mut active, true);
        // The panel tab stays.
        assert_eq!(crate::tab_move_target(&t, 0, 2), None);
        // Another goes no further left than second.
        assert_eq!(crate::tab_move_target(&t, 2, 0), Some(1));
        assert_eq!(crate::tab_move_target(&t, 1, 0), None);
        assert_eq!(crate::tab_move_target(&t, 1, 3), Some(3));
        assert_eq!(crate::tab_move_target(&t, 1, 4), None);
        // Without one, any tab goes anywhere.
        let t = tabs(3);
        assert_eq!(crate::tab_move_target(&t, 2, 0), Some(0));
    }

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

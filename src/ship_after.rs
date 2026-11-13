//! Undoing what followed shipping a feature: its workspace closing comes
//! back with its tabs (Zed and apps; terminals and what ran in them are
//! gone), its worktrees removed come back on their commits, set up again,
//! with its workspace. Offered by the toast saying it happened, Ship's page
//! (Alt+Z), and, for worktrees, the launcher. An abandoned feature comes back
//! the same way, open again, with what it had uncommitted (`layout::keep`).

use crate::{
    Gerdr, Tab,
    feature::{self, AfterDone, AfterStep, layout},
    features::{Busy, Done, Job},
    menu::MenuAction,
    projects::TabRecord,
};

/// Why `step`, as it last followed shipping `feature`, can't be undone; none
/// when it can.
pub fn after_refusal(feature: &feature::Feature, step: AfterStep) -> Option<String> {
    let title = &feature.title;
    if feature.after_done(step).is_none() {
        return Some(match step {
            AfterStep::Close => {
                format!("\u{201c}{title}\u{201d} has no closed workspace to bring back")
            }
            AfterStep::CloseTabs => {
                format!("\u{201c}{title}\u{201d} has no closed tabs to bring back")
            }
            AfterStep::Remove => {
                format!("\u{201c}{title}\u{201d} has no removed worktrees to bring back")
            }
        });
    }
    match step {
        AfterStep::Close | AfterStep::CloseTabs
            if feature.after_done(AfterStep::Remove).is_some() =>
        {
            Some(format!(
                "The worktrees of \u{201c}{title}\u{201d} are gone: restore them first"
            ))
        }
        AfterStep::Remove if feature.writer.is_some() => Some(format!(
            "\u{201c}{title}\u{201d} is written on another machine: restore it there"
        )),
        _ => None,
    }
}

/// The button a toast saying `step` happened has, to undo it.
fn toast_button(project: &str, slug: &str, step: AfterStep) -> (String, MenuAction) {
    let label = match step {
        AfterStep::Close | AfterStep::CloseTabs => "Reopen",
        AfterStep::Remove => "Restore",
    };
    (
        label.to_owned(),
        MenuAction::UndoAfterShip(project.to_owned(), slug.to_owned(), step),
    )
}

impl Gerdr {
    /// The tabs of workspace `w` as gerdr saves them, as JSON: what undoing
    /// its closing opens again.
    pub(crate) fn kept_tabs(&self, w: usize) -> Vec<serde_json::Value> {
        self.kept_tabs_where(w, |_| true)
    }

    /// `kept_tabs`, but only those that come back once closed: a file's, or
    /// one with an app in it. A closed terminal doesn't.
    pub(crate) fn kept_app_tabs(&self, w: usize) -> Vec<serde_json::Value> {
        self.kept_tabs_where(w, |tab| tab.doc.is_some() || tab.layout.holds_apps())
    }

    fn kept_tabs_where(
        &self,
        w: usize,
        keep: impl Fn(&TabRecord) -> bool,
    ) -> Vec<serde_json::Value> {
        let Some(ws) = self.workspaces.get(w) else {
            return Vec::new();
        };
        (ws.tabs.iter())
            .filter_map(Tab::save)
            .filter(|tab| keep(tab))
            .filter_map(|tab| {
                serde_json::to_value(tab)
                    .inspect_err(|err| tracing::warn!("keeping a closed tab: {err}"))
                    .ok()
            })
            .collect()
    }

    /// Note that `step` followed shipping feature `slug` now, `tabs` its
    /// workspace's and `heads` its worktrees'.
    pub(crate) fn note_after_ship(
        &mut self,
        project: &str,
        slug: &str,
        step: AfterStep,
        tabs: Vec<serde_json::Value>,
        heads: Vec<feature::KeptHead>,
    ) {
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        feature.after_ran(AfterDone {
            at: feature::now(),
            step,
            tabs,
            heads,
            undone: None,
        });
        self.save_feature(project, slug);
    }

    /// Say `text`, a success of shipping feature `slug`: with a Reopen
    /// button when it closed its workspace, or its tabs, just now.
    pub(crate) fn show_shipped(&mut self, project: &str, slug: &str, text: String) {
        let closed =
            (self.features.just_closed.take()).filter(|(p, s, _)| p == project && s == slug);
        let button = closed.map(|(_, _, step)| toast_button(project, slug, step));
        self.toast_with(text, crate::toast::Level::Success, button);
    }

    /// Say that feature `slug`'s worktrees went, with a Restore button when
    /// they can come back.
    pub(crate) fn show_removed(
        &mut self,
        project: &str,
        slug: &str,
        text: String,
        abandoned: bool,
    ) {
        let can = (self.feature(project, slug))
            .is_some_and(|f| after_refusal(f, AfterStep::Remove).is_none());
        let button = can.then(|| toast_button(project, slug, AfterStep::Remove));
        // Not a success: no done green on what was given up.
        let level = match abandoned {
            true => crate::toast::Level::Info,
            false => crate::toast::Level::Success,
        };
        self.toast_with(text, level, button);
    }

    /// Undo `step`, as it last followed shipping feature `slug`; or say why
    /// it can't be.
    pub(crate) fn undo_after_ship(&mut self, project: &str, slug: &str, step: AfterStep) {
        let Some(feature) = self.feature(project, slug) else {
            return self.show_warning("That feature is gone".into());
        };
        if let Some(why) = after_refusal(feature, step) {
            return self.show_warning(why);
        }
        if self.feature_busy(project, slug) {
            let title = feature.title.clone();
            return self.show_warning(format!(
                "\u{201c}{title}\u{201d} is busy; try again once it is done"
            ));
        }
        match step {
            AfterStep::Close | AfterStep::CloseTabs => {
                self.reopen_shipped_workspace(project, slug, step);
            }
            AfterStep::Remove => self.restore_worktrees(project, slug),
        }
    }

    /// Open feature `slug`'s workspace again on its Ship page, with the tabs
    /// it had when shipping closed it, or them (`step`).
    fn reopen_shipped_workspace(&mut self, project: &str, slug: &str, step: AfterStep) {
        let Some(done) = (self.feature(project, slug))
            .and_then(|f| f.after_done(step))
            .cloned()
        else {
            return;
        };
        if !self.reopen_with_tabs(project, slug, &done) {
            return;
        }
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let now = feature::now();
        feature.after_undid(step, now);
        let tabs = step == AfterStep::CloseTabs;
        let said = match tabs {
            true => "tabs reopened",
            false => "workspace reopened",
        };
        feature.log(now, said);
        let title = feature.title.clone();
        self.save_feature(project, slug);
        self.show_success(match tabs {
            true => format!("Reopened the tabs of \u{201c}{title}\u{201d}"),
            false => format!("Reopened the workspace of \u{201c}{title}\u{201d}"),
        });
    }

    /// Show feature `slug`'s workspace on its page, opening it if it isn't,
    /// with the tabs `done` kept brought back into it. Whether it shows.
    fn reopen_with_tabs(&mut self, project: &str, slug: &str, done: &AfterDone) -> bool {
        let Some((w, _)) = self.ensure_feature_workspace(project, slug) else {
            return false;
        };
        let tabs: Vec<TabRecord> = (done.tabs.iter())
            .filter_map(|tab| {
                serde_json::from_value(tab.clone())
                    .inspect_err(|err| tracing::warn!("reading a kept tab: {err}"))
                    .ok()
            })
            .collect();
        let zed = self.restore_app_tabs(w, &tabs, true).1;
        if zed {
            self.launch_next_zed();
        }
        self.prune();
        self.arrange();
        self.open_feature(project, slug);
        true
    }

    /// Bring feature `slug`'s worktrees back off the thread, as they were
    /// when they went.
    fn restore_worktrees(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let Some(heads) = (feature.after_done(AfterStep::Remove)).map(|d| d.heads.clone()) else {
            return;
        };
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Remove,
            repo: None,
            what: format!("Restoring \u{201c}{}\u{201d}", feature.title),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move |_| Done::WorktreesRestored {
            result: layout::restore(&feature, &heads),
            project,
            slug,
        });
    }

    /// Restoring feature `slug`'s worktrees came to `result`: back, they are
    /// set up again (what isn't in git is copied, its commands run), and its
    /// workspace opens with the tabs it had.
    pub(crate) fn worktrees_restored(
        &mut self,
        project: &str,
        slug: &str,
        result: Result<(), String>,
    ) {
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let title = feature.title.clone();
        if let Err(why) = result {
            tracing::warn!("restoring the worktrees of feature {project}/{slug}: {why}");
            return self.show_error(format!(
                "Couldn't restore the worktrees of \u{201c}{title}\u{201d}: {why}"
            ));
        }
        let Some(done) = feature.after_done(AfterStep::Remove).cloned() else {
            return;
        };
        let now = feature::now();
        feature.after_undid(AfterStep::Remove, now);
        // Abandoned, it is open again, as it was.
        let abandoned = feature.status == feature::Status::Abandoned;
        if abandoned {
            feature.status = feature::Status::Open;
        }
        feature.log(
            now,
            if abandoned {
                "restored"
            } else {
                "worktrees restored"
            },
        );
        let dirs: Vec<std::path::PathBuf> = (feature.folders().into_iter())
            .map(|(_, dir)| dir.to_path_buf())
            .collect();
        self.save_feature(project, slug);
        // Its folders link to its stage files again once its workspace opens.
        for dir in &dirs {
            self.features.docs_linked.remove(dir);
        }
        let setting_up = self.start_setup(project, slug);
        self.reopen_with_tabs(project, slug, &done);
        self.probe_feature(project, slug);
        let what = match abandoned {
            true => format!("\u{201c}{title}\u{201d}"),
            false => format!("the worktrees of \u{201c}{title}\u{201d}"),
        };
        self.show_success(match setting_up {
            true => format!("Restored {what}; setting it up"),
            false => format!("Restored {what}"),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::tests::{feature, repo};

    fn done(step: AfterStep) -> AfterDone {
        AfterDone {
            at: 1,
            step,
            tabs: Vec::new(),
            heads: Vec::new(),
            undone: None,
        }
    }

    #[test]
    fn closed_tabs_come_back_until_the_worktrees_went() {
        let mut f = feature("s", vec![repo("api", true)], 1);
        assert!(after_refusal(&f, AfterStep::CloseTabs).is_some());
        f.after_ran(done(AfterStep::CloseTabs));
        assert_eq!(after_refusal(&f, AfterStep::CloseTabs), None);
        // Not a closed workspace: that one has nothing to bring back.
        assert!(after_refusal(&f, AfterStep::Close).is_some());
        f.after_ran(done(AfterStep::Remove));
        let why = after_refusal(&f, AfterStep::CloseTabs).unwrap_or_default();
        assert!(why.ends_with("restore them first"), "{why}");
        let said = serde_json::to_string(&AfterStep::CloseTabs).unwrap_or_default();
        assert_eq!(said, "\"close-tabs\"");
    }
}

//! Every popup, panel and dialog gerdr shows over the apps, and the one
//! order they stack in. Input goes to what is on top, and the chrome
//! paints in the same order, so what takes a click is always what shows.
//!
//! From the top: the tooltip, the modal dialogs (quit, restart, checkout,
//! new feature, the manual test sheet, the plan review's sheet, confirm), the context menu, the toasts, the welcome screen,
//! the help panel, then the panels (the tasks, the project menu, the
//! worktrees popup, the repositories and inbox panels, the scrollback
//! viewer, the switcher), and last the feature panel and the file's tab,
//! which are not over the apps but in a tab of their own.
//! The toasts are over every popup but the menu and the dialogs, so what
//! a panel reports shows while it is open; the help panel is under them,
//! over the panels it may be pinned over.
//!
//! The dialogs, the welcome screen and the panels grab: while one is
//! open, every key, press and scroll goes to it, wherever the pointer
//! is. The context menu takes every key and press too (a press outside
//! closes it). The toasts and the help panel take only what lands on
//! them; the tooltip takes nothing. The feature panel takes the keys
//! while its tab shows, and what lands in the tab, like an app would; so
//! does the file's tab, leaving its page what its page shows on (see
//! `doc_tab`).

use std::time::Instant;

use smithay::utils::{Logical, Point};

use crate::{Action, Gerdr, chrome, scroll::Motion, tooltip::Tooltip};

/// A popup, panel or dialog over the apps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    Tooltip,
    Quit,
    Restart,
    Checkout,
    /// The new feature dialog (see `features`).
    NewFeature,
    /// The dialog asking for a finding's text (see `finding_dialog`).
    FindingDialog,
    /// The name and email git wants to commit (see `git_identity`).
    GitIdentity,
    /// The manual test sheet: a feature's acceptance criteria to try
    /// (see `manual_test`).
    ManualTest,
    /// The plan review's sheet: a feature's suggestions to decide (see
    /// `plan_sheet`).
    PlanSheet,
    Confirm,
    ContextMenu,
    Toasts,
    Welcome,
    /// The key help: up while the prefix is armed, or pinned open.
    Help,
    /// The background tasks, over the footer (see `tasks`).
    Tasks,
    /// The feature panel: a feature's stages, in the pinned first tab of
    /// its workspace (see `features`, `feature_workspace`).
    Features,
    /// A feature's file in a tab of its own, its preview and review flat
    /// (see `doc_tab`).
    Doc,
    ProjectMenu,
    Worktrees,
    Repos,
    Inbox,
    Scrollback,
    Switcher,
}

impl Overlay {
    /// Every overlay, top first.
    pub const ORDER: [Self; 22] = [
        Self::Tooltip,
        Self::Quit,
        Self::Restart,
        Self::Checkout,
        Self::NewFeature,
        Self::FindingDialog,
        Self::GitIdentity,
        Self::Confirm,
        Self::ContextMenu,
        Self::Toasts,
        // A sheet worked in for a while: the toasts of what it does (and
        // of all else going on) show over it.
        Self::ManualTest,
        Self::PlanSheet,
        Self::Welcome,
        Self::Help,
        Self::Tasks,
        Self::ProjectMenu,
        Self::Worktrees,
        Self::Repos,
        Self::Inbox,
        Self::Scrollback,
        Self::Switcher,
        // In their tabs, under every popup.
        Self::Features,
        Self::Doc,
    ];

    /// Whether it is a modal dialog: over all but the tooltip, asking
    /// before something goes on.
    pub fn is_dialog(self) -> bool {
        matches!(
            self,
            Self::Quit
                | Self::Restart
                | Self::Checkout
                | Self::NewFeature
                | Self::FindingDialog
                | Self::GitIdentity
                | Self::ManualTest
                | Self::PlanSheet
                | Self::Confirm
        )
    }

    /// Whether it is one of the panels with a list to pick from.
    pub fn is_panel(self) -> bool {
        matches!(
            self,
            Self::ProjectMenu
                | Self::Worktrees
                | Self::Repos
                | Self::Inbox
                | Self::Scrollback
                | Self::Switcher
                | Self::Tasks
        )
    }

    /// Whether, while it is open, every key, press and scroll goes to it,
    /// wherever the pointer is, and no client has the pointer.
    pub fn grabs(self) -> bool {
        self.is_dialog() || self.is_panel() || self == Self::Welcome
    }

    /// Whether, while it is open, the keys go to it rather than the apps
    /// (or what is under it).
    pub fn takes_keys(self) -> bool {
        self.grabs() || matches!(self, Self::ContextMenu | Self::Features | Self::Doc)
    }

    /// Whether it is over `other`.
    pub fn is_above(self, other: Self) -> bool {
        let rank = |o: Self| Self::ORDER.iter().position(|x| *x == o);
        rank(self) < rank(other)
    }

    /// The action whose key opens it, after the prefix: the same key over
    /// it closes it again.
    pub fn toggled_by(self) -> Option<Action> {
        Some(match self {
            Self::ProjectMenu => Action::Projects,
            Self::Worktrees => Action::Worktrees,
            Self::Repos => Action::Repos,
            Self::Inbox => Action::Inbox,
            Self::Scrollback => Action::Scrollback,
            Self::Switcher => Action::Switcher,
            Self::Help => Action::Shortcuts,
            Self::Tasks => Action::Tasks,
            _ => return None,
        })
    }

    /// What a message calls it: "the repositories panel".
    pub fn name(self) -> &'static str {
        match self {
            Self::Tooltip => "the tooltip",
            Self::Quit => "the quit dialog",
            Self::Restart => "the restart dialog",
            Self::Checkout => "the checkout dialog",
            Self::NewFeature => "the new feature dialog",
            Self::FindingDialog => "the finding dialog",
            Self::GitIdentity => "the name and email dialog",
            Self::ManualTest => "the manual test sheet",
            Self::PlanSheet => "the plan review's sheet",
            Self::Confirm => "the dialog",
            Self::ContextMenu => "the menu",
            Self::Toasts => "the notices",
            Self::Welcome => "the welcome screen",
            Self::Help => "the help",
            Self::Tasks => "the background tasks",
            Self::Features => "the feature panel",
            Self::Doc => "the file's tab",
            Self::ProjectMenu => "the project menu",
            Self::Worktrees => "the worktrees popup",
            Self::Repos => "the repositories panel",
            Self::Inbox => "the inbox",
            Self::Scrollback => "the scrollback viewer",
            Self::Switcher => "the switcher",
        }
    }
}

impl Gerdr {
    /// Whether `overlay` shows: a tooltip about to show counts, a toast
    /// going out does not.
    pub fn is_open(&self, overlay: Overlay) -> bool {
        match overlay {
            Overlay::Tooltip => self.tooltip.is_up(),
            Overlay::Quit => self.quit.is_some(),
            Overlay::Restart => self.restart.is_some(),
            Overlay::Checkout => self.checkout.is_some(),
            Overlay::NewFeature => self.features.dialog.is_some(),
            Overlay::FindingDialog => self.features.finding_dialog.is_some(),
            Overlay::GitIdentity => self.repos.identity.is_some(),
            Overlay::ManualTest => self.features.manual.is_some(),
            Overlay::PlanSheet => self.features.plan_sheet.is_some(),
            Overlay::Confirm => self.confirm.is_some(),
            Overlay::ContextMenu => self.context_menu.is_some(),
            Overlay::Toasts => {
                let now = Instant::now();
                self.toasts.iter().any(|toast| now < toast.until)
            }
            Overlay::Welcome => self.welcome.is_some(),
            Overlay::Help => self.prefix_armed || self.help_pinned,
            Overlay::Tasks => self.tasks_panel.is_some(),
            Overlay::Features => self.features.panel.is_some(),
            Overlay::Doc => self.review_tab().is_some(),
            Overlay::ProjectMenu => self.project_menu.is_some(),
            Overlay::Worktrees => self.worktrees.is_open(),
            Overlay::Repos => self.repos.is_open(),
            Overlay::Inbox => self.inbox.is_open(),
            Overlay::Scrollback => self.scrollback.is_some(),
            Overlay::Switcher => self.switcher.is_some(),
        }
    }

    /// The overlays open, top first.
    pub fn open_overlays(&self) -> impl Iterator<Item = Overlay> + '_ {
        Overlay::ORDER.into_iter().filter(|o| self.is_open(*o))
    }

    /// The overlay on top that grabs every key, press and scroll, if one
    /// is open.
    pub fn grabbing_overlay(&self) -> Option<Overlay> {
        self.open_overlays().find(|o| o.grabs())
    }

    /// The overlay on top that has the keys, if one does.
    pub fn key_overlay(&self) -> Option<Overlay> {
        self.open_overlays()
            .find(|o| o.takes_keys() && (*o != Overlay::Doc || self.doc_tab_takes_keys()))
    }

    /// Whether open `overlay` takes the pointer at `pos`: one that grabs,
    /// and the menu (a press outside it closes it), anywhere; the toasts
    /// and the help panel where they are; the feature panel in its tab;
    /// the file's tab there but over its page; the tooltip nowhere.
    pub fn overlay_covers(&self, overlay: Overlay, pos: Point<f64, Logical>) -> bool {
        match overlay {
            Overlay::Tooltip => false,
            Overlay::Toasts => self.toast_at(pos),
            Overlay::Help => self.help_hit(pos).is_some(),
            Overlay::ContextMenu => true,
            Overlay::Features => self.content_rect().to_f64().contains(pos),
            Overlay::Doc => self.doc_tab_covers(pos),
            _ => overlay.grabs(),
        }
    }

    /// The overlay on top at `pos`: what a press there goes to, and what
    /// takes the hover from what is under it. `None` where none is, and
    /// what is under is gerdr's chrome or a client.
    pub fn overlay_at(&self, pos: Point<f64, Logical>) -> Option<Overlay> {
        self.open_overlays().find(|o| self.overlay_covers(*o, pos))
    }

    /// Whether a toast staying is at `pos`.
    pub fn toast_at(&self, pos: Point<f64, Logical>) -> bool {
        let views = self.toast_views();
        let layouts = chrome::toast_layout(&views, self.toast_rect());
        layouts
            .iter()
            .zip(&views)
            .any(|(layout, view)| !view.leaving && layout.panel.to_f64().contains(pos))
    }

    /// The pointer moved to `pos` over `overlay`: what is under it there
    /// lights up, and the tooltip it offers, if any.
    pub fn overlay_hover(&mut self, overlay: Overlay, pos: Point<f64, Logical>) -> Option<Tooltip> {
        match overlay {
            Overlay::Quit => self.quit_hover(pos),
            Overlay::Restart => self.restart_hover(pos),
            Overlay::Checkout => self.checkout_hover(pos),
            Overlay::Confirm => self.confirm_hover(pos),
            Overlay::Tasks => self.tasks_hover(pos),
            Overlay::NewFeature => self.new_feature_hover(pos),
            Overlay::FindingDialog => self.finding_dialog_hover(pos),
            Overlay::GitIdentity => self.identity_hover(pos),
            Overlay::ManualTest => return self.manual_test_hover(pos),
            Overlay::PlanSheet => return self.plan_sheet_hover(pos),
            Overlay::Features => return self.feature_panel_hover(pos),
            Overlay::Doc => return self.repos_hover(pos),
            Overlay::ContextMenu => self.menu_motion(pos),
            Overlay::Welcome => return self.welcome_hover(pos),
            Overlay::ProjectMenu => return self.project_menu_hover(pos),
            Overlay::Worktrees => return self.worktrees_hover(pos),
            Overlay::Repos => return self.repos_hover(pos),
            Overlay::Inbox => return self.inbox_hover(pos),
            Overlay::Scrollback => return self.scrollback_hover(pos),
            Overlay::Switcher => return self.switcher_motion(pos),
            Overlay::Tooltip | Overlay::Toasts | Overlay::Help => {}
        }
        None
    }

    /// The wheel or the touchpad moved over `overlay`: its list moves,
    /// if it has one. The restart dialog's commands and the welcome
    /// screen's projects move by whole rows.
    pub fn overlay_scroll(&mut self, overlay: Overlay, motion: Motion) {
        match overlay {
            Overlay::Restart => {
                let rows = self.restart.as_ref().map_or(0, |d| d.rows.len());
                let layout = chrome::restart_layout(self.output_size(), rows);
                let rows = self.motion_rows(motion, row_pitch(&layout.rows));
                if rows != 0 {
                    self.restart_scroll(rows);
                }
            }
            Overlay::Welcome => {
                let layout = chrome::welcome_layout(self.output_size(), 2, false, false);
                let rows = self.motion_rows(motion, row_pitch(&layout.rows));
                if rows != 0 {
                    self.welcome_scroll(rows);
                }
            }
            Overlay::ProjectMenu => self.project_menu_scroll(motion),
            Overlay::Worktrees => self.worktrees_scroll(motion),
            Overlay::Repos => self.repos_scroll(motion),
            Overlay::Inbox => self.inbox_scroll(motion),
            Overlay::Scrollback => self.scrollback_scroll(motion),
            Overlay::Switcher => self.switcher_scroll(motion),
            Overlay::NewFeature => {
                let rows = self.motion_rows(motion, row_pitch(&self.new_feature_rows()));
                if rows != 0 {
                    self.new_feature_scroll(rows);
                }
            }
            Overlay::ManualTest => {
                let rows = self.motion_rows(motion, row_pitch(&self.manual_test_rows()));
                if rows != 0 {
                    self.manual_test_scroll(rows);
                }
            }
            Overlay::PlanSheet => self.plan_sheet_scroll(motion),
            Overlay::Features => self.feature_panel_scroll(motion),
            Overlay::Doc => self.repos_scroll(motion),
            Overlay::Tasks => {
                let rows = self.motion_rows(motion, row_pitch(&self.tasks_rows()));
                if rows != 0 {
                    self.tasks_scroll(rows as i32);
                }
            }
            _ => {}
        }
    }
}

/// How far apart `rows`, laid out one under the other, are: from the top
/// of one to the next, or the height of the one there is.
fn row_pitch(rows: &[chrome::Rect]) -> f64 {
    match rows {
        [first, second, ..] => (second.loc.y - first.loc.y) as f64,
        [only] => only.size.h as f64,
        [] => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_as_far_apart_as_they_are_laid_out() {
        let row = |y| chrome::Rect::new((0, y).into(), (100, 28).into());
        assert_eq!(row_pitch(&[row(10), row(42), row(74)]), 32.0);
        assert_eq!(row_pitch(&[row(10)]), 28.0);
        assert_eq!(row_pitch(&[]), 1.0);
    }

    #[test]
    fn every_overlay_stacks_once() {
        for (i, o) in Overlay::ORDER.iter().enumerate() {
            assert!(!Overlay::ORDER[i + 1..].contains(o), "{o:?} twice");
        }
    }

    #[test]
    fn dialogs_are_over_the_toasts_and_the_toasts_over_the_panels() {
        assert!(Overlay::Quit.is_above(Overlay::Toasts));
        assert!(Overlay::Confirm.is_above(Overlay::ContextMenu));
        assert!(Overlay::ContextMenu.is_above(Overlay::Toasts));
        assert!(Overlay::Toasts.is_above(Overlay::Help));
        assert!(Overlay::Toasts.is_above(Overlay::ManualTest));
        assert!(Overlay::ManualTest.is_above(Overlay::Welcome));
        assert!(Overlay::Toasts.is_above(Overlay::PlanSheet));
        assert!(Overlay::PlanSheet.is_above(Overlay::Welcome));
        for panel in Overlay::ORDER.into_iter().filter(|o| o.is_panel()) {
            assert!(Overlay::Toasts.is_above(panel));
            assert!(Overlay::Help.is_above(panel));
            assert!(Overlay::Welcome.is_above(panel));
        }
    }

    #[test]
    fn what_grabs_takes_the_keys_and_the_menu_does_too() {
        for o in Overlay::ORDER {
            assert_eq!(
                o.grabs(),
                o.is_dialog() || o.is_panel() || o == Overlay::Welcome
            );
            if o.grabs() {
                assert!(o.takes_keys());
            }
        }
        assert!(Overlay::ContextMenu.takes_keys());
        assert!(!Overlay::Help.takes_keys());
        assert!(!Overlay::Toasts.takes_keys());
        assert!(!Overlay::Tooltip.takes_keys());
    }

    #[test]
    fn each_panel_closes_on_its_own_key() {
        for panel in Overlay::ORDER.into_iter().filter(|o| o.is_panel()) {
            assert!(panel.toggled_by().is_some(), "{panel:?}");
        }
        assert_eq!(Overlay::Repos.toggled_by(), Some(Action::Repos));
        assert_eq!(Overlay::Quit.toggled_by(), None);
    }
}

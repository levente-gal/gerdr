//! Context menus: what a region of the chrome offers on a right click (or
//! under a button's chevron), one component for all of them. A region
//! builds a `MenuSpec`; `open_menu` shows it; a pick runs its
//! `MenuAction` through `menu_action`, which maps onto what the keys do.
//! The arrow keys (and Home, End) walk the menu, Enter picks, Esc closes;
//! any other key only closes it. A menu taller than its output shows the
//! items that fit.

use std::path::PathBuf;

use smithay::{
    input::keyboard::Keysym,
    utils::{Logical, Point, Size},
};

use crate::{
    Action, Gerdr, WorkspaceKind,
    chrome::{self, MenuRow, MenuView, Rect, ReposButton, ThemeMode},
    inbox_store,
    layout::{Dir, Side},
    rename::RenameTarget,
};

/// What a menu item does when picked.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuAction {
    NewTerminal,
    /// Give project `.0` the id `.1` (see `Gerdr::change_project_id`): a
    /// change's Undo.
    ChangeProjectId(String, String),
    /// An action of the repositories panel, as its footer offers it.
    ReposAction(ReposButton),
    /// The dialog raising the inotify limit (see `inotify_limit`).
    RaiseInotifyLimit,
    /// The name and email git wants, asked for the panel's batch that
    /// failed for want of them (see `Gerdr::open_identity`).
    SetGitIdentity,
    /// The project's Cursor workspace (see `Gerdr::open_cursor`).
    NewCursor,
    /// The agent picker, for an agent CLI in a new tab of workspace `.0`
    /// (see `Gerdr::open_agent_picker`).
    NewAgent(usize),
    /// The fixes of AI review of a project's feature, planned (see
    /// `Gerdr::send_findings`).
    PlanFixes(String, String),
    /// The same with no fix plan (see `Gerdr::make_finding_fixes`).
    MakeFindingFixes(String, String),
    /// T3 Code's server, started in the background (see
    /// `Gerdr::spawn_t3`).
    StartT3,
    /// T3 Code's server, stopped (see `Gerdr::stop_t3`).
    StopT3,
    /// T3 Code's server, stopped and started again.
    RestartT3,
    /// T3 Code's app, in the shown workspace.
    OpenT3,
    /// Look for a newer T3 Code now (see `Gerdr::check_t3`).
    CheckT3,
    /// A log file, in a new tab.
    ShowLog(std::path::PathBuf),
    /// The apps project `.0` keeps running, stopped (see `keep::stop`).
    StopKept(String),
    /// The dialog connecting another machine (see `machines`).
    AddMachine,
    /// The connect dialog, filled in for a gerdr that reached this machine.
    AddMachineFrom { target: String, name: String },
    /// The machines panel, on the machine of this name.
    OpenMachines(String),
    /// Connect, disconnect or forget the machine of this name.
    ConnectMachine(String),
    DisconnectMachine(String),
    /// Stop taking this feature over from another machine: project, slug.
    StopTakeOver(String, String),
    /// Stop keeping earlier AI work, or drawing its stats again.
    StopAiWork,
    /// Stop bringing the AI work of a feature (project, slug) from the
    /// machine it was taken over from.
    StopAiWorkPull(String, String),
    RemoveMachine(String),
    /// T3 Code, updated to the newest version gerdr reads, or to this one
    /// (see `Gerdr::update_t3`).
    UpdateT3(Option<String>),
    /// A GitHub page in a tab of workspace `.0` (see `Gerdr::open_page`).
    OpenPage(usize, String),
    /// The same in a new tab, even when a tab shows it already.
    OpenPageInNewTab(usize, String),
    /// Stop watching the CI of a pull request, by repository key and
    /// number (see `ci_watch`).
    Unwatch(String, u64),
    OpenFolder,
    NewProject,
    CloseWorkspace(usize),
    RenameWorkspace(usize),
    /// Restart workspace `w`'s commands, agents aside (see
    /// `Gerdr::workspace_restarts`).
    RestartWorkspace(usize),
    /// Choose which of workspace `w`'s commands restarting it runs again.
    ChooseRestarts(usize),
    /// A new tab in workspace `w`.
    NewTab(usize),
    /// Tab `t` of workspace `w`.
    CloseTab(usize, usize),
    RenameTab(usize, usize),
    /// Split pane `id` of the active tab.
    SplitPane(u32, Dir),
    ClosePane(u32),
    /// Move pane `id` of the active tab to `side`: beside its neighbour
    /// there, or to the tab's edge.
    MovePane(u32, Side),
    /// Swap pane `id` of the active tab with its neighbour on `side`.
    SwapPane(u32, Side),
    /// Turn the split pane `id` of the active tab is in.
    FlipPane(u32),
    /// Zoom pane `id` of the active tab to fill it, or unzoom it.
    ZoomPane(u32),
    /// Move pane `id` of the active tab to tab `t` of the workspace, or a
    /// new tab (`None`).
    PaneToTab(u32, Option<usize>),
    /// Open the scrollback viewer over terminal pane `id`.
    Scrollback(u32),
    RestartPane(u32),
    StopPane(u32),
    SendPrompt(u32),
    /// Take the web page in pane `id` of the active tab back, forward, or
    /// load it again.
    WebStep(u32, crate::web_control::WebStep),
    /// Copy the address of the web page in pane `id` of the active tab.
    CopyPaneUrl(u32),
    /// Show the agent.
    FocusAgent(crate::agents::AgentKey),
    /// Close the agent, asking first if it runs (see `Gerdr::close_agent`).
    CloseAgent(crate::agents::AgentKey),
    CopyPath(PathBuf),
    Theme(ThemeMode),
    /// Start again what pane `id` of the active tab waits for, after it
    /// failed to show a window (or its window went away).
    RetryExpected(u32),
    /// Ask again to unlock the keyring, which apps wait for (see `keyring`).
    UnlockKeyring,
    /// Give up on what pane `id` of the active tab waits for, and close
    /// the pane.
    CloseExpected(u32),
    /// Open the run prompt for an app in `place`.
    RunApp(crate::launch::Place),
    /// Open the worktrees popup, the repository of the workspace in this
    /// folder first.
    Worktrees(PathBuf),
    /// Delete the worktree in this folder, squashed in: it loses nothing.
    DeleteWorktree(PathBuf),
    /// Open the repositories panel.
    Repos,
    /// Open the inbox.
    Inbox,
    /// Open the feature panel (the new feature dialog without features).
    Features,
    /// The new feature dialog, from the folder of workspace `.0` (else the
    /// active one's; see `Gerdr::open_new_feature`).
    NewFeature(Option<usize>),
    /// The feature panel on feature `.1` of project `.0`.
    OpenFeature(String, String),
    /// A toast's "Undo" after inbox items were dismissed or snoozed: list
    /// the items (by `inbox::ItemId`) of project `project` (by id) again,
    /// and mark the service errors `unsee` as not looked at again.
    InboxRestore {
        project: String,
        ids: Vec<String>,
        unsee: Vec<crate::services::ServiceErrorKey>,
    },
    /// A toast's "Undo" after an inbox todo was deleted: put `todo` back
    /// in project `project`'s file (by id), at index `at` of its todos,
    /// with how it was triaged.
    InboxUndelete {
        project: String,
        todo: inbox_store::Todo,
        at: usize,
        triage: Option<inbox_store::Triage>,
    },
    /// A toast's "Undo" after a service's error was ignored, or ignored
    /// no more: put it back as it was.
    ServiceErrorsRestore(Box<crate::services::ErrorsSnapshot>),
    /// A toast's button undoing what followed shipping feature `.1` of
    /// project `.0`: its workspace closing, its worktrees going.
    UndoAfterShip(String, String, crate::feature::AfterStep),
    /// Undo dropping a feature's context repository.
    RestoreContextRepo {
        project: String,
        slug: String,
        repo: Box<crate::feature::ContextRepo>,
    },
    /// Undo deleting the user's comment on a review item: an AI review
    /// finding, or a Plan review suggestion (`of`).
    RestoreItemComment {
        project: String,
        slug: String,
        of: crate::feature::review_items::ItemOf,
        id: String,
        note: String,
    },
    /// Write feature `slug`'s final plan from its decided suggestions, or go
    /// on with the plan as it is (see `Gerdr::write_final_plan`).
    WriteFinalPlan(String, String),
    /// Undo deleting an item of the user's own from an AI review: put it back
    /// at index `at`.
    RestoreOwnFinding {
        project: String,
        slug: String,
        at: usize,
        finding: Box<crate::feature::review_import::Finding>,
    },
    /// Undo deleting a comment of the user's own from a feature's manual test.
    RestoreManualComment {
        project: String,
        slug: String,
        removed: Box<crate::feature::manual::Removed>,
    },
    /// Undo deleting a comment the user left in a feature's diff: put it back
    /// at index `at` of their comments.
    RestoreDiffNote {
        project: String,
        slug: String,
        at: usize,
        comment: Box<crate::review::Comment>,
    },
    /// Undo removing one of the user's own suggestions of a plan review:
    /// put it back at index `at`.
    RestoreSuggestion {
        project: String,
        slug: String,
        at: usize,
        suggestion: Box<crate::feature::plan_review::Suggestion>,
    },
    /// Undo a save of the project commands dialog.
    RestoreProjectCommands(crate::project_commands::CommandsUndo),
    /// Open the project commands dialog, for feature `slug` if one is given.
    EditCommands {
        project: String,
        slug: Option<String>,
        tab: crate::chrome::CommandsTab,
    },
    /// Ask before removing what sandboxes of gone features left.
    AskRemoveSandboxLeftovers(Vec<crate::feature_sandbox::LeftStack>),
    /// Read the project files again, after one was edited by hand.
    ReloadProjects,
    /// Join project `id`, open in another gerdr, here (see
    /// `Gerdr::attach_project`).
    AttachProject(String),
    /// Open the settings file, in a tab.
    OpenSettings,
    /// Open gerdr's log file, in a tab.
    OpenLog,
    /// Show the setup log of feature `.1` of project `.0`.
    SetupLog(String, String),
    /// Show tab `t` of workspace `w`.
    SwitchTab(usize, usize),
    /// Go back (`true`) or forward to tab `id`, as the back or forward
    /// button's menu lists it (see `nav_history`).
    GoToVisited(crate::nav_history::Visit, bool),
    /// Fold every workspace in the sidebar (`Some(true)`), unfold every
    /// one (`Some(false)`), or leave each to fold while quiet (`None`).
    FoldAll(Option<bool>),
}

/// One row of a menu.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuItem {
    pub label: String,
    /// The key that does the same, shown at the right.
    pub hint: Option<String>,
    pub action: MenuAction,
    pub enabled: bool,
}

impl MenuItem {
    pub fn new(label: &str, action: MenuAction) -> Self {
        Self {
            label: label.into(),
            hint: None,
            action,
            enabled: true,
        }
    }

    pub fn hint(mut self, hint: &str) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// A menu to open: below `anchor` (window coordinates), with `items`.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuSpec {
    pub anchor: Rect,
    pub items: Vec<MenuItem>,
}

/// The menu open now.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextMenu {
    pub spec: MenuSpec,
    /// The item under the pointer, or picked with the arrow keys.
    pub hover: Option<usize>,
    /// The output the menu opened on, which it stays within.
    pub output: Size<i32, Logical>,
}

impl ContextMenu {
    pub fn new(spec: MenuSpec, output: Size<i32, Logical>) -> Self {
        Self {
            spec,
            hover: None,
            output,
        }
    }

    /// The items shown: as many as fit on the output.
    fn items(&self) -> &[MenuItem] {
        let fit = chrome::menu_rows_fitting(self.output.h);
        &self.spec.items[..self.spec.items.len().min(fit)]
    }

    fn rows(&self) -> Vec<MenuRow> {
        self.items()
            .iter()
            .map(|item| MenuRow {
                label: item.label.clone(),
                hint: item.hint.clone(),
                enabled: item.enabled,
            })
            .collect()
    }

    /// What the chrome draws of it.
    pub fn view(&self) -> MenuView {
        MenuView {
            anchor: self.spec.anchor,
            rows: self.rows(),
            hover: self.hover,
        }
    }

    /// The panel and one rect per item (window coordinates).
    pub fn layout(&self) -> (Rect, Vec<Rect>) {
        chrome::menu_layout(self.spec.anchor, &self.rows(), self.output)
    }

    /// The item at `pos`, if any.
    pub fn item_at(&self, pos: Point<f64, Logical>) -> Option<usize> {
        let (_, items) = self.layout();
        items.iter().position(|item| item.to_f64().contains(pos))
    }

    /// Move the pick by `delta` rows (down for positive), over the enabled
    /// items, wrapping around; from the first or last with none picked.
    pub fn step(&mut self, delta: i32) {
        let items = self.items();
        let n = items.len() as i32;
        if n == 0 || !items.iter().any(|item| item.enabled) {
            return;
        }
        let mut at = match self.hover {
            Some(at) => at as i32 + delta,
            None if delta > 0 => 0,
            None => n - 1,
        };
        for _ in 0..n {
            at = at.rem_euclid(n);
            if items[at as usize].enabled {
                self.hover = Some(at as usize);
                return;
            }
            at += delta.signum();
        }
    }

    /// Pick the first enabled item (`last`: the last one).
    pub fn step_to_end(&mut self, last: bool) {
        self.hover = None;
        self.step(if last { -1 } else { 1 });
    }

    /// What the picked item does, if one is picked and enabled.
    pub fn pick(&self) -> Option<MenuAction> {
        let item = self.items().get(self.hover?)?;
        item.enabled.then(|| item.action.clone())
    }
}

impl Gerdr {
    /// Show `spec`; it takes every press and key until it closes.
    pub fn open_menu(&mut self, spec: MenuSpec) {
        self.tooltip.hide();
        self.context_menu = Some(ContextMenu::new(spec, self.output_size()));
        self.refresh_pointer_focus();
    }

    pub fn close_menu(&mut self) {
        if self.context_menu.take().is_some() {
            self.refresh_pointer_focus();
        }
    }

    /// The pointer moved to `pos` while the menu is open: light the item
    /// under it, or none off the items, so Enter picks nothing it left.
    pub fn menu_motion(&mut self, pos: Point<f64, Logical>) {
        if let Some(menu) = self.context_menu.as_mut() {
            menu.hover = menu.item_at(pos);
        }
    }

    /// Any press at `pos` while the menu is open: an enabled item runs,
    /// anything else just closes the menu. The press goes no further.
    pub fn menu_press(&mut self, pos: Point<f64, Logical>) {
        let picked = self.context_menu.as_ref().and_then(|menu| {
            let item = menu.items().get(menu.item_at(pos)?)?;
            item.enabled.then(|| item.action.clone())
        });
        self.close_menu();
        if let Some(action) = picked {
            self.menu_action(action);
        }
    }

    /// A key while the menu is open: the arrows, Home and End walk it,
    /// Enter picks, Esc closes. Whether the key was the menu's: with one
    /// open every key is, and any other only closes it, so a key meant
    /// for the menu never reaches the app under it.
    pub fn menu_key(&mut self, sym: Keysym) -> bool {
        let Some(menu) = self.context_menu.as_mut() else {
            return false;
        };
        match sym {
            Keysym::Up | Keysym::KP_Up => menu.step(-1),
            Keysym::Down | Keysym::KP_Down => menu.step(1),
            Keysym::Home | Keysym::KP_Home | Keysym::Page_Up | Keysym::KP_Page_Up => {
                menu.step_to_end(false)
            }
            Keysym::End | Keysym::KP_End | Keysym::Page_Down | Keysym::KP_Page_Down => {
                menu.step_to_end(true)
            }
            Keysym::Return | Keysym::KP_Enter => {
                let picked = menu.pick();
                self.close_menu();
                if let Some(action) = picked {
                    self.menu_action(action);
                }
            }
            _ => self.close_menu(),
        }
        true
    }

    /// Do what a menu item asks for, as the keys would.
    pub fn menu_action(&mut self, action: MenuAction) {
        match action {
            MenuAction::NewTerminal => self.run_action(Action::NewTerminal),
            MenuAction::ChangeProjectId(id, new) => self.change_project_id(&id, &new),
            MenuAction::UndoAfterShip(project, slug, step) => {
                self.undo_after_ship(&project, &slug, step);
            }
            MenuAction::NewCursor => self.open_cursor(None),
            MenuAction::OpenPage(w, url) => self.open_page(w, url),
            MenuAction::OpenPageInNewTab(w, url) => self.open_page_in_new_tab(w, url),
            MenuAction::Unwatch(key, n) => self.unwatch(&key, n),
            MenuAction::OpenFolder => self.pick_workspace(WorkspaceKind::Terminal),
            MenuAction::NewProject => self.run_action(Action::NewProject),
            MenuAction::CloseWorkspace(w) => self.close_workspace(w),
            MenuAction::RenameWorkspace(w) => self.start_rename(RenameTarget::Workspace(w)),
            MenuAction::RestartWorkspace(w) => self.restart_workspace(w),
            MenuAction::ChooseRestarts(w) => self.choose_restarts(w),
            MenuAction::NewAgent(w) => self.open_agent_picker(w),
            MenuAction::StartT3 => self.spawn_t3(None),
            MenuAction::StopT3 => self.stop_t3_asking(),
            MenuAction::RestartT3 => self.restart_t3_asking(),
            MenuAction::OpenT3 => self.open_t3(self.active_workspace, None),
            MenuAction::CheckT3 => self.check_t3(true),
            MenuAction::ShowLog(path) => self.show_log(path),
            MenuAction::StopKept(id) => self.ask_stop_kept(&id),
            MenuAction::AddMachine => self.open_machine_dialog(),
            MenuAction::AddMachineFrom { target, name } => self.add_machine_from(target, name),
            MenuAction::OpenMachines(name) => self.open_machines(Some(&name)),
            MenuAction::ConnectMachine(name) => self.connect_machine(&name),
            MenuAction::DisconnectMachine(name) => self.disconnect_machine(&name),
            MenuAction::StopTakeOver(project, slug) => self.stop_take(&project, &slug),
            MenuAction::StopAiWork => self.ai_work.stop_busy(),
            MenuAction::StopAiWorkPull(project, slug) => self.ai_work.stop_pull(&project, &slug),
            MenuAction::RemoveMachine(name) => self.ask_remove_machine(&name),
            MenuAction::UpdateT3(version) => self.update_t3(version),
            MenuAction::NewTab(w) => {
                self.switch_workspace(w);
                self.new_tab();
            }
            MenuAction::CloseTab(w, t) => {
                self.switch_workspace(w);
                self.close_tab(t);
            }
            MenuAction::RenameTab(w, t) => self.start_rename(RenameTarget::Tab(w, t)),
            MenuAction::SplitPane(id, dir) => {
                self.focus_pane(|_| Some(id));
                self.split_pane(dir);
            }
            MenuAction::ClosePane(id) => {
                self.focus_pane(|_| Some(id));
                self.close_pane();
            }
            MenuAction::MovePane(id, side) => self.move_pane(id, side),
            MenuAction::SwapPane(id, side) => self.swap_pane(id, side),
            MenuAction::FlipPane(id) => self.flip_pane(id),
            MenuAction::ZoomPane(id) => self.zoom_pane(id),
            MenuAction::PaneToTab(id, to) => self.move_pane_to_tab(id, to),
            MenuAction::Scrollback(id) => self.open_scrollback(id),
            MenuAction::RestartPane(id) => {
                self.focus_pane(|_| Some(id));
                self.restart_focused_pane();
            }
            MenuAction::StopPane(id) => {
                self.focus_pane(|_| Some(id));
                self.stop_focused_pane();
            }
            MenuAction::SendPrompt(id) => {
                self.focus_pane(|_| Some(id));
                self.send_focused_prompt();
            }
            MenuAction::WebStep(id, step) => {
                self.web_pane_step(id, step);
            }
            MenuAction::CopyPaneUrl(id) => self.copy_pane_url(id),
            MenuAction::FocusAgent(key) => self.focus_agent(&key),
            MenuAction::CloseAgent(key) => self.close_agent(&key),
            MenuAction::CopyPath(path) => self.copy_text(path.display().to_string()),
            MenuAction::AttachProject(id) => self.attach_project(&id),
            MenuAction::OpenSettings => self.open_settings(crate::launch::Place::Tab),
            MenuAction::OpenLog => self.open_log(crate::launch::Place::Tab),
            MenuAction::SetupLog(project, slug) => self.open_setup_log(&project, &slug),
            MenuAction::FoldAll(folded) => self.fold_all(folded),
            MenuAction::SwitchTab(w, t) => {
                self.switch_workspace(w);
                self.switch_tab(t);
            }
            MenuAction::GoToVisited(id, back) => self.go_to_visited(id, back),
            MenuAction::ReposAction(button) => self.repos_press(button),
            MenuAction::SetGitIdentity => self.open_identity(),
            MenuAction::RaiseInotifyLimit => self.ask_raise_inotify(),
            MenuAction::Theme(mode) => self.set_theme_mode(mode),
            MenuAction::RetryExpected(pane) => self.retry_expected(pane),
            MenuAction::UnlockKeyring => crate::keyring::ask(),
            MenuAction::CloseExpected(pane) => self.close_expected(pane),
            MenuAction::RunApp(place) => self.open_run_prompt(place),
            MenuAction::Worktrees(dir) => self.open_worktrees(Some(dir)),
            MenuAction::DeleteWorktree(path) => self.delete_merged_worktree(&path),
            MenuAction::Repos => self.open_repos(),
            MenuAction::Inbox => self.open_inbox(None),
            MenuAction::Features => self.open_features(),
            MenuAction::NewFeature(at) => self.open_new_feature(at),
            MenuAction::OpenFeature(project, slug) => self.open_linked_feature(&project, &slug),
            MenuAction::PlanFixes(project, slug) => self.send_findings(&project, &slug),
            MenuAction::MakeFindingFixes(project, slug) => {
                self.make_finding_fixes(&project, &slug);
            }
            MenuAction::InboxRestore {
                project,
                ids,
                unsee,
            } => {
                self.inbox_restore(&project, &ids);
                self.unsee_service_errors(&unsee);
            }
            MenuAction::InboxUndelete {
                project,
                todo,
                at,
                triage,
            } => self.inbox_undelete(&project, todo, at, triage),
            MenuAction::ServiceErrorsRestore(before) => self.restore_service_errors(*before),
            MenuAction::RestoreProjectCommands(before) => self.restore_project_commands(before),
            MenuAction::EditCommands { project, slug, tab } => {
                self.open_project_commands(&project, slug.as_deref(), tab);
            }
            MenuAction::AskRemoveSandboxLeftovers(stacks) => {
                self.ask_remove_sandbox_leftovers(stacks);
            }
            MenuAction::RestoreContextRepo {
                project,
                slug,
                repo,
            } => self.restore_context_repo(&project, &slug, *repo),
            MenuAction::RestoreItemComment {
                project,
                slug,
                of,
                id,
                note,
            } => match of {
                crate::feature::review_items::ItemOf::Finding => {
                    self.note_finding(&project, &slug, &id, &note);
                }
                crate::feature::review_items::ItemOf::Suggestion => {
                    self.restore_suggestion_comment(&project, &slug, &id, &note);
                }
            },
            MenuAction::WriteFinalPlan(project, slug) => self.write_final_plan(&project, &slug),
            MenuAction::RestoreOwnFinding {
                project,
                slug,
                at,
                finding,
            } => self.restore_own_finding(&project, &slug, at, *finding),
            MenuAction::RestoreManualComment {
                project,
                slug,
                removed,
            } => self.restore_manual_comment(&project, &slug, *removed),
            MenuAction::RestoreDiffNote {
                project,
                slug,
                at,
                comment,
            } => self.restore_diff_note(&project, &slug, at, *comment),
            MenuAction::RestoreSuggestion {
                project,
                slug,
                at,
                suggestion,
            } => self.restore_suggestion(&project, &slug, at, *suggestion),
            MenuAction::ReloadProjects => self.reload_projects(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ContextMenu, MenuAction, MenuItem, MenuSpec};
    use crate::chrome::Rect;

    fn menu() -> ContextMenu {
        ContextMenu::new(
            MenuSpec {
                anchor: Rect::new((10, 10).into(), (20, 20).into()),
                items: vec![
                    MenuItem::new("New tab", MenuAction::NewTab(0)).hint("Ctrl+B c"),
                    MenuItem::new("Rename", MenuAction::RenameWorkspace(0)).enabled(false),
                    MenuItem::new("Close", MenuAction::CloseWorkspace(0)),
                ],
            },
            (800, 600).into(),
        )
    }

    #[test]
    fn the_arrow_keys_walk_the_enabled_items_and_wrap() {
        let mut open = menu();
        assert_eq!(open.pick(), None);
        open.step(1);
        assert_eq!(open.hover, Some(0));
        // Down skips the disabled row and wraps at the end.
        open.step(1);
        assert_eq!(open.hover, Some(2));
        open.step(1);
        assert_eq!(open.hover, Some(0));
        open.step(-1);
        assert_eq!(open.hover, Some(2));
        assert_eq!(open.pick(), Some(MenuAction::CloseWorkspace(0)));
        // Up from nothing starts at the end.
        let mut open = menu();
        open.step(-1);
        assert_eq!(open.hover, Some(2));
        // A disabled row under the pointer picks nothing.
        open.hover = Some(1);
        assert_eq!(open.pick(), None);
    }

    #[test]
    fn home_and_end_go_to_the_ends_and_a_short_output_cuts_the_menu() {
        let mut open = menu();
        open.step_to_end(true);
        assert_eq!(open.hover, Some(2));
        open.step_to_end(false);
        assert_eq!(open.hover, Some(0));
        // Room for two rows: the third is not shown, picked or walked to.
        let mut short = ContextMenu::new(menu().spec, (800, 70).into());
        assert_eq!(short.layout().1.len(), 2);
        assert_eq!(short.view().rows.len(), 2);
        short.step(-1);
        assert_eq!(short.hover, Some(0));
        short.hover = Some(2);
        assert_eq!(short.pick(), None);
    }

    #[test]
    fn nothing_moves_with_every_item_disabled() {
        let mut menu = ContextMenu::new(
            MenuSpec {
                anchor: Rect::new((0, 0).into(), (1, 1).into()),
                items: vec![MenuItem::new("Nope", MenuAction::NewTerminal).enabled(false)],
            },
            (800, 600).into(),
        );
        menu.step(1);
        assert_eq!(menu.hover, None);
        let (panel, items) = menu.layout();
        assert_eq!(items.len(), 1);
        let on_item = (panel.loc.x as f64 + 5.0, panel.loc.y as f64 + 10.0).into();
        assert!(menu.item_at(on_item).is_some());
        assert!(menu.item_at((0.0, 0.0).into()).is_none());
    }
}

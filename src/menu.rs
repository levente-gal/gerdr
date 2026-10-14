//! Context menus: what a region of the chrome offers on a right click (or
//! under a button's chevron), one component for all of them. A region
//! builds a `MenuSpec`; `open_menu` shows it; a pick runs its
//! `MenuAction` through `menu_action`, which maps onto what the keys do.

use std::path::PathBuf;

use smithay::utils::{Logical, Point};

use crate::{
    Action, Gerdr, WorkspaceKind,
    chrome::{self, MenuRow, MenuView, Rect, ThemeMode},
    layout::Dir,
    rename::RenameTarget,
};

/// What a menu item does when picked.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuAction {
    NewTerminal,
    OpenFolder,
    NewProject,
    CloseWorkspace(usize),
    RenameWorkspace(usize),
    /// A new tab in workspace `w`.
    NewTab(usize),
    /// Tab `t` of workspace `w`.
    CloseTab(usize, usize),
    RenameTab(usize, usize),
    /// Split pane `id` of the active tab.
    SplitPane(u32, Dir),
    ClosePane(u32),
    RestartPane(u32),
    StopPane(u32),
    /// Show the agent in herdr pane `pane_id`.
    FocusAgent(String),
    CopyPath(PathBuf),
    Theme(ThemeMode),
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
    /// The item under the pointer.
    pub hover: Option<usize>,
}

impl ContextMenu {
    /// What the chrome draws of it.
    pub fn view(&self) -> MenuView {
        MenuView {
            anchor: self.spec.anchor,
            rows: self
                .spec
                .items
                .iter()
                .map(|item| MenuRow {
                    label: item.label.clone(),
                    hint: item.hint.clone(),
                    enabled: item.enabled,
                })
                .collect(),
            hover: self.hover,
        }
    }

    /// The panel and one rect per item (window coordinates).
    pub fn layout(&self) -> (Rect, Vec<Rect>) {
        chrome::menu_layout(self.spec.anchor, self.spec.items.len())
    }

    /// The item at `pos`, if any.
    pub fn item_at(&self, pos: Point<f64, Logical>) -> Option<usize> {
        let (_, items) = self.layout();
        items.iter().position(|item| item.to_f64().contains(pos))
    }
}

impl Gerdr {
    /// Show `spec`; it takes every press until it closes.
    pub fn open_menu(&mut self, spec: MenuSpec) {
        self.tooltip.hide();
        self.context_menu = Some(ContextMenu { spec, hover: None });
        self.refresh_pointer_focus();
    }

    pub fn close_menu(&mut self) {
        if self.context_menu.take().is_some() {
            self.refresh_pointer_focus();
        }
    }

    /// The pointer moved to `pos` while the menu is open: light the item
    /// under it.
    pub fn menu_motion(&mut self, pos: Point<f64, Logical>) {
        let hover = self.context_menu.as_ref().and_then(|m| m.item_at(pos));
        if let Some(menu) = self.context_menu.as_mut() {
            menu.hover = hover;
        }
    }

    /// Any press at `pos` while the menu is open: an enabled item runs,
    /// anything else just closes the menu. The press goes no further.
    pub fn menu_press(&mut self, pos: Point<f64, Logical>) {
        let picked = self.context_menu.as_ref().and_then(|menu| {
            let item = &menu.spec.items[menu.item_at(pos)?];
            item.enabled.then(|| item.action.clone())
        });
        self.close_menu();
        if let Some(action) = picked {
            self.menu_action(action);
        }
    }

    /// Do what a menu item asks for, as the keys would.
    pub fn menu_action(&mut self, action: MenuAction) {
        match action {
            MenuAction::NewTerminal => self.run_action(Action::NewTerminal),
            MenuAction::OpenFolder => self.pick_workspace(WorkspaceKind::Terminal),
            MenuAction::NewProject => self.run_action(Action::NewProject),
            MenuAction::CloseWorkspace(w) => self.close_workspace(w),
            MenuAction::RenameWorkspace(w) => self.start_rename(RenameTarget::Workspace(w)),
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
            MenuAction::RestartPane(id) => {
                self.focus_pane(|_| Some(id));
                self.restart_focused_pane();
            }
            MenuAction::StopPane(id) => {
                self.focus_pane(|_| Some(id));
                self.stop_focused_pane();
            }
            MenuAction::FocusAgent(pane_id) => self.focus_agent(&pane_id),
            MenuAction::CopyPath(path) => self.copy_text(path.display().to_string()),
            MenuAction::Theme(mode) => self.set_theme_mode(mode),
        }
    }

    /// The menu under the sidebar's terminal button: where else to open a
    /// terminal workspace.
    pub fn terminal_menu_spec(&self) -> MenuSpec {
        MenuSpec {
            anchor: chrome::sidebar_layout(&[], &[], self.sidebar_width).terminal,
            items: vec![MenuItem::new("Open folder…", MenuAction::OpenFolder)],
        }
    }
}

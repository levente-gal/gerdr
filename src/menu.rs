//! Context menus: what a region of the chrome offers on a right click (or
//! under a button's chevron), one component for all of them. A region
//! builds a `MenuSpec`; `open_menu` shows it; a pick runs its
//! `MenuAction` through `menu_action`, which maps onto what the keys do.
//! The arrow keys walk the menu, Enter picks, Esc closes.

use std::path::PathBuf;

use smithay::{
    input::keyboard::Keysym,
    utils::{Logical, Point, Size},
};

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
    /// Start again what pane `id` of the active tab waits for, after it
    /// failed to show a window.
    RetryExpected(u32),
    /// Open the run prompt for an app in `place`.
    RunApp(crate::launch::Place),
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

    fn rows(&self) -> Vec<MenuRow> {
        self.spec
            .items
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
        let n = self.spec.items.len() as i32;
        if n == 0 || !self.spec.items.iter().any(|item| item.enabled) {
            return;
        }
        let mut at = match self.hover {
            Some(at) => at as i32 + delta,
            None if delta > 0 => 0,
            None => n - 1,
        };
        for _ in 0..n {
            at = at.rem_euclid(n);
            if self.spec.items[at as usize].enabled {
                self.hover = Some(at as usize);
                return;
            }
            at += delta.signum();
        }
    }

    /// What the picked item does, if one is picked and enabled.
    pub fn pick(&self) -> Option<MenuAction> {
        let item = &self.spec.items[self.hover?];
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
    /// under it.
    pub fn menu_motion(&mut self, pos: Point<f64, Logical>) {
        let hover = self.context_menu.as_ref().and_then(|m| m.item_at(pos));
        if let Some(menu) = self.context_menu.as_mut()
            && hover.is_some()
        {
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

    /// A key while the menu is open: the arrows walk it, Enter picks,
    /// Esc closes. Whether the key was the menu's; any other closes the
    /// menu and goes on to whatever it would do otherwise.
    pub fn menu_key(&mut self, sym: Keysym) -> bool {
        let Some(menu) = self.context_menu.as_mut() else {
            return false;
        };
        match sym {
            Keysym::Up | Keysym::KP_Up => menu.step(-1),
            Keysym::Down | Keysym::KP_Down => menu.step(1),
            Keysym::Return | Keysym::KP_Enter => {
                let picked = menu.pick();
                self.close_menu();
                if let Some(action) = picked {
                    self.menu_action(action);
                }
            }
            Keysym::Escape => self.close_menu(),
            _ => {
                self.close_menu();
                return false;
            }
        }
        true
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
            MenuAction::RetryExpected(pane) => self.retry_expected(pane),
            MenuAction::RunApp(place) => self.open_run_prompt(place),
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

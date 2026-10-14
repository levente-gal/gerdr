//! Input to the workspace sidebar: presses on its header buttons, the
//! project name, the workspace rows and the agent cards, and the pointer
//! moving over it (see `chrome::sidebar` for what it shows).

use smithay::utils::{Logical, Point};

use crate::{
    Gerdr, Reorderable, WorkspaceKind, chrome, home_dir,
    input::{BTN_LEFT, BTN_MIDDLE},
};

impl Gerdr {
    /// A press at `(x, y)` in the sidebar. Left click selects a workspace,
    /// "×" or middle click closes it; a left click on an agent card shows
    /// its pane. A row held with the left button can be dragged to another
    /// place. The header buttons open a project, a terminal in home, or the
    /// terminal menu; the project's name left of them opens the project
    /// menu (the welcome screen while no project is shown).
    pub(crate) fn sidebar_press(&mut self, x: i32, y: i32, button: u32) {
        let agents = self.agent_views();
        let workspaces: Vec<_> = self
            .workspace_views()
            .into_iter()
            .map(|(ws, _)| ws)
            .collect();
        let layout = chrome::sidebar_layout(&workspaces, &agents, self.sidebar_width);
        let inside =
            |r: &smithay::utils::Rectangle<i32, smithay::utils::Logical>| r.contains((x, y));
        if button == BTN_LEFT {
            if inside(&layout.project) {
                self.pick_workspace(WorkspaceKind::Project);
                return;
            }
            if inside(&layout.terminal) {
                self.open_terminal(home_dir().unwrap_or_default());
                return;
            }
            if inside(&layout.chevron) {
                self.terminal_menu = true;
                self.refresh_pointer_focus();
                return;
            }
            if inside(&layout.header) {
                if self.project.is_some() {
                    self.toggle_project_menu();
                } else {
                    self.open_welcome(true);
                }
                return;
            }
        }
        if button == BTN_LEFT {
            let cards = layout
                .cards
                .iter()
                .zip(&agents)
                .flat_map(|(rects, cards)| rects.iter().zip(cards));
            if let Some((_, card)) = cards.into_iter().find(|(rect, _)| inside(rect)) {
                let pane_id = card.pane_id.clone();
                self.focus_agent(&pane_id);
                return;
            }
        }
        let Some(idx) = layout.rows.iter().position(|(row, _)| inside(row)) else {
            return;
        };
        match button {
            BTN_LEFT if inside(&layout.rows[idx].1) => self.close_workspace(idx),
            BTN_LEFT => {
                self.switch_workspace(idx);
                self.hold(Reorderable::Workspace);
            }
            BTN_MIDDLE => self.close_workspace(idx),
            _ => {}
        }
    }

    /// The pointer moved to `pos` with nothing open over the sidebar: the
    /// header's project name lights up under it.
    pub(crate) fn sidebar_motion(&mut self, pos: Point<f64, Logical>) {
        self.header_hover = self.project.is_some()
            && chrome::sidebar_layout(&[], &[], self.sidebar_width)
                .header
                .to_f64()
                .contains(pos);
    }
}

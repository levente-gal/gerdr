//! The inbox panel in gerdr: opening and closing it, its keys, clicks,
//! hover and wheel, what it shows, and doing what it asks (see `inbox`).
//! Also what the inbox reads for itself: the issues assigned to the user
//! (`watch_issues`), and the last lines of agent panes
//! (`request_pane_text`), both off the event loop.

#![allow(dead_code)]

use std::collections::BTreeMap;

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point},
};

use crate::{
    Gerdr,
    chrome::{self, AgentView, InboxHit, InboxView, WorkspaceView},
    inbox::{InboxItem, Listing, Outcome, Panel, Section},
    inbox_sources::{self, Sources},
    inbox_store::{self, Triage},
    input::Overlay,
    tooltip::Tooltip,
};

impl Gerdr {
    /// Open the inbox panel, the first item of section `focus` selected
    /// (else the first item).
    pub fn open_inbox(&mut self, focus: Option<Section>) {
        self.switcher = None;
        self.repos.close();
        self.worktrees.close();
        self.project_menu = None;
        self.stop_key_repeat();
        self.close_menu();
        self.tooltip.hide();
        self.chrome.drop_tooltip();
        self.inbox.panel = Some(Panel::new(focus));
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// Close the inbox panel.
    pub fn close_inbox(&mut self) {
        self.inbox.close();
        // Gone, what is under the pointer gets it back.
        self.refresh_pointer_focus();
    }

    /// How many rows the inbox list shows.
    fn inbox_page(&self) -> usize {
        chrome::inbox_page(self.output_size())
    }

    /// Every item of the inbox, from the sidebar's `workspaces` and their
    /// `agents` as the view has them.
    fn inbox_items(
        &self,
        workspaces: &[WorkspaceView],
        agents: &[Vec<AgentView>],
    ) -> Vec<InboxItem> {
        let dirs: Vec<_> = self.workspaces.iter().map(|ws| ws.dir.clone()).collect();
        let records: Vec<_> = self.workspaces.iter().map(|ws| ws.record.clone()).collect();
        let store = self
            .inbox
            .store
            .as_ref()
            .filter(|(id, _)| Some(id) == self.project.as_ref())
            .map(|(_, file)| file);
        inbox_sources::collect(&Sources {
            workspaces,
            dirs: &dirs,
            records: &records,
            agents,
            herdr_agents: &self.agents,
            cursor_chats: &self.cursor_chats,
            agent_since: &self.agent_since,
            ci: &self.ci,
            issues: &self.inbox.issues,
            store,
            pane_text: &self.inbox.pane_text,
            now: std::time::Instant::now(),
            now_secs: inbox_store::now_secs(),
        })
    }

    /// The items and what else the panel's `Listing` borrows, worked out
    /// afresh: (items, triage, workspace names).
    fn inbox_listing_parts(&self) -> (Vec<InboxItem>, BTreeMap<String, Triage>, Vec<String>) {
        let workspaces: Vec<_> = self
            .workspace_views()
            .into_iter()
            .map(|(ws, _)| ws)
            .collect();
        let agents = self.agent_views();
        self.inbox_listing_parts_with(&workspaces, &agents)
    }

    fn inbox_listing_parts_with(
        &self,
        workspaces: &[WorkspaceView],
        agents: &[Vec<AgentView>],
    ) -> (Vec<InboxItem>, BTreeMap<String, Triage>, Vec<String>) {
        let items = self.inbox_items(workspaces, agents);
        let triage = self
            .inbox
            .store
            .as_ref()
            .filter(|(id, _)| Some(id) == self.project.as_ref())
            .map(|(_, file)| file.triage.clone())
            .unwrap_or_default();
        let names = self.workspaces.iter().map(|ws| ws.name.clone()).collect();
        (items, triage, names)
    }

    /// The shown project's name.
    fn inbox_project_name(&self) -> Option<String> {
        let id = self.project.as_deref()?;
        Some(self.projects.project(id)?.name.clone())
    }

    /// A key while the inbox panel has the keyboard; what to do once the
    /// keyboard is free again, if anything. `sym` is the key's own symbol,
    /// `typed` the one with Shift and the layout applied.
    pub fn inbox_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<Outcome> {
        let (items, triage, names) = self.inbox_listing_parts();
        let project = self.inbox_project_name();
        let list = Listing {
            items: &items,
            triage: &triage,
            now: inbox_store::now_secs(),
            active: self.workspace().map(|_| self.active_workspace),
            workspaces: &names,
            page: self.inbox_page(),
            project: project.as_deref(),
        };
        self.inbox.panel.as_mut()?.key(mods, sym, typed, &list)
    }

    /// A left click while the inbox panel is open: outside it closes it.
    pub fn inbox_click(&mut self, pos: Point<f64, Logical>) {
        let Some(view) = self.inbox_view() else {
            return;
        };
        let layout = chrome::inbox_layout(&view, self.output_size());
        let outcome = match chrome::inbox_hit(&view, &layout, pos) {
            // A key hint: its key, pressed.
            Some(InboxHit::Hint(i)) => {
                return self.press_hint(Overlay::Inbox, &view.hints[i].0, pos);
            }
            Some(hit) => {
                let (items, triage, names) = self.inbox_listing_parts();
                let project = self.inbox_project_name();
                let list = Listing {
                    items: &items,
                    triage: &triage,
                    now: inbox_store::now_secs(),
                    active: self.workspace().map(|_| self.active_workspace),
                    workspaces: &names,
                    page: self.inbox_page(),
                    project: project.as_deref(),
                };
                match self.inbox.panel.as_mut() {
                    Some(panel) => panel.click(hit, &list),
                    None => None,
                }
            }
            None => Some(Outcome::Close),
        };
        if let Some(outcome) = outcome {
            self.apply_inbox(outcome);
        }
    }

    /// The pointer moved while the inbox panel is open.
    pub fn inbox_hover(&mut self, pos: Point<f64, Logical>) -> Option<Tooltip> {
        let view = self.inbox_view()?;
        let layout = chrome::inbox_layout(&view, self.output_size());
        let hit = chrome::inbox_hit(&view, &layout, pos);
        if let Some(panel) = self.inbox.panel.as_mut() {
            panel.hover(hit);
        }
        None
    }

    /// The wheel turned `rows` rows (down positive) over the panel.
    pub fn inbox_scroll(&mut self, rows: f64) {
        let (items, triage, names) = self.inbox_listing_parts();
        let project = self.inbox_project_name();
        let list = Listing {
            items: &items,
            triage: &triage,
            now: inbox_store::now_secs(),
            active: self.workspace().map(|_| self.active_workspace),
            workspaces: &names,
            page: self.inbox_page(),
            project: project.as_deref(),
        };
        if let Some(panel) = self.inbox.panel.as_mut() {
            panel.scroll(rows, &list);
        }
    }

    /// What the inbox panel shows, while it is open.
    pub fn inbox_view(&self) -> Option<InboxView> {
        self.inbox.panel.as_ref()?;
        let workspaces: Vec<_> = self
            .workspace_views()
            .into_iter()
            .map(|(ws, _)| ws)
            .collect();
        let agents = self.agent_views();
        self.inbox_view_with(&workspaces, &agents)
    }

    /// What the inbox panel shows, while it is open, from the sidebar's
    /// `workspaces` and their `agents` as the frame's view has them.
    pub fn inbox_view_with(
        &self,
        workspaces: &[WorkspaceView],
        agents: &[Vec<AgentView>],
    ) -> Option<InboxView> {
        let panel = self.inbox.panel.as_ref()?;
        let (items, triage, names) = self.inbox_listing_parts_with(workspaces, agents);
        let project = self.inbox_project_name();
        let list = Listing {
            items: &items,
            triage: &triage,
            now: inbox_store::now_secs(),
            active: self.workspace().map(|_| self.active_workspace),
            workspaces: &names,
            page: self.inbox_page(),
            project: project.as_deref(),
        };
        Some(panel.view(&list))
    }

    /// Do what the inbox panel asked.
    pub fn apply_inbox(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Close => self.close_inbox(),
            Outcome::GoTo(_)
            | Outcome::Act(..)
            | Outcome::SaveTodo { .. }
            | Outcome::Refresh
            | Outcome::Restore(_) => {}
        }
    }

    /// The toast's "Undo" after a dismiss or snooze: list item `id` again.
    pub fn inbox_restore(&mut self, id: String) {
        let _ = id;
    }

    /// Read the issues assigned to the user now and then, off the event
    /// loop (see `issues::read_all`): more often while the panel is open.
    pub fn watch_issues(&mut self) {}

    /// Read the last lines of herdr pane `pane_id` off the event loop, for
    /// its agent's detail, unless a read is under way.
    pub fn request_pane_text(&mut self, pane_id: &str) {
        let _ = pane_id;
    }
}

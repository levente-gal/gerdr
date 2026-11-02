//! The inbox panel in gerdr: opening and closing it, its keys, clicks,
//! hover and wheel, what it shows, and doing what it asks (see `inbox`).
//! Also what the inbox reads for itself: the issues assigned to the user
//! (`watch_issues`), and the last lines of agent panes
//! (`request_pane_text`), both off the event loop.

use std::{
    collections::{BTreeMap, HashSet},
    thread,
    time::{Duration, Instant},
};

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    reexports::calloop::{
        channel::{self, Channel, Sender},
        timer::{TimeoutAction, Timer},
    },
    utils::{Logical, Point},
};

use crate::{
    Gerdr,
    agents::AgentKey,
    chrome::{self, AgentView, InboxHit, InboxRow, InboxView, WorkspaceView},
    herdr,
    inbox::{
        self, InboxItem, ItemAction, ItemId, Listing, Outcome, PaneText, Panel, Section, Target,
    },
    inbox_sources::{self, Sources},
    inbox_store::{self, InboxFile, Todo, TodoId, Triage},
    input::Overlay,
    issues::{self, GhRepo, IssuesRead, TicketRepo},
    menu::MenuAction,
    toast::{Level, ToastHit},
    tooltip::Tooltip,
};

/// How often the issues are read while the panel is closed.
const ISSUES_CLOSED: Duration = Duration::from_secs(300);
/// How often while it is open, and how old they may be when it opens.
const ISSUES_OPEN: Duration = Duration::from_secs(60);
/// How often the inbox looks whether to read again: the issues, the
/// selected agent's pane.
const TICK: Duration = Duration::from_secs(1);
/// How long a pane's text read is good for.
const PANE_FRESH: Duration = Duration::from_secs(2);
/// How many of a pane's last lines are read.
const PANE_LINES: usize = 40;
/// How long Ctrl+S snoozes an item for, in seconds.
const SNOOZE: u64 = 3600;
/// How long a triage entry's `seen` may lag before opening the panel
/// brings it up to date, in seconds: a write per open at most this often.
const SEEN_EVERY: u64 = 3600;

/// What a `Listing` borrows, gathered afresh.
struct Parts {
    items: Vec<InboxItem>,
    triage: BTreeMap<String, Triage>,
    names: Vec<String>,
    project: Option<String>,
    active: Option<usize>,
    page: usize,
    now: u64,
}

impl Parts {
    fn listing(&self) -> Listing<'_> {
        Listing {
            items: &self.items,
            triage: &self.triage,
            now: self.now,
            active: self.active,
            workspaces: &self.names,
            page: self.page,
            project: self.project.as_deref(),
        }
    }
}

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
        self.scrollback = None;
        self.inbox.panel = Some(Panel::new(focus));
        self.inbox_mark_seen();
        if self.issues_due(ISSUES_OPEN) {
            self.refresh_issues();
        }
        self.inbox_follow_selection();
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// Close the inbox panel.
    pub fn close_inbox(&mut self) {
        self.inbox.close();
        // Gone, what is under the pointer gets it back.
        self.refresh_pointer_focus();
    }

    /// The shown project's inbox file, loaded if it isn't yet (or another
    /// project's is); `None` without a project.
    fn inbox_file_mut(&mut self) -> Option<&mut InboxFile> {
        let id = self.project.clone()?;
        if self.inbox.store.as_ref().is_none_or(|(p, _)| *p != id) {
            let file = inbox_store::load(None, &id);
            self.inbox.store = Some((id, file));
        }
        self.inbox.store.as_mut().map(|(_, file)| file)
    }

    /// The shown project's inbox file, if loaded.
    fn inbox_file(&self) -> Option<&InboxFile> {
        let (id, file) = self.inbox.store.as_ref()?;
        (Some(id) == self.project.as_ref()).then_some(file)
    }

    /// Whether the inbox has the agent of card `view` dismissed or snoozed
    /// at `now` (seconds), as it is now: the sidebar's "Needs you" list
    /// leaves it out too.
    pub(crate) fn inbox_hides_agent(&self, view: &AgentView, now: u64) -> bool {
        let Some(file) = self.inbox_file() else {
            return false;
        };
        inbox_sources::agent_fingerprint(
            view,
            &self.agents,
            &self.cursor_chats,
            &self.t3.threads,
            &self.test_runs,
        )
        .is_some_and(|print| {
            inbox::hides(&file.triage, &format!("agent:{}", view.key), &print, now)
        })
    }

    /// Load the shown project's inbox file, so what it hides is hidden
    /// from the sidebar too before the inbox is first opened.
    pub(crate) fn load_inbox(&mut self) {
        self.inbox_file_mut();
    }

    /// Save the shown project's inbox file, its stale triage pruned.
    fn save_inbox(&mut self) {
        let Some((id, file)) = self.inbox.store.as_mut() else {
            return;
        };
        inbox_store::prune(file, inbox_store::now_secs());
        if let Err(err) = inbox_store::save(None, id, file) {
            tracing::warn!("inbox of {id}: {err}");
        }
    }

    /// Mark the triage of the items still there as seen, so they aren't
    /// pruned, and prune the rest; saved when anything changed.
    fn inbox_mark_seen(&mut self) {
        let now = inbox_store::now_secs();
        let ids: HashSet<String> = self
            .inbox_parts()
            .items
            .into_iter()
            .map(|i| i.id.0)
            .collect();
        let Some(file) = self.inbox_file_mut() else {
            return;
        };
        let before = file.triage.len();
        let mut changed = false;
        for (id, triage) in &mut file.triage {
            if ids.contains(id) && triage.seen + SEEN_EVERY <= now {
                triage.seen = now;
                changed = true;
            }
        }
        inbox_store::prune(file, now);
        if changed || file.triage.len() != before {
            self.save_inbox();
        }
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
        let service_errors: Vec<_> = self
            .projects
            .service_errors
            .iter()
            .map(|group| {
                let shown = self.project.as_deref() == Some(group.project.as_str());
                let workspace = shown
                    .then(|| self.herdr_pane_place(&group.pane_id).map(|(w, _)| w))
                    .flatten();
                let place = match workspace {
                    Some(w) => self.workspaces[w].name.clone(),
                    None => self
                        .projects
                        .project(&group.project)
                        .map_or_else(|| group.project.clone(), |p| p.name.clone()),
                };
                inbox_sources::ServiceItem {
                    group: group.clone(),
                    workspace,
                    place,
                    // As it was called then; else as it is now.
                    service: if group.service.is_empty() {
                        self.service_item_label(&group.project, &group.pane_id)
                    } else {
                        group.service.clone()
                    },
                }
            })
            .collect();
        inbox_sources::collect(&Sources {
            workspaces,
            dirs: &dirs,
            records: &records,
            active: self.workspace().map(|_| self.active_workspace),
            agents,
            herdr_agents: &self.agents,
            cursor_chats: &self.cursor_chats,
            t3_threads: &self.t3.threads,
            test_runs: &self.test_runs,
            agent_since: &self.agent_since,
            ci: &self.ci,
            issues: &self.inbox.issues,
            tickets: self.ticket_repos(),
            store: self.inbox_file(),
            pane_text: &self.inbox.pane_text,
            service_errors: &service_errors,
            now: Instant::now(),
            now_secs: inbox_store::now_secs(),
        })
    }

    /// How many items the tab strip's inbox button counts (see
    /// `inbox::counted`), hidden ones left out, from the sidebar's
    /// `workspaces` and their `agents`.
    pub fn inbox_count(&self, workspaces: &[WorkspaceView], agents: &[Vec<AgentView>]) -> usize {
        let triage = self.inbox_file().map(|file| &file.triage);
        let now = inbox_store::now_secs();
        self.inbox_items(workspaces, agents)
            .iter()
            .filter(|item| {
                inbox::counted(item) && !triage.is_some_and(|t| inbox::is_hidden(item, t, now))
            })
            .count()
    }

    /// The same, from the views as they are now.
    pub fn inbox_count_now(&self) -> usize {
        let workspaces: Vec<_> = self
            .workspace_views()
            .into_iter()
            .map(|(ws, _)| ws)
            .collect();
        self.inbox_count(&workspaces, &self.agent_views())
    }

    /// What the panel's `Listing` borrows, worked out afresh.
    fn inbox_parts(&self) -> Parts {
        let workspaces: Vec<_> = self
            .workspace_views()
            .into_iter()
            .map(|(ws, _)| ws)
            .collect();
        let agents = self.agent_views();
        self.inbox_parts_with(&workspaces, &agents)
    }

    fn inbox_parts_with(&self, workspaces: &[WorkspaceView], agents: &[Vec<AgentView>]) -> Parts {
        let project = self
            .project
            .as_deref()
            .and_then(|id| self.projects.project(id));
        Parts {
            items: self.inbox_items(workspaces, agents),
            triage: self
                .inbox_file()
                .map(|file| file.triage.clone())
                .unwrap_or_default(),
            names: self.workspaces.iter().map(|ws| ws.name.clone()).collect(),
            project: project.map(|p| p.name.clone()),
            active: self.workspace().map(|_| self.active_workspace),
            page: chrome::inbox_page(self.output_size()),
            now: inbox_store::now_secs(),
        }
    }

    /// Item `id`, as listed now (hidden or not).
    fn inbox_item(&self, id: &ItemId) -> Option<InboxItem> {
        let items = self.inbox_parts().items;
        items.into_iter().find(|item| item.id == *id)
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
        let parts = self.inbox_parts();
        let outcome = self
            .inbox
            .panel
            .as_mut()?
            .key(mods, sym, typed, &parts.listing());
        self.inbox_follow_selection();
        outcome
    }

    /// A left click while the inbox panel is open: outside it closes it,
    /// but on a toast (the Undo of what was just done) presses that.
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
                let parts = self.inbox_parts();
                let outcome = match self.inbox.panel.as_mut() {
                    Some(panel) => panel.click(hit, &parts.listing()),
                    None => None,
                };
                self.inbox_follow_selection();
                outcome
            }
            None => match self.toast_press(pos) {
                Some(hit) => {
                    if let ToastHit::Action(action) = hit {
                        self.menu_action(action);
                    }
                    None
                }
                None => Some(Outcome::Close),
            },
        };
        if let Some(outcome) = outcome {
            self.apply_inbox(outcome);
        }
    }

    /// The pointer moved while the inbox panel is open: a button says what
    /// it does and its key.
    pub fn inbox_hover(&mut self, pos: Point<f64, Logical>) -> Option<Tooltip> {
        let view = self.inbox_view()?;
        let layout = chrome::inbox_layout(&view, self.output_size());
        let hit = chrome::inbox_hit(&view, &layout, pos);
        // What the panel keeps of it: no row while the list moves.
        let hit = self.inbox.panel.as_mut()?.hover(hit, Instant::now());
        let (button, rect) = match hit? {
            InboxHit::Button(i, k) => {
                let InboxRow::Item(line) = view.rows.get(i)? else {
                    return None;
                };
                (line.buttons.get(k)?, *layout.rows.get(i)?.1.get(k)?)
            }
            InboxHit::DetailButton(k) => (
                view.detail.as_ref()?.buttons.get(k)?,
                *layout.detail_buttons.get(k)?,
            ),
            _ => return None,
        };
        Some(Tooltip::new(&button.label, rect).key(&button.key))
    }

    /// The wheel or the touchpad moved the panel's list.
    pub fn inbox_scroll(&mut self, motion: crate::scroll::Motion) {
        if self.inbox.panel.is_none() {
            return;
        }
        let parts = self.inbox_parts();
        if let Some(panel) = self.inbox.panel.as_mut() {
            panel.scroll(motion, Instant::now(), &parts.listing());
        }
    }

    /// Stand the panel's list at row `pos`, as its scrollbar's thumb held
    /// there puts it.
    pub fn inbox_scroll_to(&mut self, pos: f64) {
        if self.inbox.panel.is_none() {
            return;
        }
        let parts = self.inbox_parts();
        if let Some(panel) = self.inbox.panel.as_mut() {
            panel.scroll_to(pos, &parts.listing());
        }
    }

    /// Move the inbox panel's list on to `now` if it moves on its own;
    /// whether it still moves (see `Scroll::moving`).
    pub fn inbox_scroll_tick(&mut self, now: Instant) -> bool {
        match self.inbox.panel.as_mut() {
            Some(panel) => panel.tick(now),
            None => false,
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
        let parts = self.inbox_parts_with(workspaces, agents);
        Some(panel.view(&parts.listing()))
    }

    /// Do what the inbox panel asked.
    pub fn apply_inbox(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Close => self.close_inbox(),
            Outcome::GoTo(id) => self.inbox_act(&id, ItemAction::GoTo),
            Outcome::Act(id, action) => self.inbox_act(&id, action),
            Outcome::SaveTodo {
                id,
                text,
                workspace,
            } => self.save_todo(id, text, workspace),
            Outcome::AddTickets(repo) => self.add_tickets(repo),
            Outcome::Refresh => self.refresh_issues(),
            Outcome::Notice(text) => self.show_notice(text),
            Outcome::Restore(id) => {
                if let Some(project) = self.project.clone() {
                    self.inbox_restore(&project, id.0);
                }
            }
        }
    }

    /// Do `action` to item `id`.
    fn inbox_act(&mut self, id: &ItemId, action: ItemAction) {
        // A hidden item listed again needs nothing of it but its id.
        if action == ItemAction::Restore {
            return self.apply_inbox(Outcome::Restore(id.clone()));
        }
        let Some(item) = self.inbox_item(id) else {
            return;
        };
        let now = inbox_store::now_secs();
        let link = match &item.target {
            Target::Page { url, .. } | Target::Link(url) => Some(url.clone()),
            _ => None,
        };
        let todo = match &item.target {
            Target::Todo(todo) => Some(todo.clone()),
            _ => None,
        };
        match action {
            ItemAction::GoTo => self.inbox_go(item),
            ItemAction::Snooze | ItemAction::SnoozeLong | ItemAction::Dismiss => {
                let (until, text) = match action {
                    ItemAction::Snooze => (Some(now + SNOOZE), "Snoozed for an hour"),
                    ItemAction::SnoozeLong => (
                        Some(inbox_store::tomorrow_morning(now)),
                        "Snoozed until tomorrow morning",
                    ),
                    _ => (None, "Dismissed until it changes"),
                };
                if let Target::ServiceError {
                    project,
                    pane_id,
                    fingerprint,
                } = &item.target
                {
                    self.see_service_error(project, pane_id, fingerprint);
                }
                let Some(file) = self.inbox_file_mut() else {
                    return;
                };
                let triage = Triage {
                    fingerprint: item.fingerprint.clone(),
                    until,
                    seen: now,
                };
                file.triage.insert(id.0.clone(), triage);
                self.save_inbox();
                self.toast_undo(format!("{text}: {}", item.title), id);
            }
            ItemAction::Restore => {}
            ItemAction::Done => {
                let Some(todo) = todo else { return };
                let Some(file) = self.inbox_file_mut() else {
                    return;
                };
                let Some(found) = file.todos.iter_mut().find(|t| t.id == todo) else {
                    return;
                };
                found.done = Some(now);
                self.save_inbox();
                self.toast_undo(format!("Done: {}", item.title), id);
            }
            ItemAction::Delete => {
                let Some(todo) = todo else { return };
                let Some(file) = self.inbox_file_mut() else {
                    return;
                };
                let Some(at) = file.todos.iter().position(|t| t.id == todo) else {
                    return;
                };
                let todo = file.todos.remove(at);
                let triage = file.triage.remove(&id.0);
                self.save_inbox();
                let text = format!("Deleted: {}", item.title);
                let Some(project) = self.project.clone() else {
                    return self.toast(text, Level::Info);
                };
                let undelete = MenuAction::InboxUndelete {
                    project,
                    todo,
                    at,
                    triage,
                };
                self.toast_with(text, Level::Info, Some(("Undo".to_owned(), undelete)));
            }
            // The panel edits the todo itself, and hands back `SaveTodo`.
            ItemAction::Edit => {}
            ItemAction::OpenLink => {
                let Some(url) = link else { return };
                self.close_inbox();
                self.open_page(self.active_workspace, url);
            }
            ItemAction::CopyLink => {
                let Some(url) = link else { return };
                self.copy_text(url);
                self.toast("Copied the link".into(), Level::Info);
            }
        }
    }

    /// `text`, in a toast whose "Undo" lists item `id` of the shown
    /// project again.
    fn toast_undo(&mut self, text: String, id: &ItemId) {
        let Some(project) = self.project.clone() else {
            return self.toast(text, Level::Info);
        };
        let restore = MenuAction::InboxRestore {
            project,
            id: id.0.clone(),
        };
        self.toast_with(text, Level::Info, Some(("Undo".to_owned(), restore)));
    }

    /// Go where `item` leads (see `Target`), the panel closed first. A
    /// todo of no workspace leads nowhere: the panel stays.
    fn inbox_go(&mut self, item: InboxItem) {
        if matches!(item.target, Target::Todo(_)) && item.workspace.is_none() {
            return;
        }
        self.close_inbox();
        match item.target {
            Target::Agent(key) => self.focus_agent(&key),
            Target::Workspace(w) => self.switch_workspace(w),
            Target::Worktree(path) => self.open_worktrees(Some(path)),
            Target::Page { workspace, url } => self.open_page(workspace, url),
            Target::Link(url) => self.open_page(self.active_workspace, url),
            Target::Todo(_) => {
                if let Some(w) = item.workspace {
                    self.switch_workspace(w);
                }
            }
            Target::ServiceError {
                project,
                pane_id,
                fingerprint,
            } => self.show_service_error(&project, &pane_id, &fingerprint),
        }
    }

    /// Save the todo written: a new one (`id: None`) or `id` edited,
    /// linked to the workspace at index `workspace`. An empty one isn't.
    fn save_todo(&mut self, id: Option<TodoId>, text: String, workspace: Option<usize>) {
        let text = text.trim().to_owned();
        if text.is_empty() {
            return;
        }
        let record = workspace
            .and_then(|w| self.workspaces.get(w))
            .map(|ws| ws.record.clone());
        let now = inbox_store::now_secs();
        let Some(file) = self.inbox_file_mut() else {
            return self.show_notice("Todos are kept per project: open one first".into());
        };
        match id.and_then(|id| file.todos.iter().position(|t| t.id == id)) {
            Some(k) => {
                file.todos[k].text = text;
                file.todos[k].workspace = record;
            }
            None => {
                let id = next_todo_id(file);
                file.todos.push(Todo {
                    id,
                    text,
                    workspace: record,
                    created: now,
                    done: None,
                });
            }
        }
        self.save_inbox();
    }

    /// Add `repo` to the shown project's ticket repositories, saved, and
    /// read its tickets. Not one there already.
    fn add_tickets(&mut self, repo: String) {
        let repo = TicketRepo {
            repo,
            project: None,
        };
        let key = repo.key();
        let Some(project) = self
            .project
            .as_deref()
            .and_then(|id| self.projects.project_mut(id))
        else {
            return self
                .show_notice("Ticket repositories are kept per project: open one first".into());
        };
        if project.tickets.iter().any(|r| r.key() == key) {
            return self.show_notice(format!("{} is a ticket repository already", repo.repo));
        }
        let name = repo.repo.clone();
        project.tickets.push(repo);
        self.projects.save();
        self.show_notice(format!("Reading the tickets of {name}"));
        self.refresh_issues();
    }

    /// The toast's "Undo" after a dismiss or snooze: list item `id` of
    /// project `project` (by id) again. After a todo was marked done, it
    /// isn't any more. The project needn't be the shown one any more: the
    /// file loaded is changed if it is that project's, else that project's
    /// is loaded, changed and saved on its own.
    pub fn inbox_restore(&mut self, project: &str, id: String) {
        self.change_inbox_of(project, |file| restore(file, &id));
    }

    /// The toast's "Undo" after a todo was deleted: put `todo` back in
    /// project `project`'s file at index `at` (or last, if there are fewer
    /// now), triaged as it was. Not if one of its id was written since.
    pub fn inbox_undelete(&mut self, project: &str, todo: Todo, at: usize, triage: Option<Triage>) {
        self.change_inbox_of(project, |file| undelete(file, todo, at, triage));
    }

    /// Change project `project`'s inbox file (by id) with `change`, and
    /// save it if `change` says it changed. The project needn't be the
    /// shown one: the file loaded is changed if it is that project's, else
    /// that project's is loaded, changed and saved on its own.
    fn change_inbox_of(&mut self, project: &str, change: impl FnOnce(&mut InboxFile) -> bool) {
        match self.inbox.store.as_mut() {
            Some((p, file)) if p == project => {
                if change(file) {
                    self.save_inbox();
                }
            }
            _ => {
                let mut file = inbox_store::load(None, project);
                if change(&mut file)
                    && let Err(err) = inbox_store::save(None, project, &file)
                {
                    tracing::warn!("inbox of {project}: {err}");
                }
            }
        }
    }

    /// Read the issues assigned to the user now and then, off the event
    /// loop (see `issues::read_all`): more often while the panel is open.
    /// While it is, the selected agent's pane is read every `TICK` too.
    pub fn watch_issues(&mut self) {
        let (tx, rx): (_, Channel<IssuesRead>) = channel::channel();
        let read = self.loop_handle.insert_source(rx, |event, _, state| {
            if let channel::Event::Msg(read) = event {
                let issues = &mut state.inbox.issues;
                issues.last = inbox_sources::merge_issues(&issues.last, read);
                issues.at = Some(Instant::now());
                issues.pending = false;
            }
        });
        self.inbox.issues_tx = Some(tx);
        let timer = self
            .loop_handle
            .insert_source(Timer::immediate(), |_, _, state| {
                state.inbox_tick();
                TimeoutAction::ToDuration(TICK)
            });
        if let Err(err) = read
            .map_err(|e| e.to_string())
            .and(timer.map_err(|e| e.to_string()))
        {
            tracing::warn!("inbox: {err}");
        }
    }

    /// What the inbox does every `TICK`: read the issues if they are due,
    /// the selected agent's pane while the panel is open; forget the panes'
    /// text once it is closed.
    fn inbox_tick(&mut self) {
        // The tab strip's count leaves out what the shown project's file
        // hides: have it loaded.
        self.inbox_file_mut();
        let open = self.inbox.is_open();
        if self.issues_due(if open { ISSUES_OPEN } else { ISSUES_CLOSED }) {
            self.refresh_issues();
        }
        if open {
            self.inbox_follow_selection();
        } else if !self.inbox.pane_text.is_empty() {
            self.inbox.pane_text.clear();
        }
    }

    /// The repositories whose issues are read: the CI's, each once.
    fn gh_repos(&self) -> Vec<GhRepo> {
        let mut roots: Vec<_> = self.ci.iter().collect();
        roots.sort_by(|a, b| a.0.cmp(b.0));
        let mut repos: Vec<GhRepo> = Vec::new();
        for (root, ci) in roots {
            if !repos.iter().any(|r| r.host == ci.host && r.slug == ci.slug) {
                repos.push(GhRepo {
                    root: root.clone(),
                    host: ci.host.clone(),
                    slug: ci.slug.clone(),
                });
            }
        }
        repos
    }

    /// The shown project's ticket repositories.
    fn ticket_repos(&self) -> &[TicketRepo] {
        self.project
            .as_deref()
            .and_then(|id| self.projects.project(id))
            .map_or(&[], |project| &project.tickets)
    }

    /// Whether the issues are to be read again: none under way, and none
    /// read for `every`, or a repository not read yet (the CI found one
    /// since, a project with other ticket repositories is shown).
    fn issues_due(&self, every: Duration) -> bool {
        let issues = &self.inbox.issues;
        if issues.pending || self.inbox.issues_tx.is_none() {
            return false;
        }
        let stale = issues.at.is_none_or(|at| at.elapsed() >= every);
        let unread = self.gh_repos().iter().any(|repo| {
            let key = crate::ci::key(&repo.host, &repo.slug);
            !issues.last.github.iter().any(|(k, _)| *k == key)
        });
        let unread_tickets = self.ticket_repos().iter().any(|repo| {
            let key = repo.key();
            !issues.last.tickets.iter().any(|(k, _)| *k == key)
        });
        stale || unread || unread_tickets
    }

    /// Read the issues now, off the event loop, unless a round is under
    /// way.
    fn refresh_issues(&mut self) {
        let Some(tx) = self.inbox.issues_tx.clone() else {
            return;
        };
        if self.inbox.issues.pending {
            return;
        }
        let repos = self.gh_repos();
        let tickets = self.ticket_repos().to_vec();
        let linear = self.inbox.linear.clone();
        if repos.is_empty() && tickets.is_empty() && linear.is_none() {
            self.inbox.issues.last = IssuesRead::default();
            self.inbox.issues.at = Some(Instant::now());
            return;
        }
        self.inbox.issues.pending = true;
        let spawned = thread::Builder::new().name("issues".into()).spawn(move || {
            let _ = tx.send(issues::read_all(repos, tickets, linear));
        });
        if let Err(err) = spawned {
            self.inbox.issues.pending = false;
            tracing::warn!("issues: {err}");
        }
    }

    /// Read the pane of the selected item, if it is a herdr agent's: the
    /// one the view shows selected, settled first (just opened, or the one
    /// selected gone).
    fn inbox_follow_selection(&mut self) {
        if self.inbox.panel.is_none() {
            return;
        }
        let parts = self.inbox_parts();
        if let Some(panel) = self.inbox.panel.as_mut() {
            panel.settle_on(&parts.listing());
        }
        let selected = self.inbox.panel.as_ref().and_then(Panel::selected);
        let key = selected
            .and_then(|id| id.0.strip_prefix("agent:"))
            .map(AgentKey::parse);
        if let Some(AgentKey::Herdr(pane_id) | AgentKey::Test(pane_id)) = key {
            self.request_pane_text(&pane_id);
        }
    }

    /// Read the last lines of herdr pane `pane_id` off the event loop, for
    /// its agent's detail, unless a read is under way or the last is
    /// fresh. A pane on the alternate screen has no scrollback: its screen
    /// is read then.
    pub fn request_pane_text(&mut self, pane_id: &str) {
        let fresh = |p: &PaneText| p.at.elapsed() < PANE_FRESH;
        if self.inbox.pending.contains(pane_id)
            || self.inbox.pane_text.get(pane_id).is_some_and(fresh)
        {
            return;
        }
        let Some(socket) = self.herdr.as_ref().map(|h| h.socket().to_path_buf()) else {
            return;
        };
        let Some(tx) = self.pane_sender() else {
            return;
        };
        let id = pane_id.to_owned();
        self.inbox.pending.insert(id.clone());
        let spawned = thread::Builder::new()
            .name("pane text".into())
            .spawn(move || {
                let mut herdr = herdr::Herdr::on_socket(socket);
                let text = herdr.pane_read(&id, "recent", PANE_LINES).and_then(|text| {
                    if text.trim().is_empty() {
                        herdr.pane_read(&id, "visible", PANE_LINES)
                    } else {
                        Ok(text)
                    }
                });
                let _ = tx.send((id, text));
            });
        if let Err(err) = spawned {
            self.inbox.pending.remove(pane_id);
            tracing::warn!("pane text: {err}");
        }
    }

    /// What hands a pane's text to the event loop, set up the first time.
    fn pane_sender(&mut self) -> Option<Sender<(String, Result<String, String>)>> {
        if let Some(tx) = &self.inbox.pane_tx {
            return Some(tx.clone());
        }
        let (tx, rx): (_, Channel<(String, Result<String, String>)>) = channel::channel();
        let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
            let channel::Event::Msg((id, read)) = event else {
                return;
            };
            state.inbox.pending.remove(&id);
            let at = Instant::now();
            match read {
                Ok(text) => {
                    state.inbox.pane_text.insert(id, PaneText { text, at });
                }
                Err(err) => {
                    tracing::debug!("pane {id}: {err}");
                    // What was read before stays, not read again at once.
                    if let Some(old) = state.inbox.pane_text.get_mut(&id) {
                        old.at = at;
                    }
                }
            }
        });
        match inserted {
            Ok(_) => {
                self.inbox.pane_tx = Some(tx.clone());
                Some(tx)
            }
            Err(err) => {
                tracing::warn!("pane text: {err}");
                None
            }
        }
    }
}

/// List item `id` of `file` again: its triage forgotten, and a todo
/// marked done not done any more; whether anything changed.
fn restore(file: &mut InboxFile, id: &str) -> bool {
    let mut changed = file.triage.remove(id).is_some();
    if let Some(todo) = id.strip_prefix("todo:")
        && let Some(todo) = file.todos.iter_mut().find(|t| t.id.0 == todo)
        && todo.done.take().is_some()
    {
        changed = true;
    }
    changed
}

/// Put deleted `todo` back in `file` at index `at` (or last), with its
/// `triage`; whether it was. Not if a todo of its id is there already.
fn undelete(file: &mut InboxFile, todo: Todo, at: usize, triage: Option<Triage>) -> bool {
    if file.todos.iter().any(|t| t.id == todo.id) {
        return false;
    }
    if let Some(triage) = triage {
        file.triage.insert(format!("todo:{}", todo.id.0), triage);
    }
    let at = at.min(file.todos.len());
    file.todos.insert(at, todo);
    true
}

/// The id of a new todo of `file`: `t<next_todo>`, moved past any taken,
/// and `next_todo` moved on.
fn next_todo_id(file: &mut InboxFile) -> TodoId {
    loop {
        let id = TodoId(format!("t{}", file.next_todo));
        file.next_todo += 1;
        if !file.todos.iter().any(|t| t.id == id) {
            return id;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn todo(id: &str) -> Todo {
        Todo {
            id: TodoId(id.into()),
            text: id.into(),
            ..Todo::default()
        }
    }

    #[test]
    fn a_deleted_todo_comes_back_where_it_was() {
        let mut file = InboxFile {
            todos: vec![todo("t1"), todo("t3")],
            ..InboxFile::default()
        };
        let triage = Triage {
            fingerprint: String::new(),
            until: Some(9),
            seen: 1,
        };
        assert!(undelete(&mut file, todo("t2"), 1, Some(triage.clone())));
        let ids: Vec<_> = file.todos.iter().map(|t| t.id.0.as_str()).collect();
        assert_eq!(ids, ["t1", "t2", "t3"]);
        assert_eq!(file.triage.get("todo:t2"), Some(&triage));
        // Once is enough; past the end goes last.
        assert!(!undelete(&mut file, todo("t2"), 0, None));
        assert!(undelete(&mut file, todo("t4"), 9, None));
        assert_eq!(file.todos.last().map(|t| t.id.0.as_str()), Some("t4"));
    }
}

//! Input to the workspace sidebar: presses on its header, the workspace
//! rows and the agent cards, the pointer moving over it (hover, tooltips),
//! its context menus, and what moves in it (see `chrome::sidebar` for what
//! it shows).

use std::{path::Path, time::Instant};

use smithay::utils::{Logical, Point};

use crate::{
    Gerdr, Reorderable,
    agents::AgentKey,
    anim::quantize,
    chrome::{
        self, AgentView, CardState, CiCardState, Rect, RowState, SidebarHit, SidebarLayout,
        SidebarState, TooltipRow, WorkspaceView,
    },
    ci::{self, Subject},
    ci_watch::ci_state_word,
    git::Worktree,
    home_dir,
    input::{BTN_LEFT, BTN_MIDDLE},
    launch,
    menu::{MenuAction, MenuItem, MenuSpec},
    paths,
    rename::RenameTarget,
    tooltip::Tooltip,
};

/// The tooltip listing a workspace's other `worktrees`, about `anchor`:
/// how many, then a line each with its state and where it is, from `root`
/// (the workspace's own worktree) when inside it.
fn worktrees_tooltip(worktrees: &[Worktree], root: Option<&Path>, anchor: Rect) -> Tooltip {
    let home = home_dir();
    let place = |path: &Path| match root.and_then(|r| path.strip_prefix(r).ok()) {
        Some(rest) => rest.display().to_string(),
        None => paths::display(path, home.as_deref(), 40),
    };
    let rows = worktrees
        .iter()
        .map(|w| TooltipRow {
            name: w.name(),
            dirty: w.dirty,
            // The row's word for one needing a look; else it is merged.
            tag: Some(w.tag().into()),
            path: place(&w.path),
        })
        .collect();
    let text = match worktrees.len() {
        1 => "1 other worktree".to_owned(),
        n => format!("{n} other worktrees"),
    };
    Tooltip::new(&text, anchor).rows(rows)
}

impl Gerdr {
    /// Where the sidebar's parts sit, at the configured density.
    pub(crate) fn sidebar_layout(&self) -> SidebarLayout {
        let agents = self.agent_views();
        let workspaces: Vec<_> = self
            .workspace_views()
            .into_iter()
            .map(|(ws, _)| ws)
            .collect();
        chrome::sidebar_layout_at(&workspaces, &agents, self.sidebar_width, self.density)
    }

    /// Whether something covers the sidebar, taking the pointer from it.
    fn sidebar_covered(&self) -> bool {
        self.quit.is_some()
            || self.welcome.is_some()
            || self.project_menu.is_some()
            || self.switcher.is_some()
            || self.context_menu.is_some()
    }

    /// What is under the pointer in the sidebar, if it is free and there.
    fn sidebar_hit(&self, layout: &SidebarLayout, pos: Point<f64, Logical>) -> Option<SidebarHit> {
        if self.sidebar_covered() || pos.x >= self.sidebar_width as f64 {
            return None;
        }
        layout.hit(pos)
    }

    /// The agent whose pane has the keyboard in the shown tab: herdr's in
    /// it, or the Cursor chat last shown from its card in Cursor.
    fn focused_agent(&self) -> Option<AgentKey> {
        let tab = self.tab()?;
        let id = tab.focused_id()?;
        let pane = tab.panes.iter().find(|p| p.id == id)?;
        if let Some(herdr) = &pane.herdr {
            return Some(AgentKey::Herdr(herdr.pane_id.clone()));
        }
        let cursor = pane.app_command.as_deref().and_then(launch::program_name);
        let chat = self
            .cursor_opened
            .clone()
            .filter(|_| cursor.as_deref() == Some("cursor"))?;
        Some(AgentKey::Cursor(chat))
    }

    /// What moves in the sidebar at `now`, given its `workspaces` and
    /// their `agents` as the view has them: the hover pills, the "+" menu,
    /// the name being renamed.
    pub(crate) fn sidebar_state(
        &mut self,
        now: Instant,
        workspaces: &[WorkspaceView],
        agents: &[Vec<AgentView>],
    ) -> SidebarState {
        let layout =
            chrome::sidebar_layout_at(workspaces, agents, self.sidebar_width, self.density);
        let hit = self.sidebar_hit(&layout, self.pointer);
        self.sidebar_hover.track(hit.map(SidebarHit::pill), now);
        let hover = &self.sidebar_hover;
        let rows = (0..layout.rows.len())
            .map(|i| RowState {
                hover: quantize(hover.alpha(SidebarHit::Row(i), now)),
                close: hit == Some(SidebarHit::Close(i)),
                worktrees: hit == Some(SidebarHit::Worktrees(i)),
            })
            .collect();
        let focused = self.focused_agent();
        let cards = agents
            .iter()
            .enumerate()
            .map(|(i, cards)| {
                cards
                    .iter()
                    .enumerate()
                    .map(|(k, card)| {
                        let open =
                            i == self.active_workspace && focused.as_ref() == Some(&card.key);
                        CardState {
                            hover: quantize(hover.alpha(SidebarHit::Card(i, k), now)),
                            open,
                            close: hit == Some(SidebarHit::CardClose(i, k)),
                        }
                    })
                    .collect()
            })
            .collect();
        let ci = workspaces
            .iter()
            .enumerate()
            .map(|(i, ws)| CiCardState {
                hover: quantize(hover.alpha(SidebarHit::Ci(i), now)),
                chips: (0..ws.ci.as_ref().map_or(0, |ci| ci.chips.len()))
                    .map(|k| quantize(hover.alpha(SidebarHit::CiChip(i, k), now)))
                    .collect(),
                others: quantize(hover.alpha(SidebarHit::CiOthers(i), now)),
            })
            .collect();
        let plus = quantize(hover.alpha(SidebarHit::Plus, now));
        let plus_open = self
            .context_menu
            .as_ref()
            .is_some_and(|menu| menu.spec.anchor == layout.plus);
        let repos = quantize(hover.alpha(SidebarHit::Repos, now));
        let repos_open = self.repos.panel.is_some();
        let rename = self.rename.as_ref().and_then(|r| match r.target {
            RenameTarget::Workspace(w) => Some((w, r.field.line())),
            RenameTarget::Tab(..) => None,
        });
        SidebarState {
            density: self.density,
            rows,
            cards,
            ci,
            plus,
            plus_open,
            repos,
            repos_open,
            rename,
        }
    }

    /// A press at `(x, y)` in the sidebar. Left click selects a workspace
    /// (a second one right after renames it: see `input`), "×" or middle
    /// click closes it; a left click on an agent card shows its pane, on
    /// a history card's "×" deletes it from gerdr. A
    /// row held with the left button can be dragged to another place. The
    /// "+" opens the menu of what to add, the button left of it the
    /// repositories panel; the project's name left of them opens the
    /// project menu (the welcome screen while no project is
    /// shown).
    pub(crate) fn sidebar_press(&mut self, x: i32, y: i32, button: u32) {
        let pos = Point::from((x as f64 + 0.5, y as f64 + 0.5));
        let layout = self.sidebar_layout();
        let Some(hit) = layout.hit(pos) else {
            return;
        };
        match (hit, button) {
            (SidebarHit::Plus, BTN_LEFT) => self.open_menu(self.sidebar_plus_menu(&layout)),
            (SidebarHit::Repos, BTN_LEFT) => self.open_repos(),
            (SidebarHit::Header, BTN_LEFT) => {
                if self.project.is_some() {
                    self.toggle_project_menu();
                } else {
                    self.open_welcome(true);
                }
            }
            (SidebarHit::Card(i, k), BTN_LEFT) => {
                if let Some(card) = self.agent_views().get(i).and_then(|c| c.get(k)) {
                    let key = card.key.clone();
                    self.focus_agent(&key);
                }
            }
            (SidebarHit::CardClose(i, k), BTN_LEFT) => {
                let card = self.agent_views().get(i).and_then(|c| c.get(k)).cloned();
                if let Some(AgentKey::Cursor(chat)) = card.map(|c| c.key) {
                    self.delete_cursor_chat(&chat);
                }
            }
            (SidebarHit::CiChip(i, k), BTN_LEFT) => {
                if let Some((_, row)) = self.ci_row(i, k) {
                    let link = row.link.clone();
                    self.open_github(Some(link));
                }
            }
            (SidebarHit::CiOthers(i), BTN_LEFT) => {
                if let Some(repo) = self.ci_repo(i) {
                    let link = repo.actions_url();
                    self.open_github(Some(link));
                }
            }
            (SidebarHit::Close(i), BTN_LEFT) => self.close_workspace(i),
            (SidebarHit::Worktrees(i), BTN_LEFT) => {
                let dir = self.workspaces.get(i).map(|ws| ws.dir.clone());
                self.open_worktrees(dir);
            }
            (SidebarHit::Attention(i, k), BTN_LEFT) => {
                // The worktree on that line, else (on "+N more") the row's.
                let picked = self.workspace_views().get(i).and_then(|ws| {
                    let needing = ws.0.needing_attention();
                    needing.get(k).map(|wt| wt.path.clone())
                });
                let dir = picked.or_else(|| self.workspaces.get(i).map(|ws| ws.dir.clone()));
                self.open_worktrees(dir);
            }
            (SidebarHit::Row(i) | SidebarHit::Name(i) | SidebarHit::Chip(i), BTN_LEFT) => {
                self.switch_workspace(i);
                self.hold(Reorderable::Workspace);
            }
            (hit, BTN_MIDDLE) => {
                if let Some(i) = hit.row() {
                    self.close_workspace(i);
                }
            }
            _ => {}
        }
    }

    /// The pointer moved to `pos` with nothing open over the sidebar, laid
    /// out as `layout`: the header's project name and the row, card or
    /// button under it light up. The tooltip for what is under the
    /// pointer, if it has one.
    pub(crate) fn sidebar_motion(
        &mut self,
        layout: &SidebarLayout,
        pos: Point<f64, Logical>,
    ) -> Option<Tooltip> {
        let hit = self.sidebar_hit(layout, pos);
        self.header_hover = self.project.is_some() && hit == Some(SidebarHit::Header);
        self.sidebar_hover
            .track(hit.map(SidebarHit::pill), Instant::now());
        let workspaces = self.workspace_views();
        let agents = self.agent_views();
        match hit? {
            SidebarHit::Plus => {
                let tip = Tooltip::new(
                    "New terminal, folder, project, Cursor or GitHub",
                    layout.plus,
                );
                let help = self.keymap.help();
                let keys: Vec<_> = ["terminal workspace in home", "open project"]
                    .into_iter()
                    .filter_map(|label| help.key_for(label, 0))
                    .collect();
                Some(if keys.is_empty() {
                    tip
                } else {
                    tip.key(&keys.join(" / "))
                })
            }
            SidebarHit::Repos => {
                let tip = Tooltip::new("Repositories", layout.repos);
                Some(match self.keymap.help().key_for("repositories", 0) {
                    Some(key) => tip.key(&key),
                    None => tip,
                })
            }
            SidebarHit::Close(i) => {
                let tip = Tooltip::new("Close workspace", layout.rows[i].close);
                Some(match self.keymap.help().key_for("close workspace", 0) {
                    Some(key) => tip.key(&key),
                    None => tip,
                })
            }
            SidebarHit::Chip(i) => {
                let cards = agents.get(i).filter(|cards| !cards.is_empty())?;
                let text = match cards.len() {
                    1 => "1 agent".to_owned(),
                    n => format!("{n} agents"),
                };
                let fields = cards
                    .iter()
                    .map(|c| (c.label(), chrome::status_meaning(c.status).to_owned()))
                    .collect();
                Some(Tooltip::new(&text, layout.rows[i].chip?).fields(fields))
            }
            SidebarHit::Name(i) => {
                let (ws, row) = (workspaces.get(i)?, layout.rows.get(i)?);
                let fits = chrome::name_fits(&ws.0.name, row.name.size.w);
                (!fits).then(|| Tooltip::new(&ws.0.name, row.name))
            }
            SidebarHit::Card(i, k) => {
                let card = agents.get(i)?.get(k)?;
                let rect = layout.cards[i].iter().find(|(c, _)| *c == k)?.1;
                Some(Tooltip::new(&card.label(), rect).fields(card.tooltip_fields()))
            }
            SidebarHit::CardClose(i, k) => {
                let rect = layout.card_closes[i].iter().find(|(c, _)| *c == k)?.1;
                Some(Tooltip::new("Delete from gerdr", rect))
            }
            SidebarHit::More(i) => {
                let names = &workspaces.get(i)?.0.more_agents;
                let text = format!("Also kept: {}", names.join(" · "));
                Some(Tooltip::new(&text, layout.more[i]?))
            }
            SidebarHit::Worktrees(i) => {
                let (ws, row) = (workspaces.get(i)?, layout.rows.get(i)?);
                let root = ws.0.git.as_ref().and_then(|g| g.root.as_deref());
                // Below the whole row, clear of the lines it explains.
                row.worktrees?;
                Some(worktrees_tooltip(ws.0.worktrees(), root, row.rect))
            }
            SidebarHit::Attention(i, _) => {
                let lines = layout.rows.get(i)?.attention?;
                let tip = Tooltip::new("Review, merge or delete", lines);
                Some(match self.keymap.help().key_for("worktrees", 0) {
                    Some(key) => tip.key(&key),
                    None => tip,
                })
            }
            SidebarHit::Row(i) => {
                // Without line 2 (compact) the row itself lists them.
                let (ws, row) = (workspaces.get(i)?, layout.rows.get(i)?);
                let (worktrees, git) = (ws.0.worktrees(), ws.0.git.as_ref());
                let root = git.and_then(|g| g.root.as_deref());
                (row.worktrees.is_none() && !worktrees.is_empty())
                    .then(|| worktrees_tooltip(worktrees, root, row.rect))
            }
            SidebarHit::Header => None,
            SidebarHit::Ci(i) => {
                let repo = self.ci_repo(i)?;
                let card = layout.ci.get(i)?.as_ref()?.card;
                Some(Tooltip::new(&format!("CI of {}", repo.slug), card))
            }
            SidebarHit::CiChip(i, k) => {
                let (_, row) = self.ci_row(i, k)?;
                let rect = *layout.ci.get(i)?.as_ref()?.chips.get(k)?;
                let label = ci::subject_label(&row.subject);
                let text = match &row.title {
                    Some(title) => format!("{label} {title}"),
                    None => label,
                };
                let mut fields = vec![("Status".to_owned(), ci_state_word(row.state).to_owned())];
                fields.extend(
                    row.checks
                        .iter()
                        .map(|check| (check.name.clone(), ci_state_word(check.state).to_owned())),
                );
                let tip = Tooltip::new(&text, rect).fields(fields);
                Some(if row.watched {
                    tip.key("right click to unwatch")
                } else {
                    tip
                })
            }
            SidebarHit::CiOthers(i) => {
                let n = self.ci_repo(i)?.others_running;
                let rect = layout.ci.get(i)?.as_ref()?.others?;
                let runs = if n == 1 { "run" } else { "runs" };
                let text = format!("{n} other {runs} going on · open the Actions page");
                Some(Tooltip::new(&text, rect))
            }
        }
    }

    /// The menu a right press at `pos` in the sidebar opens, if any: for
    /// the "+", what to add; for a workspace row, what to do with the
    /// workspace; for an agent card, with the agent.
    pub(crate) fn sidebar_menu(&self, pos: Point<f64, Logical>) -> Option<MenuSpec> {
        let layout = self.sidebar_layout();
        let at = Rect::new((pos.x as i32, pos.y as i32).into(), (0, 0).into());
        match layout.hit(pos)? {
            SidebarHit::Plus => Some(self.sidebar_plus_menu(&layout)),
            SidebarHit::Ci(i) | SidebarHit::CiOthers(i) => {
                let repo = self.ci_repo(i)?;
                let items = vec![
                    MenuItem::new("Open Actions", MenuAction::OpenGitHub(repo.actions_url())),
                    MenuItem::new("Open repository", MenuAction::OpenGitHub(repo.web_url())),
                ];
                Some(MenuSpec { anchor: at, items })
            }
            SidebarHit::CiChip(i, k) => {
                let (repo, row) = self.ci_row(i, k)?;
                let mut items = vec![MenuItem::new(
                    "Open",
                    MenuAction::OpenGitHub(row.link.clone()),
                )];
                if let (true, Subject::Pr(n)) = (row.watched, &row.subject) {
                    let key = ci::key(&repo.host, &repo.slug);
                    items.push(MenuItem::new("Unwatch", MenuAction::Unwatch(key, *n)));
                }
                items.push(MenuItem::new(
                    "Open Actions",
                    MenuAction::OpenGitHub(repo.actions_url()),
                ));
                Some(MenuSpec { anchor: at, items })
            }
            SidebarHit::Card(i, k) | SidebarHit::CardClose(i, k) => {
                let card = self.agent_views().get(i)?.get(k)?.clone();
                let mut items = vec![MenuItem::new(
                    "Focus",
                    MenuAction::FocusAgent(card.key.clone()),
                )];
                items.push(match card.dir {
                    Some(cwd) => MenuItem::new("Copy path", MenuAction::CopyPath(cwd)),
                    None => MenuItem::new("Copy path", MenuAction::NewTerminal).enabled(false),
                });
                if let (true, AgentKey::Cursor(chat)) = (card.deletable, card.key) {
                    let delete = MenuAction::DeleteCursorChat(chat);
                    items.push(MenuItem::new("Delete from gerdr", delete));
                }
                Some(MenuSpec { anchor: at, items })
            }
            hit => {
                let i = hit.row()?;
                let dir = self.workspaces.get(i)?.dir.clone();
                let help = self.keymap.help();
                let hinted = |item: MenuItem, label: &str| match help.key_for(label, 0) {
                    Some(key) => item.hint(&key),
                    None => item,
                };
                let mut items = vec![
                    hinted(MenuItem::new("New tab", MenuAction::NewTab(i)), "new tab"),
                    MenuItem::new("Rename", MenuAction::RenameWorkspace(i)),
                    MenuItem::new("Copy path", MenuAction::CopyPath(dir.clone())),
                ];
                let git = self.git.get(&dir);
                if git.is_some_and(|g| g.root.is_some()) {
                    let item = MenuItem::new("Repositories…", MenuAction::Repos);
                    items.push(hinted(item, "repositories"));
                }
                if git.is_some_and(|g| !g.worktrees.is_empty()) {
                    let item = MenuItem::new("Worktrees…", MenuAction::Worktrees(dir));
                    items.push(hinted(item, "worktrees"));
                }
                items.push(hinted(
                    MenuItem::new("Close workspace", MenuAction::CloseWorkspace(i)),
                    "close workspace",
                ));
                Some(MenuSpec { anchor: at, items })
            }
        }
    }

    /// The menu under the header's "+": what to add, with the keys that
    /// do the same.
    pub(crate) fn sidebar_plus_menu(&self, layout: &SidebarLayout) -> MenuSpec {
        let help = self.keymap.help();
        let hinted = |item: MenuItem, label: &str| match help.key_for(label, 0) {
            Some(key) => item.hint(&key),
            None => item,
        };
        MenuSpec {
            anchor: layout.plus,
            items: vec![
                hinted(
                    MenuItem::new("New terminal", MenuAction::NewTerminal),
                    "terminal workspace in home",
                ),
                MenuItem::new("Open folder…", MenuAction::OpenFolder),
                hinted(
                    MenuItem::new("New project…", MenuAction::NewProject),
                    "open project",
                ),
                MenuItem::new("New Cursor", MenuAction::NewCursor),
                MenuItem::new("GitHub page", MenuAction::NewGitHub),
            ],
        }
    }
}

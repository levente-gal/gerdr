//! Input to the workspace sidebar: presses on its header, the workspace
//! rows and the agent cards, the pointer moving over it (hover, tooltips),
//! its context menus, and what moves in it (see `chrome::sidebar` for what
//! it shows).

use std::{path::Path, time::Instant};

use smithay::utils::{Logical, Point};

use crate::{
    Gerdr, Reorderable,
    anim::quantize,
    chrome::{
        self, AgentView, CardState, Rect, RowState, SidebarHit, SidebarLayout, SidebarState,
        TooltipRow, WorkspaceView,
    },
    git::Worktree,
    home_dir,
    input::{BTN_LEFT, BTN_MIDDLE},
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
        let cards = agents
            .iter()
            .enumerate()
            .map(|(i, cards)| {
                (0..cards.len())
                    .map(|k| CardState {
                        hover: quantize(hover.alpha(SidebarHit::Card(i, k), now)),
                    })
                    .collect()
            })
            .collect();
        let plus = quantize(hover.alpha(SidebarHit::Plus, now));
        let plus_open = self
            .context_menu
            .as_ref()
            .is_some_and(|menu| menu.spec.anchor == layout.plus);
        let rename = self.rename.as_ref().and_then(|r| match r.target {
            RenameTarget::Workspace(w) => Some((w, r.field.line())),
            RenameTarget::Tab(..) => None,
        });
        SidebarState {
            density: self.density,
            rows,
            cards,
            plus,
            plus_open,
            rename,
        }
    }

    /// A press at `(x, y)` in the sidebar. Left click selects a workspace
    /// (a second one right after renames it: see `input`), "×" or middle
    /// click closes it; a left click on an agent card shows its pane. A
    /// row held with the left button can be dragged to another place. The
    /// "+" opens the menu of what to add; the project's name left of it
    /// opens the project menu (the welcome screen while no project is
    /// shown).
    pub(crate) fn sidebar_press(&mut self, x: i32, y: i32, button: u32) {
        let pos = Point::from((x as f64 + 0.5, y as f64 + 0.5));
        let layout = self.sidebar_layout();
        let Some(hit) = layout.hit(pos) else {
            return;
        };
        match (hit, button) {
            (SidebarHit::Plus, BTN_LEFT) => self.open_menu(self.sidebar_plus_menu(&layout)),
            (SidebarHit::Header, BTN_LEFT) => {
                if self.project.is_some() {
                    self.toggle_project_menu();
                } else {
                    self.open_welcome(true);
                }
            }
            (SidebarHit::Card(i, k), BTN_LEFT) => {
                if let Some(card) = self.agent_views().get(i).and_then(|c| c.get(k)) {
                    let pane_id = card.pane_id.clone();
                    self.focus_agent(&pane_id);
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
                let tip = Tooltip::new("New terminal, folder, project or Cursor", layout.plus);
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
            SidebarHit::Close(i) => {
                let tip = Tooltip::new("Close workspace", layout.rows[i].close);
                Some(match self.keymap.help().key_for("close workspace", 0) {
                    Some(key) => tip.key(&key),
                    None => tip,
                })
            }
            SidebarHit::Chip(i) => {
                let text = agents
                    .get(i)
                    .map(|cards| {
                        cards
                            .iter()
                            .map(|c| format!("{} {}", c.label(), chrome::status_label(c.status)))
                            .collect::<Vec<_>>()
                            .join(" · ")
                    })
                    .filter(|t| !t.is_empty())?;
                Some(Tooltip::new(&text, layout.rows[i].chip?))
            }
            SidebarHit::Name(i) => {
                let (ws, row) = (workspaces.get(i)?, layout.rows.get(i)?);
                let fits = chrome::name_fits(&ws.0.name, row.name.size.w);
                (!fits).then(|| Tooltip::new(&ws.0.name, row.name))
            }
            SidebarHit::Card(i, k) => {
                let card = agents.get(i)?.get(k)?;
                let rect = layout.cards[i].iter().find(|(c, _)| *c == k)?.1;
                Some(Tooltip::new(&card.summary(), rect))
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
            SidebarHit::Card(i, k) => {
                let card = self.agent_views().get(i)?.get(k)?.clone();
                let cwd = self
                    .agents
                    .iter()
                    .find(|a| a.pane_id == card.pane_id)
                    .and_then(|a| a.cwd.clone());
                let mut items = vec![MenuItem::new("Focus", MenuAction::FocusAgent(card.pane_id))];
                items.push(match cwd {
                    Some(cwd) => MenuItem::new("Copy path", MenuAction::CopyPath(cwd)),
                    None => MenuItem::new("Copy path", MenuAction::NewTerminal).enabled(false),
                });
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
            ],
        }
    }
}

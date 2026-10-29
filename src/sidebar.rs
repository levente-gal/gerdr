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
        self, AgentView, BAR_HEIGHT, BarStatus, CardState, CiCardState, Rect, RowState, SidebarHit,
        SidebarLayout, SidebarState, TooltipRow, WorkspaceView,
    },
    ci::{self, Subject},
    ci_watch::ci_state_word,
    git::Worktree,
    home_dir,
    inbox::Section,
    input::{BTN_LEFT, BTN_MIDDLE},
    launch,
    menu::{MenuAction, MenuItem, MenuSpec},
    paths,
    rename::RenameTarget,
    repo_batches::repos_tip,
    scroll::{Bounds, Motion},
    tooltip::Tooltip,
};

/// How far a notch of the wheel scrolls the sidebar's rows, in logical
/// pixels.
const SIDEBAR_NOTCH: f64 = 48.0;

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

/// The tooltip of workspace `ws`'s header, about `anchor`: its name, then
/// its branch, where its tabs are and what runs in them, each when known.
fn workspace_tooltip(ws: &WorkspaceView, anchor: Rect) -> Tooltip {
    let fields = [
        ("Branch", ws.branch.clone()),
        ("Folder", Some(ws.place.clone())),
        ("Tabs", Some(ws.activity.clone())),
    ]
    .into_iter()
    .filter_map(|(label, value)| Some((label.to_owned(), value.filter(|v| !v.is_empty())?)))
    .collect();
    Tooltip::new(&ws.name, anchor).fields(fields)
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
            .scrolled_over(
                self.sidebar_scroll.pos(),
                self.sidebar_scroll.overscroll(),
                self.output_size().h,
            )
    }

    /// Whether workspace `w` was folded or unfolded by hand: this run, else
    /// as its record keeps it. `None` leaves it to fold while quiet.
    pub(crate) fn workspace_folded(&self, w: usize) -> Option<bool> {
        let ws = self.workspaces.get(w)?;
        ws.folded.or_else(|| {
            let project = self.project.as_deref()?;
            self.projects.record(project, &ws.record)?.folded
        })
    }

    /// Fold workspace `w` in the sidebar if it shows what is under its
    /// header, else unfold it; kept in its record.
    pub(crate) fn toggle_fold(&mut self, w: usize) {
        let Some(folded) = self.workspace_views().get(w).map(|(ws, _)| !ws.folded) else {
            return;
        };
        self.set_fold(w, Some(folded));
    }

    /// Fold workspace `w` by hand (`Some`), or leave it to fold while
    /// quiet (`None`); here and in its record.
    fn set_fold(&mut self, w: usize, folded: Option<bool>) {
        let Some(ws) = self.workspaces.get_mut(w) else {
            return;
        };
        ws.folded = folded;
        let id = ws.record.clone();
        if let Some(project) = self.project.clone()
            && let Some(record) = self.projects.record_mut(&project, &id)
            && record.folded != folded
        {
            record.folded = folded;
            self.projects.save();
        }
    }

    /// Move the sidebar's rows as `motion` has it, under the header.
    pub(crate) fn sidebar_scroll(&mut self, motion: Motion) {
        let bounds = self.sidebar_bounds(&self.sidebar_layout());
        self.sidebar_scroll.apply(motion, Instant::now(), bounds);
    }

    /// Move the sidebar's rows on to `now` if they move on their own;
    /// whether they still move (see `Scroll::moving`).
    pub fn sidebar_scroll_tick(&mut self, now: Instant) -> bool {
        let scroll = &self.sidebar_scroll;
        if !scroll.moving(now) && scroll.overscroll() == 0.0 {
            return false;
        }
        let bounds = self.sidebar_bounds(&self.sidebar_layout());
        self.sidebar_scroll.tick(now, bounds)
    }

    /// What the sidebar's rows scroll within, laid out as `layout`: by
    /// pixels, under the header.
    fn sidebar_bounds(&self, layout: &SidebarLayout) -> Bounds {
        let height = self.output_size().h;
        Bounds {
            max: layout.max_scroll(height) as f64,
            notch: SIDEBAR_NOTCH,
            unit: 1.0,
            view: (height - BAR_HEIGHT).max(0) as f64,
        }
    }

    /// Whether something covers the sidebar, taking the pointer from it.
    fn sidebar_covered(&self) -> bool {
        self.quit.is_some()
            || self.restart.is_some()
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
        // Rows passing under a still pointer don't light up.
        let moving = self.sidebar_scroll.moving(Instant::now());
        layout.hit(pos).filter(|hit| !(moving && hit.scrolls()))
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
        let height = self.output_size().h;
        let layout =
            chrome::sidebar_layout_at(workspaces, agents, self.sidebar_width, self.density);
        // Held to what the rows now take, and moved to show the active
        // workspace when another becomes it: gliding there, or at once
        // the first time.
        let bounds = self.sidebar_bounds(&layout);
        self.sidebar_scroll.tick(now, bounds);
        let shown = self.sidebar_shown.replace(self.active_workspace);
        if shown != Some(self.active_workspace) {
            let scroll = self.sidebar_scroll.pos().round() as i32;
            let to = layout.showing(self.active_workspace, scroll, height);
            match shown {
                _ if to == scroll => {}
                Some(_) => self.sidebar_scroll.glide_to(to as f64, now, bounds),
                None => self.sidebar_scroll.jump(to as f64),
            }
        }
        let (scroll, over) = (self.sidebar_scroll.pos(), self.sidebar_scroll.overscroll());
        let layout = layout.scrolled_over(scroll, over, height);
        let hit = self.sidebar_hit(&layout, self.pointer);
        self.sidebar_hover.track(hit.map(SidebarHit::pill), now);
        let hover = &self.sidebar_hover;
        let rows = (0..layout.rows.len())
            .map(|i| RowState {
                hover: quantize(hover.alpha(SidebarHit::Row(i), now)),
                close: hit == Some(SidebarHit::Close(i)),
                restart: hit == Some(SidebarHit::Restart(i)),
                worktrees: hit == Some(SidebarHit::Worktrees(i)),
                fold: hit == Some(SidebarHit::Fold(i)),
                ci: hit == Some(SidebarHit::CiMark(i)),
            })
            .collect();
        let inbox = layout
            .inbox
            .iter()
            .map(|(i, k, _)| quantize(hover.alpha(SidebarHit::Inbox(*i, *k), now)))
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
                            subagents: std::array::from_fn(|j| {
                                quantize(hover.alpha(SidebarHit::Subagent(i, k, j), now))
                            }),
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
                others: quantize(hover.alpha(SidebarHit::CiPrs(i), now)),
            })
            .collect();
        let plus = quantize(hover.alpha(SidebarHit::Plus, now));
        let plus_open = self
            .context_menu
            .as_ref()
            .is_some_and(|menu| menu.spec.anchor == layout.plus);
        let repos = quantize(hover.alpha(SidebarHit::Repos, now));
        let repos_open = self.repos.panel.is_some();
        let (repos_changed, repos_stuck) = self.repos_changed();
        let rename = self.rename.as_ref().and_then(|r| match r.target {
            RenameTarget::Workspace(w) => Some((w, r.field.line())),
            RenameTarget::Tab(..) => None,
        });
        SidebarState {
            density: self.density,
            rows,
            cards,
            ci,
            inbox,
            plus,
            plus_open,
            repos,
            repos_open,
            repos_changed,
            repos_stuck: repos_stuck > 0,
            rename,
            scroll,
            over,
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
            (SidebarHit::InboxTitle, BTN_LEFT) => self.open_inbox(Some(Section::Agents)),
            (SidebarHit::Card(i, k) | SidebarHit::Inbox(i, k), BTN_LEFT) => {
                // An agent shown from the "Needs you" list shows in its
                // workspace too: a fold by hand gives way, and the
                // workspace folds again once quiet.
                if matches!(hit, SidebarHit::Inbox(..)) && self.workspace_folded(i) == Some(true) {
                    self.set_fold(i, None);
                }
                if let Some(card) = self.agent_views().get(i).and_then(|c| c.get(k)) {
                    let key = card.key.clone();
                    self.focus_agent(&key);
                }
            }
            (SidebarHit::Subagent(i, k, j), BTN_LEFT) => {
                let card = self.agent_views().get(i).and_then(|c| c.get(k)).cloned();
                let sub = card.and_then(|c| c.subagents.get(j)?.key.clone());
                if let Some(key) = sub {
                    self.focus_agent(&key);
                }
            }
            (SidebarHit::CardClose(i, k), BTN_LEFT) => {
                let card = self.agent_views().get(i).and_then(|c| c.get(k)).cloned();
                match card.map(|c| c.key) {
                    Some(AgentKey::Cursor(chat)) => self.delete_cursor_chat(&chat),
                    Some(AgentKey::Test(pane_id)) => self.dismiss_test(&pane_id),
                    _ => {}
                }
            }
            (SidebarHit::Ci(i) | SidebarHit::CiMark(i), BTN_LEFT) => self.focus_repo_tab(i),
            (SidebarHit::Fold(i), BTN_LEFT) => self.toggle_fold(i),
            (SidebarHit::CiChip(i, k), BTN_LEFT) => {
                if let Some((_, row)) = self.ci_row(i, k) {
                    let link = row.link.clone();
                    self.open_page(i, link);
                }
            }
            (SidebarHit::CiPrs(i), BTN_LEFT) => {
                if let Some(repo) = self.ci_repo(i) {
                    let link = repo.pulls_url();
                    self.open_page(i, link);
                }
            }
            (SidebarHit::Close(i), BTN_LEFT) => self.close_workspace(i),
            (SidebarHit::Restart(i), BTN_LEFT) => self.restart_workspace(i),
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
                let (changed, stuck) = self.repos_changed();
                let tip = Tooltip::new(&repos_tip(changed, stuck), layout.repos);
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
            SidebarHit::Restart(i) => {
                let rect = layout.rows.get(i)?.restart?;
                // Each command, where it runs, whether it has exited, and
                // whether the user chose to leave it alone.
                let home = home_dir();
                let choices = self.restart_choices(i);
                let fields: Vec<_> = self
                    .workspace_restarts(i)
                    .into_iter()
                    .map(|(pane_id, c)| {
                        let mut place = c.cwd.as_deref().map_or(String::new(), |dir| {
                            paths::display(dir, home.as_deref(), 40)
                        });
                        if self.pane_bar_content(&pane_id).status == BarStatus::Exited {
                            place = format!("exited · {place}");
                        }
                        if choices
                            .iter()
                            .any(|ch| ch.command == c.command && !ch.restart)
                        {
                            place = format!("skipped · {place}");
                        }
                        (chrome::command_line(&c.command).unwrap_or(c.command), place)
                    })
                    .collect();
                let text = match fields.len() {
                    1 => "Restart its command".to_owned(),
                    n => format!("Restart its {n} commands"),
                };
                let tip = Tooltip::new(&text, rect).fields(fields);
                Some(tip.key("right click to choose which"))
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
                Some(workspace_tooltip(&ws.0, row.title))
            }
            SidebarHit::Fold(i) => {
                let (ws, row) = (workspaces.get(i)?, layout.rows.get(i)?);
                let text = if ws.0.folded { "Unfold" } else { "Fold" };
                Some(Tooltip::new(text, row.fold?))
            }
            SidebarHit::CiMark(i) => {
                let repo = self.ci_repo(i)?;
                let (ws, row) = (workspaces.get(i)?, layout.rows.get(i)?);
                let ci = ws.0.ci.as_ref()?;
                let text = format!("GitHub · {}", repo.slug);
                let mut fields: Vec<_> = ci
                    .chips
                    .iter()
                    .map(|chip| {
                        let label = format!("{} {}", chrome::group_label(chip.group), chip.label);
                        (label, ci_state_word(chip.state).to_owned())
                    })
                    .collect();
                if ci.other_prs > 0 {
                    fields.push(("Other PRs".into(), ci.other_prs.to_string()));
                }
                let tip = Tooltip::new(&text, row.ci?).fields(fields);
                Some(tip.key("click to show its page"))
            }
            SidebarHit::InboxTitle => {
                let tip = Tooltip::new("Open the inbox", layout.inbox_title?);
                Some(match self.keymap.help().key_for("inbox", 0) {
                    Some(key) => tip.key(&key),
                    None => tip,
                })
            }
            SidebarHit::Inbox(i, k) => {
                let card = agents.get(i)?.get(k)?;
                let rect = layout.inbox.iter().find(|e| (e.0, e.1) == (i, k))?.2;
                let ws = &workspaces.get(i)?.0.name;
                let mut fields = vec![("Workspace".to_owned(), ws.clone())];
                fields.extend(card.tooltip_fields());
                Some(Tooltip::new(&card.label(), rect).fields(fields))
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
            SidebarHit::Subagent(i, k, j) => {
                let task = &agents.get(i)?.get(k)?.subagents.get(j)?.task;
                let rect = layout.subagents[i].iter().find(|l| (l.0, l.1) == (k, j))?.2;
                Some(Tooltip::new(task, rect))
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
                let (ws, row) = (workspaces.get(i)?, layout.rows.get(i)?);
                Some(workspace_tooltip(&ws.0, row.title))
            }
            SidebarHit::Header => None,
            SidebarHit::Ci(i) => {
                let repo = self.ci_repo(i)?;
                let card = layout.ci.get(i)?.as_ref()?.card;
                let text = format!("GitHub · {}", repo.slug);
                Some(Tooltip::new(&text, card).key("click to show its page"))
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
                Some(match (row.group, row.watched) {
                    (_, true) => tip.key("right click to unwatch"),
                    (ci::Group::Review, _) => tip.key("asks for your review, or assigned to you"),
                    _ => tip,
                })
            }
            SidebarHit::CiPrs(i) => {
                let n = self.ci_repo(i)?.other_prs;
                let rect = layout.ci.get(i)?.as_ref()?.others?;
                let prs = if n == 1 {
                    "pull request"
                } else {
                    "pull requests"
                };
                let text = format!("{n} other open {prs} · open the Pull requests page");
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
            SidebarHit::Ci(i) | SidebarHit::CiPrs(i) | SidebarHit::CiMark(i) => {
                let repo = self.ci_repo(i)?;
                let (pulls, web) = (repo.pulls_url(), repo.web_url());
                let items = vec![
                    MenuItem::new("Open pull requests", MenuAction::OpenPage(i, pulls.clone())),
                    MenuItem::new(
                        "Open pull requests in new tab",
                        MenuAction::OpenPageInNewTab(i, pulls),
                    ),
                    MenuItem::new("Open repository", MenuAction::OpenPage(i, web.clone())),
                    MenuItem::new(
                        "Open repository in new tab",
                        MenuAction::OpenPageInNewTab(i, web),
                    ),
                ];
                Some(MenuSpec { anchor: at, items })
            }
            SidebarHit::CiChip(i, k) => {
                let (repo, row) = self.ci_row(i, k)?;
                let mut items = vec![
                    MenuItem::new("Open", MenuAction::OpenPage(i, row.link.clone())),
                    MenuItem::new(
                        "Open in new tab",
                        MenuAction::OpenPageInNewTab(i, row.link.clone()),
                    ),
                ];
                if let (true, Subject::Pr(n)) = (row.watched, &row.subject) {
                    let key = ci::key(&repo.host, &repo.slug);
                    items.push(MenuItem::new("Unwatch", MenuAction::Unwatch(key, *n)));
                }
                Some(MenuSpec { anchor: at, items })
            }
            SidebarHit::Card(i, k)
            | SidebarHit::CardClose(i, k)
            | SidebarHit::Subagent(i, k, _)
            | SidebarHit::Inbox(i, k) => {
                let card = self.agent_views().get(i)?.get(k)?.clone();
                let mut items = vec![MenuItem::new(
                    "Focus",
                    MenuAction::FocusAgent(card.key.clone()),
                )];
                items.push(match card.dir {
                    Some(cwd) => MenuItem::new("Copy path", MenuAction::CopyPath(cwd)),
                    None => MenuItem::new("Copy path", MenuAction::NewTerminal).enabled(false),
                });
                match (card.deletable, card.key) {
                    (true, AgentKey::Cursor(chat)) => {
                        let delete = MenuAction::DeleteCursorChat(chat);
                        items.push(MenuItem::new("Delete from gerdr", delete));
                    }
                    (true, AgentKey::Test(pane_id)) => {
                        let dismiss = MenuAction::DismissTest(pane_id);
                        items.push(MenuItem::new("Dismiss", dismiss));
                    }
                    _ => {}
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
                let restartable = !self.workspace_restarts(i).is_empty();
                let restart = MenuItem::new("Restart commands", MenuAction::RestartWorkspace(i));
                items.push(restart.enabled(restartable));
                let choose =
                    MenuItem::new("Choose commands to restart…", MenuAction::ChooseRestarts(i));
                items.push(choose.enabled(restartable));
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
                hinted(MenuItem::new("Inbox…", MenuAction::Inbox), "inbox"),
            ],
        }
    }
}

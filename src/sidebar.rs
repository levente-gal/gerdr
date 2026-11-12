//! Input to the workspace sidebar: presses on its header, the workspace
//! rows and the agent cards, the pointer moving over it (hover, tooltips),
//! its context menus, and what moves in it (see `chrome::sidebar` for what
//! it shows).

use std::{
    path::Path,
    time::{Duration, Instant},
};

use smithay::{
    reexports::calloop::timer::{TimeoutAction, Timer},
    utils::{Logical, Point},
};

use crate::{
    Action, Gerdr, Reorderable, WorkspaceKind,
    agents::AgentKey,
    anim::quantize,
    chrome::{
        self, AgentView, BAR_HEIGHT, BarStatus, CardState, CiCardState, Rect, RowState, SidebarHit,
        SidebarLayout, SidebarState, TooltipRow, WorkspaceView,
    },
    ci::{self, Subject},
    ci_watch::ci_state_word,
    cli_agents::CliAgent,
    feature::Stage,
    git::{Change, GitStatus, Worktree},
    home_dir,
    inbox::Section,
    input::{BTN_LEFT, BTN_MIDDLE},
    launch,
    menu::{MenuAction, MenuItem, MenuSpec},
    paths,
    projects::VenvSetting,
    rename::RenameTarget,
    repo_batches::repos_tip,
    scroll::{Bounds, Motion},
    tooltip::Tooltip,
};

/// How far a notch of the wheel scrolls the sidebar's rows, in logical
/// pixels.
const SIDEBAR_NOTCH: f64 = 48.0;

/// How long the pointer rests on a header's AI button before the strip
/// of agents opens under it, so passing over the button opens nothing.
const STRIP_DELAY: Duration = Duration::from_millis(300);

/// Which workspace's strip of agents shows with the pointer on `hit` at
/// `now`, and which waits to open and until when, after `open` showed
/// and `due` waited: on the AI button it opens once it rested there
/// `STRIP_DELAY`; on the strip, or on the way to it from the button
/// (`bridge`, see `RowLayout::strip_bridge`), an open one stays;
/// elsewhere none does. `hit` is `None` while something covers the
/// sidebar, so nothing opens under it.
fn strip_after(
    hit: Option<SidebarHit>,
    bridge: Option<usize>,
    open: Option<usize>,
    due: Option<(usize, Instant)>,
    now: Instant,
) -> (Option<usize>, Option<(usize, Instant)>) {
    if open.is_some() && open == bridge {
        return (open, None);
    }
    match hit {
        Some(SidebarHit::NewAgent(i)) if open == Some(i) => (open, None),
        Some(SidebarHit::NewAgent(i)) => match due.filter(|(w, _)| *w == i) {
            Some((_, at)) if now >= at => (Some(i), None),
            Some(waiting) => (None, Some(waiting)),
            None => (None, Some((i, now + STRIP_DELAY))),
        },
        Some(SidebarHit::PickAgent(i, _)) => (open.filter(|w| *w == i), None),
        _ => (None, None),
    }
}

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

/// What a feature's tooltip says of it: the stage it is at, and how many
/// are done.
fn feature_fields(f: &crate::feature::Feature) -> Vec<(String, String)> {
    let stage = match f.current() {
        Some(stage) => format!("{} \u{b7} {}", stage.label(), stage.about()),
        None => "Every stage done".to_owned(),
    };
    let done = f.marks.len();
    vec![
        ("Stage".to_owned(), stage),
        (
            "Done".to_owned(),
            format!("{done} of {}", crate::feature::Stage::ALL.len()),
        ),
    ]
}

/// The tooltip of stage dot `k` of feature `f`, about `anchor`.
fn stage_tooltip(card: &chrome::FeatureCard, k: usize, anchor: Rect) -> Option<Tooltip> {
    let stage = *Stage::ALL.get(k)?;
    let text = format!("{} \u{b7} {}", stage.label(), card.marks.get(k)?.word());
    Some(Tooltip::new(&text, anchor).fields(vec![("About".to_owned(), stage.about().to_owned())]))
}

/// What a workspace's tooltip says of the venv its new shells activate.
fn venv_word(venv: &VenvSetting) -> String {
    match venv {
        VenvSetting::Auto => "Auto (the one each shell finds)".into(),
        VenvSetting::Off => "Off".into(),
        VenvSetting::Path(path) => path.display().to_string(),
    }
}

/// The tooltip of workspace `ws`'s header, about `anchor`: its name, then
/// its branch, where its tabs are and what runs in them, and the venv its
/// new shells activate (`venv`, for one with shells), each when known.
/// What a workspace's git marks say, in words.
fn git_marks_tip(git: &GitStatus) -> String {
    let mut parts = Vec::new();
    if git.op.is_some() {
        parts.push("stopped half-way".to_owned());
    } else if git.changes.iter().any(Change::conflicted) {
        parts.push("conflicted files".to_owned());
    }
    if git.dirty {
        parts.push("uncommitted changes".to_owned());
    }
    let commits = |n: u32| match n {
        1 => "1 commit".to_owned(),
        n => format!("{n} commits"),
    };
    if git.ahead > 0 {
        parts.push(format!("{} to push", commits(git.ahead)));
    }
    if git.behind > 0 {
        parts.push(format!("{} to pull", commits(git.behind)));
    }
    let text = parts.join(", ");
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => text,
    }
}

fn workspace_tooltip(ws: &WorkspaceView, anchor: Rect, venv: Option<&VenvSetting>) -> Tooltip {
    let fields = [
        ("Branch", ws.branch.clone()),
        ("Folder", Some(ws.place.clone())),
        ("Tabs", Some(ws.activity.clone())),
        ("Python venv", venv.map(venv_word)),
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
        let rows = self.feature_rows();
        chrome::sidebar_layout_at(
            &workspaces,
            &agents,
            self.sidebar_width,
            self.settings.density,
        )
        .with_features(&rows, self.sidebar_width, self.settings.density)
        .with_feature_head(
            !workspaces.is_empty(),
            chrome::features_waiting(&workspaces, &rows),
            self.sidebar_width,
        )
        .scrolled_over(
            self.sidebar_scroll.pos(),
            self.sidebar_scroll.overscroll(),
            self.sidebar_height(),
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

    /// The venv new shells of workspace `w` activate, as its record keeps
    /// it: none for a workspace without shells (Cursor's).
    fn workspace_venv(&self, w: usize) -> Option<&VenvSetting> {
        let ws = self.workspaces.get(w)?;
        if ws.kind == WorkspaceKind::Cursor {
            return None;
        }
        let project = self.project.as_deref()?;
        Some(&self.projects.record(project, &ws.record)?.venv)
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
        if self.keep_fold(w, folded) {
            self.projects.save();
        }
    }

    /// Fold every workspace by hand (`Some`: folded or not), or leave them
    /// all to fold while quiet (`None`); here and in their records, saved
    /// once. The sidebar header's menu offers it.
    pub(crate) fn fold_all(&mut self, folded: Option<bool>) {
        let mut changed = false;
        for w in 0..self.workspaces.len() {
            changed |= self.keep_fold(w, folded);
        }
        if changed {
            self.projects.save();
        }
    }

    /// Fold workspace `w` as `set_fold` does, without saving: whether its
    /// record changed.
    fn keep_fold(&mut self, w: usize, folded: Option<bool>) -> bool {
        let Some(ws) = self.workspaces.get_mut(w) else {
            return false;
        };
        ws.folded = folded;
        let id = ws.record.clone();
        if let Some(project) = self.project.clone()
            && let Some(record) = self.projects.record_mut(&project, &id)
            && record.folded != folded
        {
            record.folded = folded;
            return true;
        }
        false
    }

    /// Move the sidebar's rows as `motion` has it, under the header.
    pub(crate) fn sidebar_scroll(&mut self, motion: Motion) {
        let bounds = self.sidebar_bounds(&self.sidebar_layout());
        self.sidebar_scroll.apply(motion, Instant::now(), bounds);
    }

    /// Stand the sidebar's rows `pos` pixels down, as a scrollbar's thumb
    /// held there puts them.
    pub(crate) fn sidebar_scroll_to(&mut self, pos: f64) {
        let bounds = self.sidebar_bounds(&self.sidebar_layout());
        self.sidebar_scroll
            .jump(pos.clamp(0.0, bounds.max.max(0.0)));
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
        let height = self.sidebar_height();
        Bounds {
            max: layout.max_scroll(height) as f64,
            notch: SIDEBAR_NOTCH,
            unit: 1.0,
            next: 1.0,
            prev: 1.0,
            view: (height - BAR_HEIGHT).max(0) as f64,
        }
    }

    /// Whether something covers the sidebar, taking the pointer from it.
    pub(crate) fn sidebar_covered(&self) -> bool {
        self.overlays.quit.is_some()
            || self.overlays.restart.is_some()
            || self.overlays.checkout.is_some()
            || self.overlays.confirm.is_some()
            || self.overlays.welcome.is_some()
            || self.overlays.project_menu.is_some()
            || self.overlays.switcher.is_some()
            || self.overlays.context_menu.is_some()
    }

    /// What is under the pointer in the sidebar, if it is free and there.
    fn sidebar_hit(&self, layout: &SidebarLayout, pos: Point<f64, Logical>) -> Option<SidebarHit> {
        if self.sidebar_covered() || pos.x >= self.sidebar_width as f64 {
            return None;
        }
        // Nothing lights up while a scrollbar's thumb is held.
        if self.mouse.thumb_drag.is_some() {
            return None;
        }
        // Rows passing under a still pointer don't light up.
        let moving = self.sidebar_scroll.moving(Instant::now());
        layout.hit(pos).filter(|hit| !(moving && hit.scrolls()))
    }

    /// The agent whose pane has the keyboard in the shown tab: herdr's in
    /// it, the T3 Code thread its app shows, or the Cursor chat last shown
    /// from its card in Cursor.
    fn focused_agent(&self) -> Option<AgentKey> {
        let tab = self.tab()?;
        let id = tab.focused_id()?;
        let pane = tab.panes.iter().find(|p| p.id == id)?;
        if let Some(herdr) = &pane.herdr {
            return Some(AgentKey::Herdr(herdr.pane_id.clone()));
        }
        if let Some(thread) = self.shown_t3_thread() {
            return Some(AgentKey::T3(thread));
        }
        let cursor = pane.app_command.as_deref().and_then(launch::program_name);
        let chat = self
            .cursor
            .opened
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
        let height = self.sidebar_height();
        let rows = self.feature_rows();
        let layout = chrome::sidebar_layout_at(
            workspaces,
            agents,
            self.sidebar_width,
            self.settings.density,
        )
        .with_features(&rows, self.sidebar_width, self.settings.density)
        .with_feature_head(
            !workspaces.is_empty(),
            chrome::features_waiting(workspaces, &rows),
            self.sidebar_width,
        );
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
        let hit = self.sidebar_hit(&layout, self.mouse.pos);
        self.hover.sidebar.track(hit.map(SidebarHit::pill), now);
        let hover = &self.hover.sidebar;
        let rows = (0..layout.rows.len())
            .map(|i| RowState {
                hover: quantize(hover.alpha(SidebarHit::Row(i), now)),
                close: hit == Some(SidebarHit::Close(i)),
                restart: hit == Some(SidebarHit::Restart(i)),
                agent: hit == Some(SidebarHit::NewAgent(i)),
                pick: match hit {
                    Some(SidebarHit::PickAgent(w, k)) if w == i => Some(k),
                    _ => None,
                },
                worktrees: hit == Some(SidebarHit::Worktrees(i)),
                fold: hit == Some(SidebarHit::Fold(i)),
                ci: hit == Some(SidebarHit::CiMark(i)),
                git: hit == Some(SidebarHit::GitMarks(i)),
                stage: match hit {
                    Some(SidebarHit::Stage(w, k)) if w == i => Some(k),
                    _ => None,
                },
                by_hand: self.workspace_folded(i).is_some(),
            })
            .collect();
        let inbox = layout
            .inbox
            .iter()
            .map(|(i, k, _)| quantize(hover.alpha(SidebarHit::Inbox(*i, *k), now)))
            .collect();
        let features = (0..layout.features.len())
            .map(|n| quantize(hover.alpha(SidebarHit::Feature(n), now)))
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
        let feature_dot = match hit {
            Some(SidebarHit::FeatureDot(n, k)) => Some((n, k)),
            _ => None,
        };
        let features_plus = quantize(hover.alpha(SidebarHit::NewFeature, now));
        let plus = quantize(hover.alpha(SidebarHit::Plus, now));
        let plus_open = self
            .overlays
            .context_menu
            .as_ref()
            .is_some_and(|menu| menu.spec.anchor == layout.plus);
        let repos = quantize(hover.alpha(SidebarHit::Repos, now));
        let repos_open = self.repos.panel.is_some();
        let counts = self.repos_changed();
        let rename = self.overlays.rename.as_ref().and_then(|r| match r.target {
            RenameTarget::Workspace(w) => Some((w, r.field.line())),
            RenameTarget::Tab(..) => None,
        });
        SidebarState {
            density: self.settings.density,
            rows,
            cards,
            ci,
            inbox,
            features,
            feature_dot,
            features_plus,
            plus,
            plus_open,
            repos,
            repos_open,
            repos_changed: counts.changed,
            repos_stuck: counts.stuck > 0,
            repos_ahead: counts.ahead > 0,
            repos_behind: counts.behind > 0,
            rename,
            scroll,
            over,
        }
    }

    /// Open the strip of agents on workspace `i`'s header once `hit` has
    /// rested `STRIP_DELAY` on its AI button, and keep it open while on
    /// the button, the strip or the way between them (`bridge`); close
    /// it off them. Opening looks for the installed agents again.
    pub(crate) fn track_agent_strip(&mut self, hit: Option<SidebarHit>, bridge: Option<usize>) {
        let now = Instant::now();
        self.apply_agent_strip(strip_after(
            hit,
            bridge,
            self.hover.strip,
            self.hover.strip_due,
            now,
        ));
        let Some((_, at)) = self.hover.strip_due else {
            return;
        };
        if self.hover.strip_timed {
            return;
        }
        // Open it when due, if the pointer still rests on that button:
        // it may have moved off, something may cover the sidebar now (a
        // click on the button opened the agent picker), the workspaces
        // may have moved or closed.
        let show = move |_, _: &mut (), state: &mut Gerdr| {
            let now = Instant::now();
            if let Some((_, at)) = state.hover.strip_due {
                if now < at {
                    return TimeoutAction::ToInstant(at);
                }
                let (hit, bridge) = state.strip_pointer();
                let (open, due) = (state.hover.strip, state.hover.strip_due);
                state.apply_agent_strip(strip_after(hit, bridge, open, due, now));
                if let Some((_, at)) = state.hover.strip_due {
                    return TimeoutAction::ToInstant(at);
                }
            }
            state.hover.strip_timed = false;
            TimeoutAction::Drop
        };
        match self
            .loop_handle
            .insert_source(Timer::from_deadline(at), show)
        {
            Ok(_) => self.hover.strip_timed = true,
            Err(err) => tracing::warn!("timing the strip of agents: {err}"),
        }
    }

    /// What of the sidebar is under the pointer where it is now, as
    /// `track_agent_strip` takes it: nothing while an overlay or the
    /// footer is there, or something covers the sidebar.
    fn strip_pointer(&self) -> (Option<SidebarHit>, Option<usize>) {
        let Some(pointer) = self.seat.get_pointer() else {
            return (None, None);
        };
        let pos = pointer.current_location();
        if self.overlay_at(pos).is_some() || self.on_footer(pos) {
            return (None, None);
        }
        let layout = self.sidebar_layout();
        let hit = self.sidebar_hit(&layout, pos);
        (hit, self.strip_bridge(&layout, pos))
    }

    /// The workspace whose open strip of agents `pos` is on the way to
    /// from its AI button, in the sidebar laid out as `layout`, unless
    /// something covers the sidebar.
    fn strip_bridge(&self, layout: &SidebarLayout, pos: Point<f64, Logical>) -> Option<usize> {
        if self.sidebar_covered() || pos.x >= self.sidebar_width as f64 {
            return None;
        }
        layout.strip_bridge_at(pos)
    }

    /// Show the strip of agents and wait for the next as `strip_after`
    /// has them: the AI button's tooltip, due or shown, steps aside for
    /// a strip opening under it.
    fn apply_agent_strip(&mut self, (open, due): (Option<usize>, Option<(usize, Instant)>)) {
        self.hover.strip_due = due;
        let opens = open.is_some() && open != self.hover.strip;
        self.set_agent_strip(open);
        if opens {
            self.hover.tooltip.offer(None, Instant::now());
        }
    }

    /// Close the strip of agents and stop waiting to open one: on a press
    /// or a key, as the tooltip hides.
    pub(crate) fn drop_agent_strip(&mut self) {
        self.hover.strip = None;
        self.hover.strip_record = None;
        self.hover.strip_due = None;
    }

    /// Show the strip of agents on workspace `open`'s header (none when
    /// `None`), looking for the installed agents again as it opens.
    fn set_agent_strip(&mut self, open: Option<usize>) {
        if open.is_some() && open != self.hover.strip {
            self.refresh_cli_agents();
        }
        self.hover.strip = open;
        self.hover.strip_record = open
            .and_then(|i| self.workspaces.get(i))
            .map(|ws| ws.record.clone());
    }

    /// The agent in slot `k` of workspace `i`'s strip, as `layout` has
    /// it; `None` on the ellipsis standing for those that don't fit.
    fn picked_agent(&self, layout: &SidebarLayout, i: usize, k: usize) -> Option<&CliAgent> {
        let slots = layout.rows.get(i)?.pick_slots();
        if self.launcher.cli_agents.len() > slots && k + 1 == slots {
            return None;
        }
        self.launcher.cli_agents.get(k)
    }

    /// The menu listing the Cursor chats of workspace `w` its rows leave
    /// out (its "+N more" line), at `anchor`: a click shows one.
    fn more_agents_menu(&self, w: usize, anchor: Rect) -> MenuSpec {
        let items = self
            .cursor_history(w)
            .1
            .into_iter()
            .map(|chat| {
                let card = AgentView::cursor(chat);
                MenuItem::new(&card.label(), MenuAction::FocusAgent(card.key))
            })
            .collect();
        MenuSpec { anchor, items }
    }

    /// The menu listing workspace `w`'s agents, the most pressing first,
    /// at `anchor`, from its status chip: a click shows one.
    fn chip_agents_menu(&self, w: usize, anchor: Rect) -> Option<MenuSpec> {
        let cards = self.agent_views().into_iter().nth(w)?;
        let items: Vec<MenuItem> = chrome::card_order(&cards)
            .into_iter()
            .map(|k| {
                let card = &cards[k];
                let label = format!("{} · {}", card.label(), chrome::status_label(card.status));
                MenuItem::new(&label, MenuAction::FocusAgent(card.key.clone()))
            })
            .collect();
        (!items.is_empty()).then_some(MenuSpec { anchor, items })
    }

    /// A press at `(x, y)` in the sidebar. Left click selects a workspace
    /// (a second one right after renames it: see `input`), "×" or middle
    /// click closes it; a left click on an agent card shows its pane, on
    /// its "×" closes it (see `Gerdr::close_agent`). A
    /// row held with the left button can be dragged to another place. The
    /// "+" opens the menu of what to add, the button left of it the
    /// repositories panel; the project's name left of them opens the
    /// project menu (the welcome screen while no project is
    /// shown). The "+N more" line lists the chats it counts, and the
    /// status chip of a header lists its agents (one blocked agent alone
    /// is shown at once).
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
            (SidebarHit::FeaturesTitle, BTN_LEFT) => self.open_features(),
            (SidebarHit::NewFeature, BTN_LEFT) => self.open_new_feature(None),
            (SidebarHit::FeaturesWaiting, BTN_LEFT) => self.open_inbox(Some(Section::Gates)),
            (SidebarHit::Feature(n), BTN_LEFT) => {
                if let Some(f) = self.sidebar_feature(n) {
                    let (project, slug) = (f.project.clone(), f.slug.clone());
                    self.open_linked_feature(&project, &slug);
                }
            }
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
                if let Some(card) = card {
                    self.close_agent(&card.key);
                }
            }
            (SidebarHit::Ci(i) | SidebarHit::CiMark(i), BTN_LEFT) => self.focus_repo_tab(i),
            (SidebarHit::Stage(i, k), BTN_LEFT) => {
                if let (Some(f), Some(stage)) = (self.workspace_feature(i), Stage::ALL.get(k)) {
                    let (project, slug) = (f.project.clone(), f.slug.clone());
                    self.open_feature_at(&project, &slug, *stage);
                }
            }
            (SidebarHit::FeatureDot(n, k), BTN_LEFT) => {
                if let (Some(f), Some(stage)) = (self.sidebar_feature(n), Stage::ALL.get(k)) {
                    let (project, slug) = (f.project.clone(), f.slug.clone());
                    // Another project's feature: entering it opens its page.
                    if self.project.as_deref() == Some(project.as_str()) {
                        self.open_feature_at(&project, &slug, *stage);
                    } else {
                        self.open_linked_feature(&project, &slug);
                    }
                }
            }
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
            (SidebarHit::NewAgent(i), BTN_LEFT) => self.open_agent_picker(i),
            (SidebarHit::PickAgent(i, k), BTN_LEFT) => match self.picked_agent(&layout, i, k) {
                Some(agent) => self.start_agent(i, &agent.clone()),
                None => self.open_agent_picker(i),
            },
            (SidebarHit::Worktrees(i), BTN_LEFT) => {
                let dir = self.workspaces.get(i).map(|ws| ws.dir.clone());
                self.open_worktrees(dir);
            }
            // Its git marks: the repositories panel on its repository.
            (SidebarHit::GitMarks(i), BTN_LEFT) => {
                let root = self
                    .workspace_views()
                    .get(i)
                    .and_then(|ws| ws.0.git.as_ref()?.root.clone());
                match root {
                    Some(root) => self.open_repos_at(&root),
                    None => self.open_repos(),
                }
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
            // Its count of errors: the inbox on the newest not looked at.
            (SidebarHit::Errors(i), BTN_LEFT) => {
                self.show_workspace_service_error(i);
            }
            (SidebarHit::More(i), BTN_LEFT) => {
                if let Some(line) = layout.more.get(i).copied().flatten() {
                    self.open_menu(self.more_agents_menu(i, line));
                }
            }
            (SidebarHit::Chip(i), BTN_LEFT) => {
                let cards = self.agent_views().into_iter().nth(i).unwrap_or_default();
                let blocked: Vec<_> = cards
                    .iter()
                    .filter(|c| c.status == crate::herdr::AgentStatus::Blocked)
                    .collect();
                let chip = layout.rows.get(i).and_then(|row| row.chip);
                match (blocked.as_slice(), chip) {
                    ([one], _) => {
                        let key = one.key.clone();
                        self.focus_agent(&key);
                    }
                    (_, Some(chip)) if !cards.is_empty() => {
                        if let Some(menu) = self.chip_agents_menu(i, chip) {
                            self.open_menu(menu);
                        }
                    }
                    _ => self.switch_workspace(i),
                }
            }
            (SidebarHit::Row(i) | SidebarHit::Name(i), BTN_LEFT) => {
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
        self.hover.header = self.project.is_some() && hit == Some(SidebarHit::Header);
        let bridge = self.strip_bridge(layout, pos);
        self.track_agent_strip(hit, bridge);
        self.hover
            .sidebar
            .track(hit.map(SidebarHit::pill), Instant::now());
        let workspaces = self.workspace_views();
        let agents = self.agent_views();
        match hit? {
            SidebarHit::Plus => {
                let tip = Tooltip::new(
                    "Add a workspace (a terminal, or a folder in Zed or Cursor), start a feature, or open the inbox",
                    layout.plus,
                );
                let help = self.key_state.keymap.help();
                let keys: Vec<_> = [Action::NewTerminal, Action::NewProject]
                    .into_iter()
                    .filter_map(|action| help.key_of(action, 0))
                    .collect();
                Some(if keys.is_empty() {
                    tip
                } else {
                    tip.key(&keys.join(" / "))
                })
            }
            SidebarHit::Repos => {
                let tip = Tooltip::new(&repos_tip(&self.repos_changed()), layout.repos);
                Some(
                    match self.key_state.keymap.help().key_of(Action::Repos, 0) {
                        Some(key) => tip.key(&key),
                        None => tip,
                    },
                )
            }
            SidebarHit::Close(i) => {
                let tip = Tooltip::new("Close workspace", layout.rows[i].close);
                Some(
                    match self
                        .key_state
                        .keymap
                        .help()
                        .key_of(Action::CloseWorkspace, 0)
                    {
                        Some(key) => tip.key(&key),
                        None => tip,
                    },
                )
            }
            // No tip of its own while it has agents: the strip it opens
            // under it says each one, and a tip shown before the strip
            // opens would sit in the way of the pointer moving onto it.
            SidebarHit::NewAgent(_) if !self.launcher.cli_agents.is_empty() => None,
            SidebarHit::NewAgent(i) => {
                let text = "No agent CLI found on PATH · add yours under [agents] in Settings";
                let tip = Tooltip::new(text, layout.rows[i].agent);
                Some(
                    match self.key_state.keymap.help().key_of(Action::NewAgent, 0) {
                        Some(key) => tip.key(&key),
                        None => tip,
                    },
                )
            }
            SidebarHit::PickAgent(i, k) => {
                let rect = layout.rows.get(i)?.pick(k)?;
                // No key on a slot: the AI button's opens the agent
                // picker, for the shown workspace only, and starts none.
                // The picker the "More agents…" slot opens is the same,
                // on the shown workspace's strip.
                Some(match self.picked_agent(layout, i, k) {
                    Some(agent) => {
                        let tip = Tooltip::new(&format!("New {} agent", agent.name), rect);
                        let field = match agent.command() {
                            Some(command) => ("Runs", command),
                            None => ("Opens", "T3 Code's app, in a web pane"),
                        };
                        tip.fields(vec![(field.0.to_owned(), field.1.to_owned())])
                    }
                    None => {
                        let tip = Tooltip::new("More agents…", rect);
                        let key = (i == self.active_workspace)
                            .then(|| self.key_state.keymap.help().key_of(Action::NewAgent, 0))
                            .flatten();
                        match key {
                            Some(key) => tip.key(&key),
                            None => tip,
                        }
                    }
                })
            }
            SidebarHit::Restart(i) => {
                let rect = layout.rows.get(i)?.restart?;
                // Each command, where it runs, whether it has exited, and
                // whether the user chose to leave it alone: status words
                // in sentence case, as every tooltip has them.
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
                            place = format!("Exited · {place}");
                        }
                        if choices
                            .iter()
                            .any(|ch| ch.command == c.command && !ch.restart)
                        {
                            place = format!("Skipped · {place}");
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
            SidebarHit::Errors(i) => {
                let errors = self.workspace_service_errors(i);
                let unseen = errors.iter().filter(|g| !g.seen).count();
                let text = match unseen {
                    1 => "1 service error".to_owned(),
                    n => format!("{n} service errors"),
                };
                let fields = errors
                    .iter()
                    .filter(|g| !g.seen)
                    .take(6)
                    .map(|g| (g.service.clone(), g.title.clone()))
                    .collect();
                let tip = Tooltip::new(&text, layout.rows[i].errors?).fields(fields);
                Some(tip.key("click to see the newest in the inbox"))
            }
            SidebarHit::Chip(i) => {
                let cards = agents.get(i).filter(|cards| !cards.is_empty())?;
                let blocked = chrome::blocked_count(cards);
                let text = match (cards.len(), blocked) {
                    (1, 0) => "1 agent".to_owned(),
                    (n, 0) => format!("{n} agents"),
                    (1, _) => "1 agent, blocked".to_owned(),
                    (n, b) => format!("{b} of {n} agents blocked"),
                };
                let fields = cards
                    .iter()
                    .map(|c| (c.label(), chrome::status_meaning(c.status).to_owned()))
                    .collect();
                let click = if blocked == 1 {
                    "click to show the blocked one"
                } else {
                    "click for the list"
                };
                Some(
                    Tooltip::new(&text, layout.rows[i].chip?)
                        .fields(fields)
                        .key(click),
                )
            }
            SidebarHit::Name(i) => {
                let (ws, row) = (workspaces.get(i)?, layout.rows.get(i)?);
                Some(workspace_tooltip(&ws.0, row.title, self.workspace_venv(i)))
            }
            SidebarHit::Fold(i) => {
                let (ws, row) = (workspaces.get(i)?, layout.rows.get(i)?);
                let by_hand = self.workspace_folded(i).is_some();
                let text = match (ws.0.folded, by_hand) {
                    (true, true) => "Unfold (folded by hand)",
                    (true, false) => "Unfold (folded while quiet)",
                    (false, _) => "Fold",
                };
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
                let tip = Tooltip::new(
                    "Agents of other workspaces that are blocked or done · open the inbox, \
                     which lists repos, pull requests and tickets too",
                    layout.inbox_title?,
                );
                Some(
                    match self.key_state.keymap.help().key_of(Action::Inbox, 0) {
                        Some(key) => tip.key(&key),
                        None => tip,
                    },
                )
            }
            SidebarHit::FeaturesTitle => {
                // Shipped and abandoned features leave the list: say where.
                let history = self.feature_history_count();
                let text = match history {
                    0 => "The project's open features · open the feature panel".to_owned(),
                    n => format!(
                        "The project's open features · open the feature panel · \
                         {n} shipped or abandoned in History (right-click)"
                    ),
                };
                let tip = Tooltip::new(&text, layout.features_title?);
                Some(
                    match self.key_state.keymap.help().key_of(Action::Features, 0) {
                        Some(key) => tip.key(&key),
                        None => tip,
                    },
                )
            }
            SidebarHit::NewFeature => {
                Some(Tooltip::new("New feature\u{2026}", layout.features_plus?))
            }
            SidebarHit::FeaturesWaiting => {
                let views: Vec<_> = workspaces.iter().map(|w| w.0.clone()).collect();
                let n = chrome::features_waiting(&views, &self.feature_rows());
                let text = match n {
                    1 => "1 feature waits for you".to_owned(),
                    n => format!("{n} features wait for you"),
                };
                let tip = Tooltip::new(&text, layout.features_waiting?);
                Some(tip.key("click to see them in the inbox"))
            }
            SidebarHit::Feature(n) => {
                let f = self.sidebar_feature(n)?;
                let rect = *layout.features.get(n)?;
                // The row cuts its title to fit, "in <home>" first.
                let tip = Tooltip::new(&self.row_title(f), rect).fields(feature_fields(f));
                if self.project.as_deref() == Some(f.project.as_str()) {
                    return Some(tip.key("click to open its panel"));
                }
                let home = self.home_name(f);
                Some(tip.key(&format!("click to switch to {home} and open its panel")))
            }
            // A feature's own workspace: a stage of it.
            SidebarHit::Stage(i, k) => {
                let f = self.workspace_feature(i)?;
                let rect = layout.rows.get(i)?.stages?;
                let card = self.feature_card(f);
                Some(stage_tooltip(&card, k, rect)?.key("click to open this stage"))
            }
            SidebarHit::FeatureDot(n, k) => {
                let f = self.sidebar_feature(n)?;
                let tip = stage_tooltip(&self.feature_card(f), k, *layout.features.get(n)?)?;
                if self.project.as_deref() == Some(f.project.as_str()) {
                    return Some(tip.key("click to open this stage"));
                }
                let home = self.home_name(f);
                Some(tip.key(&format!("click to switch to {home} and open its panel")))
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
                let mut fields = card.tooltip_fields();
                if let Some(why) = card.close_unavailable() {
                    fields.push(("No ×".to_owned(), why.to_owned()));
                }
                Some(Tooltip::new(&card.label(), rect).fields(fields))
            }
            SidebarHit::CardClose(i, k) => {
                let card = agents.get(i)?.get(k)?;
                let rect = layout.card_closes[i].iter().find(|(c, _)| *c == k)?.1;
                Some(Tooltip::new(card.close_label(), rect))
            }
            SidebarHit::Subagent(i, k, j) => {
                let task = &agents.get(i)?.get(k)?.subagents.get(j)?.task;
                let rect = layout.subagents[i].iter().find(|l| (l.0, l.1) == (k, j))?.2;
                Some(Tooltip::new(task, rect))
            }
            SidebarHit::More(i) => {
                let names = &workspaces.get(i)?.0.more_agents;
                let text = format!("Also kept: {}", names.join(" · "));
                Some(Tooltip::new(&text, layout.more[i]?).key("click for the list"))
            }
            SidebarHit::Worktrees(i) => {
                let (ws, row) = (workspaces.get(i)?, layout.rows.get(i)?);
                let root = ws.0.git.as_ref().and_then(|g| g.root.as_deref());
                // Below the whole row, clear of the lines it explains.
                row.worktrees?;
                Some(worktrees_tooltip(ws.0.worktrees(), root, row.rect))
            }
            SidebarHit::GitMarks(i) => {
                let git = workspaces.get(i)?.0.git.as_ref()?;
                let marks = layout.rows.get(i)?.marks?;
                Some(Tooltip::new(&git_marks_tip(git), marks).key("click to open in Repositories"))
            }
            SidebarHit::Attention(i, _) => {
                let lines = layout.rows.get(i)?.attention?;
                let tip = Tooltip::new("Review, merge or delete", lines);
                Some(
                    match self.key_state.keymap.help().key_of(Action::Worktrees, 0) {
                        Some(key) => tip.key(&key),
                        None => tip,
                    },
                )
            }
            SidebarHit::Row(i) => {
                let (ws, row) = (workspaces.get(i)?, layout.rows.get(i)?);
                Some(workspace_tooltip(&ws.0, row.title, self.workspace_venv(i)))
            }
            SidebarHit::Header => {
                self.project.as_ref()?;
                let tip = Tooltip::new("Project menu: switch, add or stop projects", layout.header);
                Some(
                    match self.key_state.keymap.help().key_of(Action::Projects, 0) {
                        Some(key) => tip.key(&key),
                        None => tip,
                    },
                )
            }
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
            SidebarHit::Header => Some(MenuSpec {
                anchor: at,
                items: vec![
                    MenuItem::new("Fold all workspaces", MenuAction::FoldAll(Some(true))),
                    MenuItem::new("Unfold all workspaces", MenuAction::FoldAll(Some(false))),
                    MenuItem::new("Fold each while quiet", MenuAction::FoldAll(None)),
                    MenuItem::new("New feature\u{2026}", MenuAction::NewFeature(None)),
                    MenuItem::new(
                        "Review pull requests\u{2026}",
                        MenuAction::ReviewPulls(None),
                    ),
                ],
            }),
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
                if let Subject::Pr(n) = &row.subject {
                    let open = (self.project.as_deref()).and_then(|id| {
                        self.pr_review_of(id, &repo.host, &repo.slug, *n)
                    });
                    let label = match open {
                        Some(_) => "Open review",
                        None => "Start review",
                    };
                    items.push(MenuItem::new(label, MenuAction::ReviewPr(i, row.link.clone())));
                }
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
                items.push(match card.dir.clone() {
                    Some(cwd) => MenuItem::new("Copy path", MenuAction::CopyPath(cwd)),
                    None => MenuItem::new("Copy path", MenuAction::NewTerminal).enabled(false),
                });
                if card.closable {
                    let label = card.close_label();
                    let label = if card.running() {
                        format!("{label}\u{2026}")
                    } else {
                        label.to_owned()
                    };
                    items.push(MenuItem::new(&label, MenuAction::CloseAgent(card.key)));
                } else if card.close_unavailable().is_some() {
                    // There, but off, so it can be found; the card's
                    // tooltip says why.
                    let item = MenuItem::new(card.close_label(), MenuAction::CloseAgent(card.key));
                    items.push(item.enabled(false));
                }
                Some(MenuSpec { anchor: at, items })
            }
            SidebarHit::FeaturesTitle | SidebarHit::NewFeature | SidebarHit::FeaturesWaiting => {
                Some(MenuSpec {
                    anchor: at,
                    items: vec![
                        MenuItem::new("Open features", MenuAction::Features),
                        MenuItem::new("History\u{2026}", MenuAction::FeatureHistory),
                        MenuItem::new("New feature\u{2026}", MenuAction::NewFeature(None)),
                        MenuItem::new(
                            "Review pull requests\u{2026}",
                            MenuAction::ReviewPulls(None),
                        ),
                    ],
                })
            }
            SidebarHit::Feature(n) | SidebarHit::FeatureDot(n, _) => {
                let f = self.sidebar_feature(n)?;
                let open = MenuAction::OpenFeature(f.project.clone(), f.slug.clone());
                // Another project's feature: its folder in the shown one.
                let shown = self.project.as_deref();
                let link = f.links.iter().find(|l| Some(l.project.as_str()) == shown);
                let dir = link.map_or(&f.dir, |l| &l.dir).clone();
                let items = vec![
                    MenuItem::new("Open", open),
                    MenuItem::new("Copy path", MenuAction::CopyPath(dir)),
                    MenuItem::new("New feature\u{2026}", MenuAction::NewFeature(None)),
                ];
                Some(MenuSpec { anchor: at, items })
            }
            hit => {
                let i = hit.row()?;
                let dir = self.workspaces.get(i)?.dir.clone();
                let help = self.key_state.keymap.help();
                let hinted = |item: MenuItem, action: Action| match help.key_of(action, 0) {
                    Some(key) => item.hint(&key),
                    None => item,
                };
                let mut items = vec![
                    hinted(
                        MenuItem::new("New tab", MenuAction::NewTab(i)),
                        Action::NewTab,
                    ),
                    hinted(
                        MenuItem::new("New agent…", MenuAction::NewAgent(i)),
                        Action::NewAgent,
                    ),
                    MenuItem::new("Rename", MenuAction::RenameWorkspace(i)),
                    MenuItem::new("Copy path", MenuAction::CopyPath(dir.clone())),
                    MenuItem::new("New feature\u{2026}", MenuAction::NewFeature(Some(i))),
                ];
                // A feature's own workspace opens its feature's panel.
                if let Some(f) = self.workspace_feature(i) {
                    let open = MenuAction::OpenFeature(f.project.clone(), f.slug.clone());
                    items.insert(4, MenuItem::new("Open feature", open));
                }
                let git = self.git.status.get(&dir);
                if git.is_some_and(|g| g.root.is_some()) {
                    let item = MenuItem::new("Repositories", MenuAction::Repos);
                    items.push(hinted(item, Action::Repos));
                }
                if git.is_some_and(|g| !g.worktrees.is_empty()) {
                    let item = MenuItem::new("Worktrees", MenuAction::Worktrees(dir));
                    items.push(hinted(item, Action::Worktrees));
                }
                let restartable = !self.workspace_restarts(i).is_empty();
                let restart = MenuItem::new("Restart commands", MenuAction::RestartWorkspace(i));
                items.push(restart.enabled(restartable));
                let choose =
                    MenuItem::new("Choose commands to restart…", MenuAction::ChooseRestarts(i));
                items.push(choose.enabled(restartable));
                items.push(hinted(
                    MenuItem::new("Close workspace", MenuAction::CloseWorkspace(i)),
                    Action::CloseWorkspace,
                ));
                Some(MenuSpec { anchor: at, items })
            }
        }
    }

    /// The menu under the header's "+": the workspaces it can add, each
    /// saying what it opens (a project holds the workspaces; see
    /// `docs/ux-conventions.md`), and the inbox, with the keys that do the
    /// same.
    pub(crate) fn sidebar_plus_menu(&self, layout: &SidebarLayout) -> MenuSpec {
        let help = self.key_state.keymap.help();
        let hinted = |item: MenuItem, action: Action| match help.key_of(action, 0) {
            Some(key) => item.hint(&key),
            None => item,
        };
        MenuSpec {
            anchor: layout.plus,
            items: vec![
                hinted(
                    MenuItem::new("Terminal workspace in home", MenuAction::NewTerminal),
                    Action::NewTerminal,
                ),
                MenuItem::new("Terminal workspace in a folder…", MenuAction::OpenFolder),
                hinted(
                    MenuItem::new("Zed workspace in a folder…", MenuAction::NewProject),
                    Action::NewProject,
                ),
                MenuItem::new("Cursor workspace", MenuAction::NewCursor),
                MenuItem::new("New feature\u{2026}", MenuAction::NewFeature(None)),
                hinted(
                    MenuItem::new("Open inbox", MenuAction::Inbox),
                    Action::Inbox,
                ),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::{STRIP_DELAY, SidebarHit, VenvSetting, strip_after, workspace_tooltip};

    #[test]
    fn the_strip_of_agents_opens_once_the_pointer_rests_on_the_ai_button() {
        let start = Instant::now();
        let on = Some(SidebarHit::NewAgent(1));
        // Passing over the button opens nothing yet.
        let (open, due) = strip_after(on, None, None, None, start);
        assert_eq!((open, due), (None, Some((1, start + STRIP_DELAY))));
        let (open, due2) = strip_after(on, None, None, due, start + STRIP_DELAY / 2);
        assert_eq!((open, due2), (None, due));
        assert_eq!(
            strip_after(Some(SidebarHit::Row(1)), None, None, due, start),
            (None, None)
        );
        // Resting there, it opens.
        let (open, due) = strip_after(on, None, None, due, start + STRIP_DELAY);
        assert_eq!((open, due), (Some(1), None));
        // It stays on the button and on its strip, and closes off them.
        assert_eq!(strip_after(on, None, open, None, start), (Some(1), None));
        let pick = Some(SidebarHit::PickAgent(1, 2));
        assert_eq!(strip_after(pick, None, open, None, start), (Some(1), None));
        assert_eq!(strip_after(pick, None, None, None, start), (None, None));
        let errors = Some(SidebarHit::Errors(1));
        assert_eq!(strip_after(errors, None, open, None, start), (None, None));
        // Another header's button starts its own wait.
        let other = Some(SidebarHit::NewAgent(2));
        assert_eq!(
            strip_after(other, None, open, None, start),
            (None, Some((2, start + STRIP_DELAY)))
        );
    }

    #[test]
    fn the_strip_of_agents_stays_open_on_the_way_from_the_button() {
        let start = Instant::now();
        // Over the branch's line under the button: its worktree count is
        // hit, and the strip stays.
        let count = Some(SidebarHit::Worktrees(1));
        assert_eq!(
            strip_after(count, Some(1), Some(1), None, start),
            (Some(1), None)
        );
        // Not another header's strip, nor one not open yet.
        assert_eq!(
            strip_after(count, Some(1), Some(2), None, start),
            (None, None)
        );
        assert_eq!(strip_after(count, Some(1), None, None, start), (None, None));
    }

    #[test]
    fn the_strip_of_agents_does_not_open_once_the_button_is_left_or_covered() {
        let start = Instant::now();
        let due = Some((1, start + STRIP_DELAY));
        let later = start + STRIP_DELAY;
        // The timer finds nothing under the pointer: a click on the
        // button opened the agent picker over the sidebar.
        assert_eq!(strip_after(None, None, None, due, later), (None, None));
        // The workspaces moved under a still pointer: another header's
        // button is there now, which waits its own time.
        assert_eq!(
            strip_after(Some(SidebarHit::NewAgent(0)), None, None, due, later),
            (None, Some((0, later + STRIP_DELAY)))
        );
        // A strip open under what now covers the sidebar closes.
        assert_eq!(strip_after(None, None, Some(1), None, later), (None, None));
    }
    use crate::{WorkspaceKind, chrome::Rect, chrome::WorkspaceView};

    #[test]
    fn a_workspace_tooltip_says_which_venv_its_shells_activate() {
        let ws = WorkspaceView::new("api".into(), WorkspaceKind::Terminal, &[]);
        let venv = |setting: Option<&VenvSetting>| {
            let tip = workspace_tooltip(&ws, Rect::default(), setting);
            tip.fields
                .into_iter()
                .find(|(label, _)| label == "Python venv")
                .map(|(_, value)| value)
        };
        assert_eq!(venv(Some(&VenvSetting::Off)).as_deref(), Some("Off"));
        assert!(venv(Some(&VenvSetting::Auto)).unwrap().starts_with("Auto"));
        let path = VenvSetting::Path(".venv-3.12".into());
        assert_eq!(venv(Some(&path)).as_deref(), Some(".venv-3.12"));
        // A workspace without shells says nothing of it.
        assert_eq!(venv(None), None);
    }
}

//! The machines panel: every machine saved, how its link stands, and what of
//! this gerdr's features runs there, with buttons to connect, disconnect,
//! remove or fix a machine and to show or stop what runs on it (see
//! `chrome::machines_panel`). It opens with the prefix, then O, from the
//! launcher, and from a machine's chip in the footer.
//!
//! Up/Down move through the machines, Tab/Shift+Tab through the buttons of
//! the one selected, Enter presses the focused button (on the list: the
//! machine's primary one), Del arms and then removes the machine, Esc closes.

use std::time::Duration;

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point},
};

use crate::{
    Gerdr,
    chrome::{
        self, Dot, MachineDetailView, MachineRowView, MachineRunView, MachinesFocus, MachinesHit,
        MachinesPanelView,
    },
    feature::{Feature, Stage},
    herdr::AgentStatus,
    machine_load::Load,
    machines::{Failure, LinkState},
    tasks::uptime,
};

/// The machines panel's state, while it is open.
#[derive(Debug, Default)]
pub struct MachinesPanel {
    /// The list row selected: past the machines is the add row.
    pub selected: usize,
    pub first: usize,
    pub focus: MachinesFocus,
    pub runs_first: usize,
    pub hover: Option<MachinesHit>,
    /// The machine Del was pressed on once: a second Del removes it.
    pub armed: Option<String>,
}

/// A run on a machine, as the panel's buttons act on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunRef {
    /// A stage's agent of a feature: project, slug, stage.
    Agent(String, String, Stage),
    /// A feature's Verify checks or setup: project, slug.
    Checks(String, String),
    /// A feature's sandbox: project, slug.
    Sandbox(String, String),
    /// What gone features left on the machine of this id.
    Left(String),
}

/// What a press in the machines panel does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Press {
    /// Open the dialog connecting a machine.
    Add,
    /// Connect, disconnect or remove the machine of this name.
    Connect(String),
    Disconnect(String),
    Remove(String),
    /// Press the button fixing what failed on the machine of this name
    /// (Copy command, Open terminal).
    Fix(String),
    /// Show what runs: its tab here.
    Show(RunRef),
    /// Stop it, asking as its own page does. What gone features left is
    /// removed, asking too.
    Stop(RunRef),
    /// Open the feature's tab: project, slug.
    OpenFeature(String, String),
}

/// What the machines panel asks gerdr to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Close,
    Press(Press),
}

/// What the panel shows, and what its buttons press: built together, so a
/// button's label and its press can't part.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Content {
    rows: Vec<MachineRowView>,
    /// The selected machine; none on the add row.
    detail: Option<MachineDetailView>,
    /// What each run's buttons press, as `detail.runs` has them.
    run_presses: Vec<Vec<Press>>,
    /// What the machine's buttons press, as `detail.buttons` has them.
    buttons: Vec<Press>,
}

impl Content {
    /// The add row's index.
    fn add_row(&self) -> usize {
        self.rows.len()
    }

    /// The name of the machine on list row `row`, if it is one.
    fn name(&self, row: usize) -> Option<&str> {
        self.rows.get(row).map(|r| r.name.as_str())
    }

    /// Where Tab goes, in order: the list, each run's buttons row by row,
    /// then the machine's buttons.
    fn stops(&self) -> Vec<MachinesFocus> {
        let runs = (self.run_presses.iter().enumerate())
            .flat_map(|(r, presses)| (0..presses.len()).map(move |b| MachinesFocus::Run(r, b)));
        let buttons = (0..self.buttons.len()).map(MachinesFocus::Button);
        std::iter::once(MachinesFocus::List)
            .chain(runs)
            .chain(buttons)
            .collect()
    }

    /// What focus `focus` presses on list row `selected`: on the list, the
    /// machine's primary button, or Add on the add row.
    fn press(&self, selected: usize, focus: MachinesFocus) -> Option<Press> {
        match focus {
            MachinesFocus::List if selected >= self.add_row() => Some(Press::Add),
            MachinesFocus::List => self.buttons.last().cloned(),
            MachinesFocus::Run(r, b) => self.run_presses.get(r)?.get(b).cloned(),
            MachinesFocus::Button(b) => self.buttons.get(b).cloned(),
        }
    }

    /// The label of what focus `focus` presses on list row `selected`.
    fn label(&self, selected: usize, focus: MachinesFocus) -> Option<String> {
        let detail = self.detail.as_ref();
        match focus {
            MachinesFocus::List if selected >= self.add_row() => Some("Add machine".to_owned()),
            MachinesFocus::List => detail?.buttons.last().cloned(),
            MachinesFocus::Run(r, b) => detail?.runs.get(r)?.buttons.get(b).cloned(),
            MachinesFocus::Button(b) => detail?.buttons.get(b).cloned(),
        }
    }
}

/// How a machine's link stands, shortly, and its dot.
fn link_chip(state: Option<&LinkState>) -> (&'static str, Dot) {
    match state {
        Some(LinkState::Up(_)) => ("connected", Dot::Done),
        Some(LinkState::Connecting(_)) => ("connecting", Dot::Working),
        Some(LinkState::Retrying(_)) => ("away", Dot::Blocked),
        Some(LinkState::Failed(_)) => ("can't connect", Dot::Failed),
        Some(LinkState::Off) | None => ("off", Dot::Idle),
    }
}

/// What keeps a link from being up, when it says.
fn link_failure(state: Option<&LinkState>) -> Option<&Failure> {
    match state {
        Some(LinkState::Failed(failure) | LinkState::Retrying(failure)) => Some(failure),
        _ => None,
    }
}

/// The buttons of machine `name` at `target`, its link in `state`, left to
/// right: the last is the primary one.
fn machine_buttons(name: &str, target: &str, state: Option<&LinkState>) -> Vec<(String, Press)> {
    let mut buttons = vec![("Remove".to_owned(), Press::Remove(name.to_owned()))];
    if let Some(fix) = link_failure(state).and_then(|f| f.fix(target)) {
        buttons.push((fix.label().to_owned(), Press::Fix(name.to_owned())));
    }
    buttons.push(match state {
        Some(LinkState::Up(_) | LinkState::Connecting(_) | LinkState::Retrying(_)) => {
            ("Disconnect".to_owned(), Press::Disconnect(name.to_owned()))
        }
        Some(LinkState::Failed(_) | LinkState::Off) | None => {
            ("Connect".to_owned(), Press::Connect(name.to_owned()))
        }
    });
    buttons
}

/// The buttons of run `run`, labels and what they press; `live`: an agent
/// up, not one on its way.
fn run_buttons(run: &RunRef, live: bool) -> Vec<(&'static str, Press)> {
    let show = || ("Show", Press::Show(run.clone()));
    let stop = || ("Stop", Press::Stop(run.clone()));
    let open = |project: &str, slug: &str| {
        let press = Press::OpenFeature(project.to_owned(), slug.to_owned());
        ("Open feature", press)
    };
    match run {
        RunRef::Agent(project, slug, _) if live => vec![show(), stop(), open(project, slug)],
        RunRef::Agent(project, slug, _) => vec![open(project, slug)],
        RunRef::Checks(project, slug) => vec![show(), open(project, slug)],
        RunRef::Sandbox(project, slug) => vec![show(), stop(), open(project, slug)],
        RunRef::Left(_) => vec![("Remove", Press::Stop(run.clone()))],
    }
}

/// A run's row and what its buttons press, from one list of buttons.
fn run_entry(
    run: &RunRef,
    live: bool,
    title: String,
    detail: String,
    dot: Dot,
) -> (MachineRunView, Vec<Press>) {
    let (labels, presses) = run_buttons(run, live)
        .into_iter()
        .map(|(label, press)| (label.to_owned(), press))
        .unzip();
    let view = MachineRunView {
        title,
        detail,
        dot,
        buttons: labels,
    };
    (view, presses)
}

/// The lines saying how a machine's link stands, top first: its address,
/// then how long it is up and how it is when it is, or when it was last seen
/// when it isn't, then its id.
fn machine_facts(
    target: &str,
    state: Option<&LinkState>,
    up_for: Option<Duration>,
    seen_ago: Option<Duration>,
    load: Option<Load>,
    id: Option<&str>,
) -> Vec<String> {
    let mut facts = vec![target.to_owned()];
    match state {
        Some(LinkState::Up(info)) => {
            let mut parts = Vec::new();
            if let Some(up_for) = up_for {
                parts.push(format!("up {}", uptime(up_for.as_secs())));
            }
            parts.push(format!("herdr {}", info.site.version));
            if let Some(load) = load {
                parts.push(format!("load {:.1} / {}", load.average, load.cores));
            }
            facts.push(parts.join(" \u{b7} "));
        }
        Some(LinkState::Connecting(step)) => facts.push(step.clone()),
        _ => {
            if let Some(ago) = seen_ago {
                facts.push(format!("last seen {} ago", uptime(ago.as_secs())));
            }
        }
    }
    if let Some(id) = id.filter(|id| !id.is_empty()) {
        facts.push(format!("id {}", id.chars().take(8).collect::<String>()));
    }
    facts
}

/// What an agent's status says, shortly.
fn status_word(status: Option<AgentStatus>) -> Option<&'static str> {
    Some(match status? {
        AgentStatus::Working => "working",
        AgentStatus::Blocked => "waiting for you",
        AgentStatus::Done => "done",
        AgentStatus::Idle => "idle",
        AgentStatus::Unknown => return None,
    })
}

/// How long ago Unix second `then` was, shortly.
fn age(then: u64) -> String {
    uptime(crate::feature::now().saturating_sub(then))
}

/// The key hints for list row `selected` of `content`, the keyboard on
/// `focus`, Del armed or not.
fn hints(
    content: &Content,
    selected: usize,
    focus: MachinesFocus,
    armed: bool,
) -> Vec<(String, String)> {
    let hint = |k: &str, what: &str| (k.to_owned(), what.to_owned());
    let mut hints = vec![hint("Up / Down", "move")];
    if content.stops().len() > 1 {
        hints.push(hint("Tab", "next button"));
    }
    if let Some(label) = content.label(selected, focus) {
        hints.push(hint("Enter", &label.to_lowercase()));
    }
    if content.name(selected).is_some() {
        hints.push(hint("Del", if armed { "remove now" } else { "remove" }));
    }
    hints.push(hint("Esc", "close"));
    hints
}

/// `first` moved so `at` shows among `page` rows of `count`.
fn reveal(first: usize, at: usize, page: usize, count: usize) -> usize {
    let page = page.max(1);
    let first = if at < first {
        at
    } else if at >= first + page {
        at + 1 - page
    } else {
        first
    };
    first.min(count.saturating_sub(page))
}

impl MachinesPanel {
    /// Keep what it points at in `content`: a machine gone moves the
    /// selection back, a button gone the keyboard to the list.
    fn settle(&mut self, content: &Content) {
        self.selected = self.selected.min(content.add_row());
        if !content.stops().contains(&self.focus) {
            self.focus = MachinesFocus::List;
        }
        if self.armed.as_deref() != content.name(self.selected) {
            self.armed = None;
        }
        let runs = content.run_presses.len();
        self.runs_first = self.runs_first.min(runs.saturating_sub(1));
    }

    /// Select list row `row`: the keyboard back on the list, Del unarmed.
    fn select(&mut self, row: usize) {
        if row != self.selected {
            self.selected = row;
            self.runs_first = 0;
        }
        self.focus = MachinesFocus::List;
        self.armed = None;
    }

    /// A key on `content`, `list_page` list rows and `runs_page` runs
    /// showing at once.
    fn key(
        &mut self,
        content: &Content,
        mods: &ModifiersState,
        sym: Keysym,
        (list_page, runs_page): (usize, usize),
    ) -> Option<Outcome> {
        self.settle(content);
        let row = self.selected;
        let last = content.add_row();
        let plain = !mods.ctrl && !mods.alt && !mods.logo;
        let move_key = plain || (mods.ctrl && !mods.alt && !mods.logo);
        let armed = self.armed.take();
        let mut to = None;
        match sym {
            Keysym::Escape => return Some(Outcome::Close),
            Keysym::Up | Keysym::KP_Up => to = Some(row.saturating_sub(1)),
            Keysym::k if move_key => to = Some(row.saturating_sub(1)),
            Keysym::Down | Keysym::KP_Down => to = Some((row + 1).min(last)),
            Keysym::j if move_key => to = Some((row + 1).min(last)),
            Keysym::Page_Up | Keysym::KP_Page_Up => to = Some(row.saturating_sub(list_page)),
            Keysym::Page_Down | Keysym::KP_Page_Down => to = Some((row + list_page).min(last)),
            Keysym::Home | Keysym::KP_Home => to = Some(0),
            Keysym::End | Keysym::KP_End => to = Some(last),
            Keysym::Tab | Keysym::ISO_Left_Tab => {
                let back = sym == Keysym::ISO_Left_Tab || mods.shift;
                let stops = content.stops();
                let at = stops.iter().position(|f| *f == self.focus).unwrap_or(0);
                let next = match back {
                    true => (at + stops.len() - 1) % stops.len(),
                    false => (at + 1) % stops.len(),
                };
                self.focus = stops[next];
                if let MachinesFocus::Run(r, _) = self.focus {
                    let runs = content.run_presses.len();
                    self.runs_first = reveal(self.runs_first, r, runs_page, runs);
                }
            }
            Keysym::Return | Keysym::KP_Enter => {
                return content.press(row, self.focus).map(Outcome::Press);
            }
            Keysym::Delete | Keysym::KP_Delete => {
                let name = content.name(row)?;
                if armed.as_deref() == Some(name) {
                    return Some(Outcome::Press(Press::Remove(name.to_owned())));
                }
                self.armed = Some(name.to_owned());
            }
            _ => {}
        }
        if let Some(to) = to {
            self.select(to);
            self.first = reveal(self.first, to, list_page, last + 1);
        }
        None
    }

    /// The wheel `rows` rows on, over the runs or the list, of `count`
    /// with `page` showing at once.
    fn scroll(&mut self, rows: i32, on_runs: bool, count: usize, page: usize) {
        let first = match on_runs {
            true => &mut self.runs_first,
            false => &mut self.first,
        };
        let to = *first as i64 + rows as i64;
        *first = to.clamp(0, count.saturating_sub(page.max(1)) as i64) as usize;
    }
}

impl Gerdr {
    /// Open the machines panel, on the machine of name `on` if given.
    /// The other panels close: it would cover them.
    pub fn open_machines(&mut self, on: Option<&str>) {
        self.switcher = None;
        self.repos.close();
        self.worktrees.close();
        self.project_menu = None;
        self.scrollback = None;
        self.tasks_panel = None;
        if self.inbox.is_open() {
            self.close_inbox();
        }
        self.stop_key_repeat();
        self.close_menu();
        self.tooltip.hide();
        self.chrome.drop_tooltip();
        let selected = on
            .and_then(|name| self.machines.saved.keys().position(|n| n == name))
            .unwrap_or(0);
        let count = self.machines.saved.len() + 1;
        let page = chrome::machines_shown(count, self.output_size());
        self.machines_panel = Some(MachinesPanel {
            selected,
            first: reveal(0, selected, page, count),
            ..MachinesPanel::default()
        });
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// Close the machines panel.
    pub fn close_machines(&mut self) {
        self.machines_panel = None;
        self.refresh_pointer_focus();
    }

    /// The ids machine `name` is known by: the one it said, connected, then
    /// those it said before.
    fn machine_ids(&self, name: &str) -> Vec<String> {
        let mut ids = Vec::new();
        let link = self.machines.links.get(name);
        if let Some(work) = link.and_then(|l| l.work.as_ref()) {
            ids.push(work.id.clone());
        } else if let Some(LinkState::Up(info)) = link.map(|l| &l.state) {
            ids.push(info.site.id.clone());
        }
        let mut known: Vec<&String> = (self.machines.names.iter())
            .filter(|(id, called)| *called == name && !ids.contains(id))
            .map(|(id, _)| id)
            .collect();
        known.sort();
        ids.extend(known.into_iter().cloned());
        ids.retain(|id| !id.is_empty());
        ids
    }

    /// What of this gerdr's features runs on the machine of `ids`, and what
    /// gone features left there: rows, and what their buttons press.
    fn machine_runs(&self, ids: &[String]) -> Vec<(MachineRunView, Vec<Press>)> {
        let on = |machine: Option<&String>| machine.is_some_and(|m| ids.contains(m));
        let viewed = self.features.viewed.values().flatten();
        let features: Vec<&Feature> = self.features.all.values().flatten().chain(viewed).collect();
        let mut runs = Vec::new();
        for f in &features {
            for (stage, run) in &f.runs {
                let Some(pane) = run.pane.as_deref().filter(|_| on(run.machine.as_ref())) else {
                    continue;
                };
                let (status, alive) = self.stage_run_state(run);
                if !alive {
                    continue;
                }
                let mut detail = vec![run.agent.clone()];
                detail.extend(run.model.clone());
                if self.pane_out_of_reach(pane) {
                    detail.push("out of reach".to_owned());
                } else {
                    detail.extend(status_word(status).map(str::to_owned));
                }
                detail.push(age(run.started));
                let what = RunRef::Agent(f.project.clone(), f.slug.clone(), *stage);
                let title = format!("{} \u{b7} {} agent", f.title, stage.label());
                let dot = status.map_or(Dot::Unknown, Dot::of_status);
                runs.push(run_entry(&what, true, title, detail.join(" \u{b7} "), dot));
            }
            if let Some(verify) =
                (f.verify.as_ref()).filter(|v| on(v.machine.as_ref()) && v.finished.is_none())
            {
                let passed = (verify.checks.iter())
                    .filter(|c| c.outcome.as_ref().is_some_and(|o| o.passed()))
                    .count();
                let detail = format!(
                    "{} of {} checks passed \u{b7} {}",
                    passed,
                    verify.checks.len(),
                    age(verify.started)
                );
                let what = RunRef::Checks(f.project.clone(), f.slug.clone());
                let title = format!("{} \u{b7} Verify checks", f.title);
                runs.push(run_entry(&what, true, title, detail, Dot::Working));
            } else if let Some(setup) = (f.setup.as_ref())
                .filter(|s| on(s.machine.as_ref()) && s.finished.is_none() && s.pane.is_some())
            {
                let what = RunRef::Checks(f.project.clone(), f.slug.clone());
                let title = format!("{} \u{b7} setup", f.title);
                let detail = format!("setting up \u{b7} {}", age(setup.started));
                runs.push(run_entry(&what, true, title, detail, Dot::Working));
            }
            if let Some(sandbox) = f.sandbox.as_ref().filter(|s| on(s.machine.as_ref())) {
                let (word, dot) = match (&sandbox.failed, sandbox.finished) {
                    (Some(_), _) => ("didn't start", Dot::Failed),
                    (None, None) => ("starting", Dot::Working),
                    (None, Some(_)) => ("running", Dot::Done),
                };
                let mut detail = vec![word.to_owned()];
                if sandbox.kept {
                    detail.push("kept there".to_owned());
                }
                detail.push(age(sandbox.started));
                let what = RunRef::Sandbox(f.project.clone(), f.slug.clone());
                let title = format!("{} \u{b7} sandbox", f.title);
                runs.push(run_entry(&what, true, title, detail.join(" \u{b7} "), dot));
            }
        }
        let mut starting: Vec<_> = (self.machines.starting.iter())
            .filter(|(_, id)| ids.contains(id))
            .map(|(key, _)| key.clone())
            .collect();
        starting.sort();
        for (project, slug, stage) in starting {
            let Some(f) = features
                .iter()
                .find(|f| f.project == project && f.slug == slug)
            else {
                continue;
            };
            if (f.runs.get(&stage)).is_some_and(|r| r.pane.is_some()) {
                continue;
            }
            let what = RunRef::Agent(project, slug, stage);
            let title = format!("{} \u{b7} {} agent", f.title, stage.label());
            runs.push(run_entry(
                &what,
                false,
                title,
                "starting".to_owned(),
                Dot::Working,
            ));
        }
        for id in ids {
            let left = self.machines.left.get(&Some(id.clone()));
            let Some(left) = left.filter(|l| !l.is_empty()) else {
                continue;
            };
            let stacks: Vec<&str> = left.iter().map(|l| l.stack.as_str()).collect();
            let what = RunRef::Left(id.clone());
            let title = "Left by gone features".to_owned();
            runs.push(run_entry(
                &what,
                true,
                title,
                stacks.join(", "),
                Dot::Blocked,
            ));
        }
        runs
    }

    /// The panel's rows, and the detail of list row `selected`.
    fn machines_content(&self, selected: usize) -> Content {
        let rows: Vec<MachineRowView> = (self.machines.saved.iter())
            .map(|(name, saved)| {
                let link = self.machines.links.get(name);
                let (state, dot) = link_chip(link.map(|l| &l.state));
                MachineRowView {
                    name: name.clone(),
                    address: link.map_or(saved.as_str(), |l| l.target()).to_owned(),
                    state: state.to_owned(),
                    dot,
                    armed: false,
                }
            })
            .collect();
        let Some(row) = rows.get(selected) else {
            return Content {
                rows,
                ..Content::default()
            };
        };
        let name = row.name.clone();
        let target = row.address.clone();
        let dot = row.dot;
        let link = self.machines.links.get(&name);
        let state = link.map(|l| &l.state);
        let ids = self.machine_ids(&name);
        let facts = machine_facts(
            &target,
            state,
            link.and_then(|l| l.connected_since).map(|at| at.elapsed()),
            link.and_then(|l| l.last_seen).map(|at| at.elapsed()),
            link.and_then(|l| l.work.as_ref()).and_then(|w| w.load),
            ids.first().map(String::as_str),
        );
        let failure = link_failure(state);
        let how = failure.and_then(|f| match (f.how(&target), state) {
            (Some(how), _) => Some(how),
            (None, Some(LinkState::Retrying(_))) => Some("gerdr tries again by itself.".into()),
            (None, _) => None,
        });
        let up = matches!(state, Some(LinkState::Up(_)));
        let (runs, run_presses) = self.machine_runs(&ids).into_iter().unzip();
        let (labels, buttons) = machine_buttons(&name, &target, state).into_iter().unzip();
        let detail = MachineDetailView {
            name,
            dot,
            facts,
            problem: failure.map(Failure::why),
            how,
            runs,
            no_runs: match up {
                true => "Nothing of this gerdr's features runs there".to_owned(),
                false => "Connect it to see what runs there".to_owned(),
            },
            buttons: labels,
        };
        Content {
            rows,
            detail: Some(detail),
            run_presses,
            buttons,
        }
    }

    /// The panel's content, and the view of it for its state now.
    fn machines_panel_parts(&self) -> Option<(Content, MachinesPanelView)> {
        let panel = self.machines_panel.as_ref()?;
        let selected = panel.selected.min(self.machines.saved.len());
        let mut content = self.machines_content(selected);
        let focus = match content.stops().contains(&panel.focus) {
            true => panel.focus,
            false => MachinesFocus::List,
        };
        let armed = (panel.armed.as_deref()).is_some_and(|a| content.name(selected) == Some(a));
        if let Some(row) = content.rows.get_mut(selected) {
            row.armed = armed;
        }
        let runs = content.run_presses.len();
        let view = MachinesPanelView {
            rows: content.rows.clone(),
            selected,
            first: panel.first.min(content.add_row()),
            detail: content.detail.clone(),
            focus,
            runs_first: panel.runs_first.min(runs.saturating_sub(1)),
            hover: panel.hover,
            hints: hints(&content, selected, focus, armed),
        };
        Some((content, view))
    }

    pub fn machines_panel_view(&self) -> Option<MachinesPanelView> {
        self.machines_panel_parts().map(|(_, view)| view)
    }

    /// How many list rows and runs show at once.
    fn machines_pages(&self, view: &MachinesPanelView) -> (usize, usize) {
        let output = self.output_size();
        let list = chrome::machines_shown(view.rows.len() + 1, output);
        (list.max(1), chrome::machine_runs_shown(view, output).max(1))
    }

    /// A key while the machines panel has the keyboard: Up/Down (k/j,
    /// Ctrl+K/J), PgUp/PgDn, Home/End move through the machines; Tab/Shift+Tab
    /// through the runs' buttons, then the machine's; Enter presses the
    /// focused button; Del arms, and again removes; Esc closes.
    pub fn machines_panel_key(&mut self, mods: &ModifiersState, sym: Keysym) -> Option<Outcome> {
        let (content, view) = self.machines_panel_parts()?;
        let pages = self.machines_pages(&view);
        self.machines_panel
            .as_mut()?
            .key(&content, mods, sym, pages)
    }

    /// Do what the machines panel asked. What opens a tab closes it first;
    /// the rest leaves it open, to see what it did.
    pub fn apply_machines_panel(&mut self, outcome: Outcome) {
        let press = match outcome {
            Outcome::Close => return self.close_machines(),
            Outcome::Press(press) => press,
        };
        match press {
            Press::Add => self.open_machine_dialog(),
            Press::Connect(name) => self.connect_machine(&name),
            Press::Disconnect(name) => self.disconnect_machine(&name),
            // Asks when it holds sandboxes; else gone at once, with Undo.
            Press::Remove(name) => self.ask_remove_machine(&name),
            Press::Fix(name) => {
                let link = self.machines.links.get(&name);
                let fix = link.and_then(|l| link_failure(Some(&l.state))?.fix(l.target()));
                let Some(fix) = fix else {
                    return self.show_warning(format!("Nothing to fix on {name} now"));
                };
                if matches!(fix, crate::machines::Fix::Terminal(_)) {
                    self.close_machines();
                }
                self.run_fix(fix);
            }
            Press::Show(run) => {
                self.close_machines();
                match run {
                    RunRef::Agent(project, slug, stage) => {
                        self.show_stage_agent(&project, &slug, stage);
                    }
                    RunRef::Checks(project, slug) => self.show_checks_of(&project, &slug),
                    RunRef::Sandbox(project, slug) => self.show_sandbox_terminals(&project, &slug),
                    RunRef::Left(_) => {}
                }
            }
            Press::Stop(RunRef::Agent(project, slug, stage)) => {
                // A change: what another machine writes moves here first, as
                // from its page, and the press goes on there.
                if self.feature_writer(&project, &slug).is_some() {
                    self.close_machines();
                    self.open_feature(&project, &slug);
                    let press = crate::features::PanelOutcome::StopOnMachine(stage);
                    return self.take_feature(&project, &slug, Some(press));
                }
                self.ask_stop_on_machine(&project, &slug, stage);
            }
            // A sandbox stops from any gerdr showing it, as from its page.
            Press::Stop(RunRef::Sandbox(project, slug)) => self.ask_stop_sandbox(&project, &slug),
            Press::Stop(RunRef::Left(id)) => {
                let left = self.machines.left.get(&Some(id)).cloned();
                if let Some(left) = left.filter(|l| !l.is_empty()) {
                    self.ask_remove_sandbox_leftovers(left);
                }
            }
            // Never offered: checks stop on their page, where what they ran
            // shows.
            Press::Stop(RunRef::Checks(..)) => {}
            Press::OpenFeature(project, slug) => {
                self.close_machines();
                self.open_feature(&project, &slug);
            }
        }
    }

    /// Show the terminal of feature `slug`'s checks, the one still running
    /// first, or of its setup.
    fn show_checks_of(&mut self, project: &str, slug: &str) {
        let Some(f) = self.feature(project, slug) else {
            return self.show_warning("That feature is gone".into());
        };
        let checks = f.verify.as_ref().map_or(&[][..], |v| v.checks.as_slice());
        let check = (checks
            .iter()
            .position(|c| c.pane.is_some() && c.outcome.is_none()))
        .or_else(|| checks.iter().position(|c| c.pane.is_some()));
        let setup = f.setup.as_ref().and_then(|s| s.pane.clone());
        match (check, setup) {
            (Some(index), _) => self.show_check(project, slug, index),
            (None, Some(pane)) if crate::machine_work::split_pane(&pane).is_some() => {
                if !self.show_machine_pane(&pane) {
                    self.open_setup_log(project, slug);
                }
            }
            (None, _) => self.open_setup_log(project, slug),
        }
    }

    /// A left click while the machines panel is open: off it closes it, on a
    /// row selects it (the add row adds), on a button presses it, on a key
    /// hint presses its key.
    pub fn machines_panel_click(&mut self, pos: Point<f64, Logical>) {
        let Some((content, view)) = self.machines_panel_parts() else {
            return;
        };
        let hit = chrome::machines_layout(&view, self.output_size()).hit(pos);
        let outcome = match hit {
            None => Some(Outcome::Close),
            Some(MachinesHit::Panel | MachinesHit::Run(_)) => None,
            Some(MachinesHit::Hint(i)) => {
                if let Some((keys, _)) = view.hints.get(i) {
                    self.press_hint(crate::input::Overlay::MachinesPanel, keys, pos);
                }
                None
            }
            Some(MachinesHit::Row(i)) => {
                if let Some(panel) = self.machines_panel.as_mut() {
                    panel.select(i.min(content.add_row()));
                }
                (i >= content.add_row()).then_some(Outcome::Press(Press::Add))
            }
            Some(MachinesHit::RunButton(r, b)) => {
                let focus = MachinesFocus::Run(r, b);
                if let Some(panel) = self.machines_panel.as_mut() {
                    panel.focus = focus;
                }
                content.press(view.selected, focus).map(Outcome::Press)
            }
            Some(MachinesHit::Button(b)) => {
                let focus = MachinesFocus::Button(b);
                if let Some(panel) = self.machines_panel.as_mut() {
                    panel.focus = focus;
                }
                content.press(view.selected, focus).map(Outcome::Press)
            }
        };
        if let Some(outcome) = outcome {
            self.apply_machines_panel(outcome);
        }
    }

    /// The pointer moved while the machines panel is open.
    pub fn machines_panel_hover(&mut self, pos: Point<f64, Logical>) {
        let hit = (self.machines_panel_view())
            .and_then(|view| chrome::machines_layout(&view, self.output_size()).hit(pos));
        if let Some(panel) = self.machines_panel.as_mut() {
            panel.hover = hit;
        }
    }

    /// The wheel or touchpad over the machines panel: `rows` rows on, in the
    /// runs when the pointer is on them (see `MachinesPanel::hover`), else
    /// in the list.
    pub fn machines_panel_scroll(&mut self, rows: i32) {
        let Some(view) = self.machines_panel_view() else {
            return;
        };
        let (list_page, runs_page) = self.machines_pages(&view);
        let on_runs = matches!(
            view.hover,
            Some(MachinesHit::Run(_) | MachinesHit::RunButton(..))
        );
        let (count, page) = match on_runs {
            true => (view.detail.as_ref().map_or(0, |d| d.runs.len()), runs_page),
            false => (view.rows.len() + 1, list_page),
        };
        if let Some(panel) = self.machines_panel.as_mut() {
            panel.scroll(rows, on_runs, count, page);
        }
    }

    /// The list rows of the machines panel, as laid out now: for how far a
    /// notch of the wheel goes.
    pub fn machines_panel_rows(&self) -> Vec<chrome::Rect> {
        (self.machines_panel_view()).map_or_else(Vec::new, |v| {
            chrome::machines_layout(&v, self.output_size()).rows
        })
    }

    /// What the pointer can press in the machines panel, while it is open.
    pub fn machines_panel_controls(&self) -> Vec<chrome::Rect> {
        (self.machines_panel_view()).map_or_else(Vec::new, |v| {
            chrome::machines_layout(&v, self.output_size()).controls()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machines::{LinkInfo, Site};

    fn up() -> LinkState {
        LinkState::Up(LinkInfo {
            site: Site {
                version: "0.9.3".into(),
                id: "0123456789abcdef".into(),
                ..Site::default()
            },
            socket: "/s".into(),
            control: "/c".into(),
        })
    }

    fn row(name: &str) -> MachineRowView {
        MachineRowView {
            name: name.into(),
            address: format!("me@{name}"),
            state: "off".into(),
            dot: Dot::Idle,
            armed: false,
        }
    }

    /// Two machines, the first selected, connected, with one live agent.
    fn content() -> Content {
        let agent = RunRef::Agent("p".into(), "f".into(), Stage::Build);
        let title = "f · Build agent".to_owned();
        let (run, presses) = run_entry(&agent, true, title, String::new(), Dot::Working);
        let (labels, buttons) = machine_buttons("a", "me@a", Some(&up()))
            .into_iter()
            .unzip();
        Content {
            rows: vec![row("a"), row("b")],
            detail: Some(MachineDetailView {
                name: "a".into(),
                dot: Dot::Done,
                facts: Vec::new(),
                problem: None,
                how: None,
                runs: vec![run],
                no_runs: String::new(),
                buttons: labels,
            }),
            run_presses: vec![presses],
            buttons,
        }
    }

    /// The add row's content, past machines `a` and `b`.
    fn on_add_row(c: &Content) -> Content {
        Content {
            rows: c.rows.clone(),
            ..Content::default()
        }
    }

    fn key(panel: &mut MachinesPanel, c: &Content, sym: Keysym, shift: bool) -> Option<Outcome> {
        let mods = ModifiersState {
            shift,
            ..ModifiersState::default()
        };
        panel.key(c, &mods, sym, (10, 10))
    }

    #[test]
    fn a_rows_state_follows_its_link() {
        assert_eq!(link_chip(Some(&up())), ("connected", Dot::Done));
        let connecting = LinkState::Connecting("x".into());
        assert_eq!(link_chip(Some(&connecting)), ("connecting", Dot::Working));
        let away = LinkState::Retrying(Failure::TimedOut);
        assert_eq!(link_chip(Some(&away)), ("away", Dot::Blocked));
        let failed = LinkState::Failed(Failure::Auth);
        assert_eq!(link_chip(Some(&failed)), ("can't connect", Dot::Failed));
        assert_eq!(link_chip(Some(&LinkState::Off)), ("off", Dot::Idle));
        assert_eq!(link_chip(None), ("off", Dot::Idle));
    }

    #[test]
    fn the_primary_button_comes_last() {
        let labels = |state: Option<&LinkState>| -> Vec<String> {
            (machine_buttons("a", "me@a", state).into_iter())
                .map(|(l, _)| l)
                .collect()
        };
        assert_eq!(labels(Some(&up())), ["Remove", "Disconnect"]);
        assert_eq!(labels(None), ["Remove", "Connect"]);
        let auth = LinkState::Failed(Failure::Auth);
        assert_eq!(labels(Some(&auth)), ["Remove", "Copy command", "Connect"]);
        let away = LinkState::Retrying(Failure::TimedOut);
        assert_eq!(labels(Some(&away)), ["Remove", "Disconnect"]);
        let presses: Vec<Press> = (machine_buttons("a", "me@a", Some(&auth)).into_iter())
            .map(|(_, p)| p)
            .collect();
        assert_eq!(
            presses,
            [
                Press::Remove("a".into()),
                Press::Fix("a".into()),
                Press::Connect("a".into())
            ]
        );
    }

    #[test]
    fn a_runs_buttons_press_what_they_say() {
        let cases = [
            (RunRef::Agent("p".into(), "f".into(), Stage::Plan), true),
            (RunRef::Agent("p".into(), "f".into(), Stage::Plan), false),
            (RunRef::Checks("p".into(), "f".into()), true),
            (RunRef::Sandbox("p".into(), "f".into()), true),
            (RunRef::Left("id".into()), true),
        ];
        for (run, live) in cases {
            let (view, presses) = run_entry(&run, live, "t".into(), "d".into(), Dot::Idle);
            assert_eq!(view.buttons.len(), presses.len());
            for (label, press) in view.buttons.iter().zip(&presses) {
                let fits = match label.as_str() {
                    "Show" => *press == Press::Show(run.clone()),
                    "Stop" | "Remove" => *press == Press::Stop(run.clone()),
                    "Open feature" => *press == Press::OpenFeature("p".into(), "f".into()),
                    _ => false,
                };
                assert!(fits, "{label} presses {press:?}");
            }
        }
        let starting = RunRef::Agent("p".into(), "f".into(), Stage::Plan);
        let labels: Vec<_> = (run_buttons(&starting, false).into_iter())
            .map(|(l, _)| l)
            .collect();
        assert_eq!(labels, ["Open feature"]);
    }

    #[test]
    fn facts_say_how_long_it_is_up() {
        let load = Some(Load {
            average: 1.25,
            cores: 8,
        });
        let facts = machine_facts(
            "me@a",
            Some(&up()),
            Some(Duration::from_secs(7200)),
            None,
            load,
            Some("0123456789abcdef"),
        );
        assert_eq!(
            facts,
            [
                "me@a",
                "up 2h 0m · herdr 0.9.3 · load 1.2 / 8",
                "id 01234567"
            ]
        );
        let away = machine_facts(
            "me@a",
            None,
            None,
            Some(Duration::from_secs(300)),
            None,
            None,
        );
        assert_eq!(away, ["me@a", "last seen 5m ago"]);
    }

    #[test]
    fn tab_goes_through_the_runs_then_the_machine_and_back() {
        let c = content();
        let mut panel = MachinesPanel::default();
        let mut seen = Vec::new();
        for _ in 0..c.stops().len() {
            key(&mut panel, &c, Keysym::Tab, false);
            seen.push(panel.focus);
        }
        assert_eq!(
            seen,
            [
                MachinesFocus::Run(0, 0),
                MachinesFocus::Run(0, 1),
                MachinesFocus::Run(0, 2),
                MachinesFocus::Button(0),
                MachinesFocus::Button(1),
                MachinesFocus::List,
            ]
        );
        key(&mut panel, &c, Keysym::ISO_Left_Tab, true);
        assert_eq!(panel.focus, MachinesFocus::Button(1));
        let pressed = key(&mut panel, &c, Keysym::Return, false);
        assert_eq!(pressed, Some(Outcome::Press(Press::Disconnect("a".into()))));
        for _ in 0..3 {
            key(&mut panel, &c, Keysym::Tab, true);
        }
        let stop = key(&mut panel, &c, Keysym::Return, false);
        let agent = RunRef::Agent("p".into(), "f".into(), Stage::Build);
        assert_eq!(stop, Some(Outcome::Press(Press::Stop(agent))));
    }

    #[test]
    fn enter_on_the_list_presses_the_primary_button_or_adds() {
        let c = content();
        let mut panel = MachinesPanel::default();
        let enter = key(&mut panel, &c, Keysym::Return, false);
        assert_eq!(enter, Some(Outcome::Press(Press::Disconnect("a".into()))));
        key(&mut panel, &c, Keysym::End, false);
        assert_eq!(panel.selected, 2);
        let enter = key(&mut panel, &on_add_row(&c), Keysym::Return, false);
        assert_eq!(enter, Some(Outcome::Press(Press::Add)));
    }

    #[test]
    fn del_arms_then_removes_and_any_other_key_disarms() {
        let c = content();
        let mut panel = MachinesPanel::default();
        assert_eq!(key(&mut panel, &c, Keysym::Delete, false), None);
        assert_eq!(panel.armed.as_deref(), Some("a"));
        key(&mut panel, &c, Keysym::Tab, false);
        assert_eq!(panel.armed, None);
        key(&mut panel, &c, Keysym::Delete, false);
        let removed = key(&mut panel, &c, Keysym::Delete, false);
        assert_eq!(removed, Some(Outcome::Press(Press::Remove("a".into()))));
        // On the add row there is nothing to remove.
        let mut panel = MachinesPanel {
            selected: 2,
            ..MachinesPanel::default()
        };
        assert_eq!(
            key(&mut panel, &on_add_row(&c), Keysym::Delete, false),
            None
        );
        assert_eq!(panel.armed, None);
    }

    #[test]
    fn moving_puts_the_keyboard_back_on_the_list() {
        let c = content();
        let mut panel = MachinesPanel::default();
        key(&mut panel, &c, Keysym::Tab, false);
        key(&mut panel, &c, Keysym::Delete, false);
        key(&mut panel, &c, Keysym::j, false);
        assert_eq!((panel.selected, panel.focus), (1, MachinesFocus::List));
        assert_eq!(panel.armed, None);
        key(&mut panel, &c, Keysym::k, false);
        assert_eq!(panel.selected, 0);
    }

    #[test]
    fn the_selection_stays_on_what_is_there() {
        let c = Content {
            rows: vec![row("a")],
            ..Content::default()
        };
        let mut panel = MachinesPanel {
            selected: 5,
            focus: MachinesFocus::Button(3),
            armed: Some("gone".into()),
            ..MachinesPanel::default()
        };
        panel.settle(&c);
        assert_eq!(panel.selected, 1);
        assert_eq!(panel.focus, MachinesFocus::List);
        assert_eq!(panel.armed, None);
    }

    #[test]
    fn hints_say_what_enter_and_del_do() {
        let c = content();
        let keys = |h: Vec<(String, String)>| -> Vec<String> {
            h.into_iter().map(|(k, w)| format!("{k}: {w}")).collect()
        };
        assert_eq!(
            keys(hints(&c, 0, MachinesFocus::List, false)),
            [
                "Up / Down: move",
                "Tab: next button",
                "Enter: disconnect",
                "Del: remove",
                "Esc: close"
            ]
        );
        let armed = keys(hints(&c, 0, MachinesFocus::Run(0, 0), true));
        assert!(armed.contains(&"Enter: show".to_owned()));
        assert!(armed.contains(&"Del: remove now".to_owned()));
        assert_eq!(
            keys(hints(&on_add_row(&c), 2, MachinesFocus::List, false)),
            ["Up / Down: move", "Enter: add machine", "Esc: close"]
        );
    }

    #[test]
    fn a_row_far_down_is_scrolled_to() {
        assert_eq!(reveal(0, 7, 5, 10), 3);
        assert_eq!(reveal(4, 2, 5, 10), 2);
        assert_eq!(reveal(9, 9, 5, 10), 5);
        let mut panel = MachinesPanel::default();
        panel.scroll(3, false, 10, 5);
        assert_eq!(panel.first, 3);
        panel.scroll(9, false, 10, 5);
        assert_eq!(panel.first, 5);
        panel.scroll(-9, true, 4, 2);
        assert_eq!(panel.runs_first, 0);
    }
}

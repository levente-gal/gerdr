//! The machines panel: every machine saved, how its link stands, and what of
//! this gerdr's features runs there, with buttons to connect, disconnect,
//! remove or fix a machine and to show or stop what runs on it (see
//! `chrome::machines_panel`). It opens with the prefix, then O, from the
//! launcher, and from a machine's chip in the footer.
//!
//! Up/Down move through the machines, Tab/Shift+Tab through the buttons of
//! the one selected, Enter presses the focused button (on the list: the
//! machine's primary one), Del arms and then removes the machine, Ctrl+E (F2:
//! on the name) edits it in a form in place of its detail (see
//! `machine_edit`), Esc closes. A machine that is another saved one too (an
//! id both were seen by) says so, with a button merging that one into it.

use std::{collections::HashMap, time::Duration};

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point},
};

use crate::{
    Gerdr,
    chrome::{
        self, Dot, MachineDetailView, MachineEditView, MachineRowView, MachineRunView,
        MachinesFocus, MachinesHit, MachinesPanelView, MachinesTab,
    },
    feature::{Feature, Stage},
    herdr::AgentStatus,
    machine_edit::{Draft, EditForm},
    machine_load::Load,
    machines::{Failure, LinkState},
    tasks::uptime,
    text_field::{Edit, TextField},
};

/// The machines panel's state, while it is open.
#[derive(Debug, Default)]
pub struct MachinesPanel {
    /// The page shown (Ctrl+PgUp / Ctrl+PgDn, or a tab in the title row).
    pub tab: MachinesTab,
    /// The list row selected: past the machines is the add row.
    pub selected: usize,
    pub first: usize,
    pub focus: MachinesFocus,
    pub runs_first: usize,
    pub hover: Option<MachinesHit>,
    /// The machine Del was pressed on once: a second Del removes it.
    pub armed: Option<String>,
    /// The form editing the selected machine, while it is open.
    pub edit: Option<EditForm>,
    /// What was typed in the form of each machine and left unsaved, by its
    /// name: name, addresses. Kept while the panel is open.
    pub drafts: HashMap<String, Draft>,
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
    /// Open the form editing the machine of this name (Ctrl+E, its Edit
    /// button; F2 opens it on the name).
    Edit(String),
    /// The form's buttons: save (trying a new address first), save without
    /// trying, stop trying, leave the form, press the fix of what failed.
    Save,
    SaveAnyway,
    StopTrying,
    CancelEdit,
    EditFix,
    /// A new empty address field at the end of the form's list ("+ Add
    /// address").
    AddAddress,
    /// Take address field `.0` out of the form's list (its ×).
    RemoveAddress(usize),
    /// Machine `drop` is the same machine as `keep` (the same gerdr machine
    /// id): its addresses join `keep`'s, after them, and `drop` goes, all it
    /// was for moving to `keep`.
    Merge {
        keep: String,
        drop: String,
    },
    /// On the Placement page: the open project's `stage` goes to the next
    /// machine, or the one before with `back`.
    Place { stage: Stage, back: bool },
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
    /// Removing the selected machine asks first, as sandboxes this gerdr
    /// answers for stay on it: Del asks at once, without arming.
    asks_remove: bool,
    /// The edit form, while it is open on the selected machine.
    edit: Option<EditContent>,
}

/// What the edit form's buttons press, and how it stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct EditContent {
    /// As `MachineEditView::buttons` has them.
    buttons: Vec<Press>,
    /// How many address fields it has.
    addresses: usize,
    /// Its new address is tried: only Stop works.
    busy: bool,
    /// The last try failed with a fix to press.
    fix: bool,
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
    /// then the machine's buttons. In the edit form: its fields, then its
    /// buttons; while it tries, its buttons only.
    fn stops(&self) -> Vec<MachinesFocus> {
        if let Some(edit) = &self.edit {
            let buttons = (0..edit.buttons.len()).map(MachinesFocus::EditButton);
            if edit.busy {
                return buttons.collect();
            }
            return self.fields().into_iter().chain(buttons).collect();
        }
        let runs = (self.run_presses.iter().enumerate())
            .flat_map(|(r, presses)| (0..presses.len()).map(move |b| MachinesFocus::Run(r, b)));
        let buttons = (0..self.buttons.len()).map(MachinesFocus::Button);
        std::iter::once(MachinesFocus::List)
            .chain(runs)
            .chain(buttons)
            .collect()
    }

    /// The edit form's fields, top down: its name, then each address.
    fn fields(&self) -> Vec<MachinesFocus> {
        let addresses = self.edit.as_ref().map_or(0, |e| e.addresses);
        std::iter::once(MachinesFocus::EditName)
            .chain((0..addresses).map(MachinesFocus::EditAddress))
            .collect()
    }

    /// Where the keyboard goes when it is on nothing there: the list, the
    /// form's address, or Stop while it tries.
    fn home(&self) -> MachinesFocus {
        match &self.edit {
            Some(edit) if edit.busy => MachinesFocus::EditButton(0),
            Some(_) => MachinesFocus::EditAddress(0),
            None => MachinesFocus::List,
        }
    }

    /// What focus `focus` presses on list row `selected`: on the list, the
    /// machine's primary button, or Add on the add row; on a field of the
    /// form, its primary button.
    fn press(&self, selected: usize, focus: MachinesFocus) -> Option<Press> {
        match focus {
            MachinesFocus::List if selected >= self.add_row() => Some(Press::Add),
            MachinesFocus::List => self.buttons.last().cloned(),
            MachinesFocus::Run(r, b) => self.run_presses.get(r)?.get(b).cloned(),
            MachinesFocus::Button(b) => self.buttons.get(b).cloned(),
            MachinesFocus::EditName | MachinesFocus::EditAddress(_) => self
                .edit
                .as_ref()
                .filter(|e| !e.busy)?
                .buttons
                .last()
                .cloned(),
            MachinesFocus::EditButton(b) => self.edit.as_ref()?.buttons.get(b).cloned(),
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
            MachinesFocus::EditName | MachinesFocus::EditAddress(_) => {
                let edit = detail?.edit.as_ref().filter(|e| !e.busy)?;
                edit.buttons.last().cloned()
            }
            MachinesFocus::EditButton(b) => detail?.edit.as_ref()?.buttons.get(b).cloned(),
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

/// The buttons of machine `name` at `target`, its link in `state`, the
/// machines `same` being it too, left to right: the last is the primary one.
fn machine_buttons(
    name: &str,
    target: &str,
    state: Option<&LinkState>,
    same: &[String],
) -> Vec<(String, Press)> {
    let mut buttons: Vec<(String, Press)> = (same.iter())
        .map(|other| {
            let press = Press::Merge {
                keep: name.to_owned(),
                drop: other.clone(),
            };
            (format!("Merge {other}"), press)
        })
        .collect();
    buttons.push(("Remove".to_owned(), Press::Remove(name.to_owned())));
    buttons.push(("Edit".to_owned(), Press::Edit(name.to_owned())));
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

/// The edit form `form` as it shows, and what its buttons press; `trying`:
/// how far trying its new address got, while it is tried.
fn edit_parts(form: &EditForm, trying: Option<&str>) -> (MachineEditView, EditContent) {
    let busy = trying.is_some();
    let fix = form.fix().filter(|_| !busy);
    let buttons: Vec<(String, Press)> = match busy {
        true => vec![("Stop".to_owned(), Press::StopTrying)],
        false => {
            let mut buttons = vec![("Cancel".to_owned(), Press::CancelEdit)];
            if let Some(fix) = &fix {
                buttons.push((fix.label().to_owned(), Press::EditFix));
            }
            if form.untried_offered() {
                buttons.push(("Save anyway".to_owned(), Press::SaveAnyway));
            }
            let save = match form.failed.is_some() {
                true => "Try again",
                false => "Save",
            };
            buttons.push((save.to_owned(), Press::Save));
            buttons
        }
    };
    let (error, how) = match (&form.error, &form.failed) {
        (Some(error), _) => (Some(error.clone()), None),
        (None, Some((target, failure))) => (Some(failure.why()), failure.how(target)),
        (None, None) => (None, None),
    };
    let (labels, presses) = buttons.into_iter().unzip();
    let view = MachineEditView {
        name: form.name.line(),
        addresses: form.addresses.iter().map(TextField::line).collect(),
        error,
        how,
        progress: trying.map(str::to_owned),
        buttons: labels,
        busy,
    };
    let content = EditContent {
        buttons: presses,
        addresses: form.addresses.len(),
        busy,
        fix: fix.is_some(),
    };
    (view, content)
}

/// Where a stage goes after `now` (none: this machine), among this
/// machine, the machines named `names` in their order, and least busy;
/// the one before with `back`.
fn next_place(names: &[String], now: Option<&str>, back: bool) -> Option<String> {
    use crate::machine_load::LEAST_BUSY;
    let mut order: Vec<Option<&str>> = vec![None];
    order.extend(names.iter().map(|n| Some(n.as_str())));
    order.push(Some(LEAST_BUSY));
    // A machine no longer saved goes on from this one.
    let at = order.iter().position(|o| *o == now).unwrap_or(0);
    let next = match back {
        true => (at + order.len() - 1) % order.len(),
        false => (at + 1) % order.len(),
    };
    order[next].map(str::to_owned)
}

/// The Placement page's detail: the project `name`, its stages `runs`.
fn placement_detail(name: String, runs: Vec<MachineRunView>, no_runs: &str) -> MachineDetailView {
    MachineDetailView {
        name,
        dot: Dot::Idle,
        facts: vec![
            "Where each stage runs when a feature doesn't choose its own (Ctrl+Shift+W on its page)"
                .to_owned(),
        ],
        problem: None,
        how: None,
        runs,
        no_runs: no_runs.to_owned(),
        buttons: Vec::new(),
        edit: None,
    }
}

/// The Placement page's key hints.
fn placement_hints() -> Vec<(String, String)> {
    [
        ("Up / Down", "move"),
        ("Left / Right", "machine"),
        ("Ctrl+PgUp", "machines"),
        ("Esc", "close"),
    ]
    .map(|(k, w)| (k.to_owned(), w.to_owned()))
    .to_vec()
}

/// A key on the Placement page, the keyboard on `focus` of `content`: Up /
/// Down go between the stages, Right or Enter gives the selected one the
/// next machine, Left the one before, Esc closes.
fn placement_key(
    content: &Content,
    focus: MachinesFocus,
    panel: &mut MachinesPanel,
    mods: &ModifiersState,
    sym: Keysym,
) -> Option<Outcome> {
    let plain = !mods.ctrl && !mods.alt && !mods.logo;
    let rows = content.run_presses.len();
    let row = match focus {
        MachinesFocus::Run(r, _) => r,
        _ => 0,
    };
    let stage = |back| match content.run_presses.get(row)?.first()? {
        Press::Place { stage, .. } => Some(Outcome::Press(Press::Place { stage: *stage, back })),
        _ => None,
    };
    match sym {
        Keysym::Escape => return Some(Outcome::Close),
        Keysym::Return | Keysym::KP_Enter | Keysym::Right if plain => return stage(false),
        Keysym::Left if plain => return stage(true),
        Keysym::Up | Keysym::k if rows > 0 => panel.focus = MachinesFocus::Run(row.saturating_sub(1), 0),
        Keysym::Down | Keysym::j if rows > 0 => panel.focus = MachinesFocus::Run((row + 1).min(rows - 1), 0),
        _ => {}
    }
    None
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
    if let Some(edit) = &content.edit {
        if edit.busy {
            return vec![hint("Esc", "stop")];
        }
        let mut hints = vec![hint("Tab", "next")];
        if let Some(label) = content.label(selected, focus) {
            hints.push(hint("Enter", &label.to_lowercase()));
        }
        hints.push(hint("Ctrl+N", "add address"));
        if edit.fix {
            hints.push(hint("Alt+X", "fix"));
        }
        hints.push(hint("Esc", "cancel"));
        return hints;
    }
    let mut hints = vec![hint("Up / Down", "move")];
    if content.stops().len() > 1 {
        hints.push(hint("Tab", "next button"));
    }
    if let Some(label) = content.label(selected, focus) {
        hints.push(hint("Enter", &label.to_lowercase()));
    }
    if content.name(selected).is_some() {
        let what = match (armed, content.asks_remove) {
            (true, _) => "remove now",
            (false, true) => "remove\u{2026}",
            (false, false) => "remove",
        };
        hints.push(hint("Del", what));
        hints.push(hint("Ctrl+E", "edit"));
    }
    hints.push(hint("Ctrl+PgDn", "placement"));
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
    /// Show page `tab`, the keyboard on its first stop.
    fn show_tab(&mut self, tab: MachinesTab) {
        self.tab = tab;
        self.focus = MachinesFocus::List;
        self.runs_first = 0;
        self.armed = None;
    }

    /// Keep what it points at in `content`: a machine gone moves the
    /// selection back, a button gone the keyboard to the list.
    fn settle(&mut self, content: &Content) {
        self.selected = self.selected.min(content.add_row());
        // A form whose machine went from under it closes, what was typed kept.
        if content.edit.is_none() && self.edit.is_some() {
            self.close_edit();
        }
        if !content.stops().contains(&self.focus) {
            self.focus = content.home();
        }
        if self.armed.as_deref() != content.name(self.selected) {
            self.armed = None;
        }
        let runs = content.run_presses.len();
        self.runs_first = self.runs_first.min(runs.saturating_sub(1));
    }

    /// Select list row `row`: the keyboard back on the list, Del unarmed.
    /// Another row closes the form, what was typed kept.
    fn select(&mut self, row: usize) {
        if row != self.selected {
            self.close_edit();
            self.selected = row;
            self.runs_first = 0;
        }
        if self.edit.is_none() {
            self.focus = MachinesFocus::List;
        }
        self.armed = None;
    }

    /// Open the form on machine `machine`, saved at `addresses`: with what
    /// was typed there before, the keyboard on its name with `on_name`, else
    /// on its first address.
    fn open_edit(&mut self, machine: &str, addresses: &[String], on_name: bool) {
        if (self.edit.as_ref()).is_none_or(|f| f.machine != machine) {
            self.close_edit();
            let from = (machine.to_owned(), addresses.to_vec());
            let typed = self.drafts.remove(machine).unwrap_or_else(|| from.clone());
            let form = EditForm::new(machine, (&typed.0, &typed.1), from);
            self.edit = Some(form);
        }
        self.focus = match on_name {
            true => MachinesFocus::EditName,
            false => MachinesFocus::EditAddress(0),
        };
        self.armed = None;
    }

    /// Close the form, keeping what was typed for when it opens again on its
    /// machine; the keyboard back on the list.
    fn close_edit(&mut self) {
        if let Some(form) = self.edit.take() {
            match form.draft() {
                Some(draft) => _ = self.drafts.insert(form.machine, draft),
                None => _ = self.drafts.remove(&form.machine),
            }
            self.focus = MachinesFocus::List;
        }
    }

    /// Machine `drop` merged into the one on list row `keep_row`: its form
    /// and draft go with it, and the one kept is selected, `page` rows of
    /// `count` showing.
    pub(crate) fn merged(&mut self, drop: &str, keep_row: usize, (page, count): (usize, usize)) {
        self.drafts.remove(drop);
        if self.edit.as_ref().is_some_and(|f| f.machine == drop) {
            self.edit = None;
        }
        self.close_edit();
        self.focus = MachinesFocus::List;
        self.selected = keep_row;
        self.first = reveal(self.first, keep_row, page, count);
        self.armed = None;
    }

    /// A new empty address field at the end of the form, the keyboard on it.
    fn add_address(&mut self) {
        if let Some(form) = self.edit.as_mut() {
            self.focus = MachinesFocus::EditAddress(form.add_address());
        }
    }

    /// Take address field `i` out of the form, unless it is the last: the
    /// keyboard stays on the field it was on, or goes to the one now there.
    fn remove_address(&mut self, i: usize) {
        let Some(form) = self.edit.as_mut() else {
            return;
        };
        if !form.remove_address(i) {
            return;
        }
        let last = form.addresses.len() - 1;
        if let MachinesFocus::EditAddress(at) = self.focus {
            self.focus = MachinesFocus::EditAddress(match at > i {
                true => at - 1,
                false => at.min(last),
            });
        }
    }

    /// Move the focused address one place up, or down with `down`: it is
    /// tried in that order. The keyboard goes with it.
    fn move_address(&mut self, down: bool) {
        let (Some(form), MachinesFocus::EditAddress(at)) = (self.edit.as_mut(), self.focus) else {
            return;
        };
        if let Some(to) = form.move_address(at, down) {
            self.focus = MachinesFocus::EditAddress(to);
        }
    }

    /// Machine `old` is called `new` now: its form and draft follow it.
    pub(crate) fn renamed(&mut self, old: &str, new: &str) {
        if let Some(draft) = self.drafts.remove(old) {
            self.drafts.insert(new.to_owned(), draft);
        }
        if let Some(form) = self.edit.as_mut().filter(|f| f.machine == old) {
            form.machine = new.to_owned();
        }
        self.armed = None;
    }

    /// A key while the edit form is open: Tab/Shift+Tab go round its fields
    /// and buttons, Up/Down (Ctrl+K/J) between its fields, Enter (Ctrl+Enter)
    /// saves or presses the focused button, Alt+X presses the fix, Esc stops
    /// trying or leaves the form. Ctrl+N adds an address, Ctrl+Shift+Up/Down
    /// moves the focused one up or down, Del on an empty one takes it out;
    /// the rest goes to the focused field. While it tries, only Stop works.
    fn edit_key(
        &mut self,
        content: &Content,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<Outcome> {
        let edit = content.edit.as_ref()?;
        let ctrl = mods.ctrl && !mods.alt && !mods.logo;
        let alt = mods.alt && !mods.ctrl && !mods.logo;
        let plain = !mods.ctrl && !mods.alt && !mods.logo && !mods.shift;
        // Up and Down go between the fields; from a button, to the last
        // address.
        let fields = content.fields();
        let step = |focus, down: bool| match fields.iter().position(|f| *f == focus) {
            Some(at) if down => fields[(at + 1).min(fields.len() - 1)],
            Some(at) => fields[at.saturating_sub(1)],
            None => fields[fields.len() - 1],
        };
        // The address focused, if it is empty: Del takes it out.
        let empty_at = match self.focus {
            MachinesFocus::EditAddress(at) => (self.edit.as_ref()?.addresses.get(at))
                .is_some_and(|a| a.is_empty())
                .then_some(at),
            _ => None,
        };
        match sym {
            Keysym::Escape if edit.busy => return Some(Outcome::Press(Press::StopTrying)),
            Keysym::Escape => return Some(Outcome::Press(Press::CancelEdit)),
            Keysym::Return | Keysym::KP_Enter if ctrl && !edit.busy => {
                return Some(Outcome::Press(Press::Save));
            }
            Keysym::Return | Keysym::KP_Enter => {
                return content.press(self.selected, self.focus).map(Outcome::Press);
            }
            Keysym::Tab | Keysym::ISO_Left_Tab => {
                let back = sym == Keysym::ISO_Left_Tab || mods.shift;
                let stops = content.stops();
                let at = stops.iter().position(|f| *f == self.focus).unwrap_or(0);
                let next = match back {
                    true => (at + stops.len() - 1) % stops.len(),
                    false => (at + 1) % stops.len(),
                };
                self.focus = stops[next];
            }
            // The fields wait while the address is tried, as it shows.
            _ if edit.busy => {}
            Keysym::x | Keysym::X if alt && edit.fix => {
                return Some(Outcome::Press(Press::EditFix));
            }
            Keysym::n if ctrl && !mods.shift => self.add_address(),
            Keysym::Up | Keysym::KP_Up if ctrl && mods.shift => self.move_address(false),
            Keysym::Down | Keysym::KP_Down if ctrl && mods.shift => self.move_address(true),
            Keysym::Up | Keysym::KP_Up => self.focus = step(self.focus, false),
            Keysym::k | Keysym::K if ctrl => self.focus = step(self.focus, false),
            Keysym::Down | Keysym::KP_Down => self.focus = step(self.focus, true),
            Keysym::j | Keysym::J if ctrl => self.focus = step(self.focus, true),
            Keysym::Delete | Keysym::KP_Delete if plain && empty_at.is_some() => {
                self.remove_address(empty_at?);
            }
            _ => {
                let form = self.edit.as_mut()?;
                let field = match self.focus {
                    MachinesFocus::EditName => &mut form.name,
                    MachinesFocus::EditAddress(at) => form.addresses.get_mut(at)?,
                    _ => return None,
                };
                if field.key(mods, sym, typed) == Edit::Changed {
                    form.error = None;
                }
            }
        }
        None
    }

    /// A key on `content`, `list_page` list rows and `runs_page` runs
    /// showing at once.
    fn key(
        &mut self,
        content: &Content,
        mods: &ModifiersState,
        (sym, typed): (Keysym, Keysym),
        (list_page, runs_page): (usize, usize),
    ) -> Option<Outcome> {
        self.settle(content);
        if content.edit.is_some() {
            self.armed = None;
            return self.edit_key(content, mods, sym, typed);
        }
        let row = self.selected;
        let last = content.add_row();
        let plain = !mods.ctrl && !mods.alt && !mods.logo;
        let ctrl = mods.ctrl && !mods.alt && !mods.logo;
        let move_key = plain || ctrl;
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
            Keysym::e | Keysym::E if ctrl => {
                let name = content.name(row)?;
                return Some(Outcome::Press(Press::Edit(name.to_owned())));
            }
            // On the name: the press opening the form finds the keyboard
            // there.
            Keysym::F2 if plain => {
                let name = content.name(row)?;
                self.focus = MachinesFocus::EditName;
                return Some(Outcome::Press(Press::Edit(name.to_owned())));
            }
            Keysym::Delete | Keysym::KP_Delete => {
                let name = content.name(row)?;
                if content.asks_remove || armed.as_deref() == Some(name) {
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

    /// Close the machines panel: a new address tried from its form stops
    /// being tried.
    pub fn close_machines(&mut self) {
        self.stop_edit_try();
        self.machines_panel = None;
        self.refresh_pointer_focus();
    }

    /// The ids machine `name` is known by: the one it said, connected, then
    /// those it said before.
    pub(crate) fn machine_ids(&self, name: &str) -> Vec<String> {
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
        // And those it answered as before, kept while it is away.
        for id in self.projects.machine_ids.get(name).into_iter().flatten() {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
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
                    address: link.map_or(saved.first(), |l| l.target()).to_owned(),
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
        let row_name = name.clone();
        let target = row.address.clone();
        let dot = row.dot;
        let link = self.machines.links.get(&name);
        let state = link.map(|l| &l.state);
        let ids = self.machine_ids(&name);
        let same = self.same_machines_as(&name);
        let mut facts = machine_facts(
            &target,
            state,
            link.and_then(|l| l.connected_since).map(|at| at.elapsed()),
            link.and_then(|l| l.last_seen).map(|at| at.elapsed()),
            link.and_then(|l| l.work.as_ref()).and_then(|w| w.load),
            ids.first().map(String::as_str),
        );
        // The addresses it isn't using, in the order they are tried.
        let others: Vec<&str> = (self.machines.saved.get(&name).into_iter())
            .flat_map(|a| a.0.iter().map(String::as_str))
            .filter(|a| *a != target)
            .collect();
        if !others.is_empty() {
            facts.insert(1, format!("Also: {}", others.join(", ")));
        }
        facts.extend(same.iter().map(|other| format!("Same machine as {other}")));
        let failure = link_failure(state);
        let how = failure.and_then(|f| match (f.how(&target), state) {
            (Some(how), _) => Some(how),
            (None, Some(LinkState::Retrying(_))) => Some("gerdr tries again by itself.".into()),
            (None, _) => None,
        });
        let up = matches!(state, Some(LinkState::Up(_)));
        let (runs, run_presses) = self.machine_runs(&ids).into_iter().unzip();
        let (labels, buttons) = (machine_buttons(&name, &target, state, &same).into_iter()).unzip();
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
            edit: None,
        };
        let asks_remove = !self.sandboxes_on(&row_name).is_empty();
        Content {
            rows,
            detail: Some(detail),
            run_presses,
            buttons,
            asks_remove,
            edit: None,
        }
    }

    /// The panel's content, and the view of it for its state now.
    fn machines_panel_parts(&self) -> Option<(Content, MachinesPanelView)> {
        let panel = self.machines_panel.as_ref()?;
        if panel.tab == MachinesTab::Placement {
            let content = self.placement_content();
            // Always on a stage: the page has no list to be on.
            let focus = match panel.focus {
                MachinesFocus::Run(..) if content.stops().contains(&panel.focus) => panel.focus,
                _ => (content.stops().get(1).copied()).unwrap_or(MachinesFocus::List),
            };
            let view = MachinesPanelView {
                tab: MachinesTab::Placement,
                rows: Vec::new(),
                selected: 0,
                first: 0,
                detail: content.detail.clone(),
                focus,
                runs_first: (panel.runs_first).min(content.run_presses.len().saturating_sub(1)),
                hover: panel.hover,
                hints: placement_hints(),
            };
            return Some((content, view));
        }
        let selected = panel.selected.min(self.machines.saved.len());
        let mut content = self.machines_content(selected);
        let form = (panel.edit.as_ref()).filter(|f| content.name(selected) == Some(&f.machine));
        if let (Some(form), Some(detail)) = (form, content.detail.as_mut()) {
            let trying = self.edit_try().filter(|t| t.name == form.machine);
            let (view, edit) = edit_parts(form, trying.map(|t| t.progress.as_str()));
            detail.edit = Some(view);
            content.edit = Some(edit);
        }
        let focus = match content.stops().contains(&panel.focus) {
            true => panel.focus,
            false => content.home(),
        };
        let armed = (panel.armed.as_deref()).is_some_and(|a| content.name(selected) == Some(a));
        if let Some(row) = content.rows.get_mut(selected) {
            row.armed = armed;
        }
        let runs = content.run_presses.len();
        let view = MachinesPanelView {
            tab: MachinesTab::Machines,
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
    pub fn machines_panel_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<Outcome> {
        let (content, view) = self.machines_panel_parts()?;
        let pages = self.machines_pages(&view);
        let panel = self.machines_panel.as_mut()?;
        let ctrl = mods.ctrl && !mods.alt && !mods.logo && !mods.shift;
        if ctrl && matches!(sym, Keysym::Page_Up | Keysym::Page_Down) {
            panel.show_tab(match panel.tab {
                MachinesTab::Machines => MachinesTab::Placement,
                MachinesTab::Placement => MachinesTab::Machines,
            });
            return None;
        }
        match view.tab {
            MachinesTab::Placement => placement_key(&content, view.focus, panel, mods, sym),
            MachinesTab::Machines => panel.key(&content, mods, (sym, typed), pages),
        }
    }

    /// Where each placeable stage of the open project runs, as rows of the
    /// detail: its stage, how many features choose their own, and a button
    /// naming its machine, which goes to the next.
    fn placement_content(&self) -> Content {
        use crate::{machine_load::LEAST_BUSY, machine_work::takes_a_place};
        let project = self.project.as_deref().and_then(|id| self.projects.project(id));
        let Some(project) = project else {
            return Content {
                detail: Some(placement_detail(
                    "No project open".to_owned(),
                    Vec::new(),
                    "Open a project to choose where its stages run",
                )),
                ..Content::default()
            };
        };
        let features = (self.features.all.get(&project.id)).map_or(&[][..], Vec::as_slice);
        let up = |name: &str| {
            (self.machines.links.get(name)).is_some_and(|l| matches!(l.state, LinkState::Up(_)))
        };
        let (mut runs, mut presses) = (Vec::new(), Vec::new());
        for stage in Stage::ALL.into_iter().filter(|s| takes_a_place(*s)) {
            let (label, dot) = match project.workflow.machines.get(&stage).map(String::as_str) {
                None => ("Runs here".to_owned(), Dot::Done),
                Some(LEAST_BUSY) => ("Least busy".to_owned(), Dot::Done),
                Some(name) if up(name) => (format!("Runs on {name}"), Dot::Done),
                Some(name) => (format!("Runs on {name}"), Dot::Blocked),
            };
            let own = (features.iter())
                .filter(|f| f.stage_machines.contains_key(&stage))
                .count();
            let detail = match own {
                0 => "Every feature follows this".to_owned(),
                1 => "1 feature chooses its own".to_owned(),
                n => format!("{n} features choose their own"),
            };
            runs.push(MachineRunView {
                title: stage.label().to_owned(),
                detail,
                dot,
                buttons: vec![label],
            });
            presses.push(vec![Press::Place { stage, back: false }]);
        }
        Content {
            detail: Some(placement_detail(project.name.clone(), runs, "")),
            run_presses: presses,
            ..Content::default()
        }
    }

    /// The open project's `stage` goes to the next machine (`back`: the one
    /// before): this one, each saved machine by name, then least busy. Kept
    /// in the project's file, under `workflow.machines`.
    fn place_stage(&mut self, stage: Stage, back: bool) {
        let Some(id) = self.project.clone() else {
            return;
        };
        let names: Vec<String> = self.machines.saved.keys().cloned().collect();
        let Some(project) = self.projects.projects.iter_mut().find(|p| p.id == id) else {
            return;
        };
        let machines = &mut project.workflow.machines;
        match next_place(&names, machines.get(&stage).map(String::as_str), back) {
            None => machines.remove(&stage),
            Some(to) => machines.insert(stage, to),
        };
        if !self.projects.save_projects(&[id]) {
            self.show_error("gerdr couldn't write the project's file".to_owned());
        }
    }

    /// Open the edit form on machine `name`, the keyboard on its name with
    /// `on_name`.
    fn open_edit(&mut self, name: &str, on_name: bool) {
        let Some(address) = self.machines.saved.get(name).cloned() else {
            return;
        };
        if let Some(panel) = self.machines_panel.as_mut() {
            panel.open_edit(name, &address.0, on_name);
        }
    }

    /// The edit form is done: closed with nothing kept of it, the selection
    /// on machine `name` when given, as it may have a new name.
    pub(crate) fn edit_closed(&mut self, name: Option<&str>) {
        let row = name.and_then(|name| self.machines.saved.keys().position(|n| n == name));
        let count = self.machines.saved.len() + 1;
        let page = chrome::machines_shown(count, self.output_size());
        let Some(panel) = self.machines_panel.as_mut() else {
            return;
        };
        if let Some(form) = panel.edit.take() {
            panel.drafts.remove(&form.machine);
        }
        if let Some(name) = name {
            panel.drafts.remove(name);
        }
        panel.focus = MachinesFocus::List;
        if let Some(row) = row {
            panel.selected = row;
            panel.first = reveal(panel.first, row, page, count);
        }
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
            Press::Edit(name) => {
                // F2 put the keyboard on the name already.
                let on_name = (self.machines_panel.as_ref())
                    .is_some_and(|p| p.focus == MachinesFocus::EditName);
                self.open_edit(&name, on_name);
            }
            Press::Save => self.save_edit(false),
            Press::SaveAnyway => self.save_edit(true),
            Press::StopTrying => self.stop_edit_try(),
            Press::CancelEdit => {
                self.stop_edit_try();
                if let Some(panel) = self.machines_panel.as_mut() {
                    panel.close_edit();
                }
            }
            Press::EditFix => {
                let form = self.machines_panel.as_ref().and_then(|p| p.edit.as_ref());
                let Some(fix) = form.and_then(EditForm::fix) else {
                    return;
                };
                if matches!(fix, crate::machines::Fix::Terminal(_)) {
                    self.close_machines();
                }
                self.run_fix(fix);
            }
            // The list waits while it is tried, as the fields do.
            Press::AddAddress | Press::RemoveAddress(_) if self.edit_try().is_some() => {}
            Press::AddAddress => {
                if let Some(panel) = self.machines_panel.as_mut() {
                    panel.add_address();
                }
            }
            Press::RemoveAddress(i) => {
                if let Some(panel) = self.machines_panel.as_mut() {
                    panel.remove_address(i);
                }
            }
            Press::Merge { keep, drop } => self.ask_merge_machines(&keep, &drop),
            Press::Place { stage, back } => self.place_stage(stage, back),
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
            Some(MachinesHit::Tab(i)) => {
                if let (Some(panel), Some(tab)) = (self.machines_panel.as_mut(), MachinesTab::ALL.get(i)) {
                    panel.show_tab(*tab);
                }
                None
            }
            Some(MachinesHit::Hint(i)) => {
                if let Some((keys, _)) = view.hints.get(i) {
                    self.press_hint(crate::input::Overlay::MachinesPanel, keys, pos);
                }
                None
            }
            Some(MachinesHit::Row(i)) => {
                let row = i.min(content.add_row());
                // Another machine leaves the form: what it tried stops.
                if row != view.selected {
                    self.stop_edit_try();
                }
                if let Some(panel) = self.machines_panel.as_mut() {
                    panel.select(row);
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
            // The address list's × and "+ Add address": not while it tries.
            Some(hit @ (MachinesHit::EditRemoveAddress(_) | MachinesHit::EditAddAddress)) => {
                let busy = content.edit.as_ref().is_none_or(|e| e.busy);
                let press = match hit {
                    MachinesHit::EditRemoveAddress(i) => Press::RemoveAddress(i),
                    _ => Press::AddAddress,
                };
                (!busy).then_some(Outcome::Press(press))
            }
            // A field takes the keyboard; `press_field` puts the caret.
            Some(hit @ (MachinesHit::EditName | MachinesHit::EditAddress(_))) => {
                let busy = content.edit.as_ref().is_none_or(|e| e.busy);
                if let Some(panel) = self.machines_panel.as_mut().filter(|_| !busy) {
                    panel.focus = match hit {
                        MachinesHit::EditAddress(i) => MachinesFocus::EditAddress(i),
                        _ => MachinesFocus::EditName,
                    };
                }
                None
            }
            Some(MachinesHit::EditButton(b)) => {
                let focus = MachinesFocus::EditButton(b);
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

    /// The edit form's focused field, for the pointer and pastes; none while
    /// the form is shut or its address is tried.
    pub(crate) fn machines_panel_field_mut(&mut self) -> Option<&mut crate::text_field::TextField> {
        if self.edit_try().is_some() {
            return None;
        }
        let panel = self.machines_panel.as_mut()?;
        let form = panel.edit.as_mut()?;
        match panel.focus {
            MachinesFocus::EditName => Some(&mut form.name),
            MachinesFocus::EditAddress(i) => form.addresses.get_mut(i),
            _ => None,
        }
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
        let (labels, buttons) = machine_buttons("a", "me@a", Some(&up()), &[])
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
                edit: None,
            }),
            run_presses: vec![presses],
            buttons,
            asks_remove: false,
            edit: None,
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
        panel.key(c, &mods, (sym, sym), (10, 10))
    }

    /// A key with Ctrl or Alt held, as input sends it.
    fn chord(panel: &mut MachinesPanel, c: &Content, ctrl: bool, sym: Keysym) -> Option<Outcome> {
        let mods = ModifiersState {
            ctrl,
            alt: !ctrl,
            ..ModifiersState::default()
        };
        panel.key(c, &mods, (sym, sym), (10, 10))
    }

    /// `content()` with the form open on machine `a`, from `panel`'s form,
    /// tried while `trying`.
    fn editing(panel: &MachinesPanel, trying: bool) -> Content {
        let mut c = content();
        let form = panel.edit.as_ref().expect("the form is open");
        let (view, edit) = edit_parts(form, trying.then_some("reaching it over SSH…"));
        c.detail.as_mut().unwrap().edit = Some(view);
        c.edit = Some(edit);
        c
    }

    /// A panel with the form open on machine `a`, saved at `me@a`.
    fn open_form() -> MachinesPanel {
        let mut panel = MachinesPanel::default();
        panel.open_edit("a", &["me@a".to_owned()], false);
        panel
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
            (machine_buttons("a", "me@a", state, &[]).into_iter())
                .map(|(l, _)| l)
                .collect()
        };
        assert_eq!(labels(Some(&up())), ["Remove", "Edit", "Disconnect"]);
        assert_eq!(labels(None), ["Remove", "Edit", "Connect"]);
        let auth = LinkState::Failed(Failure::Auth);
        assert_eq!(
            labels(Some(&auth)),
            ["Remove", "Edit", "Copy command", "Connect"]
        );
        let away = LinkState::Retrying(Failure::TimedOut);
        assert_eq!(labels(Some(&away)), ["Remove", "Edit", "Disconnect"]);
        let presses: Vec<Press> = (machine_buttons("a", "me@a", Some(&auth), &[]).into_iter())
            .map(|(_, p)| p)
            .collect();
        assert_eq!(
            presses,
            [
                Press::Remove("a".into()),
                Press::Edit("a".into()),
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
                MachinesFocus::Button(2),
                MachinesFocus::List,
            ]
        );
        key(&mut panel, &c, Keysym::ISO_Left_Tab, true);
        assert_eq!(panel.focus, MachinesFocus::Button(2));
        let pressed = key(&mut panel, &c, Keysym::Return, false);
        assert_eq!(pressed, Some(Outcome::Press(Press::Disconnect("a".into()))));
        for _ in 0..4 {
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
    fn a_stage_goes_through_this_machine_each_saved_one_then_least_busy() {
        let names = ["b".to_owned(), "c".to_owned()];
        let next = |now, back| next_place(&names, now, back);
        assert_eq!(next(None, false).as_deref(), Some("b"));
        assert_eq!(next(Some("b"), false).as_deref(), Some("c"));
        assert_eq!(next(Some("c"), false).as_deref(), Some("least busy"));
        assert_eq!(next(Some("least busy"), false), None, "back to this machine");
        assert_eq!(next(None, true).as_deref(), Some("least busy"));
        assert_eq!(next(Some("gone"), false).as_deref(), Some("b"), "an unsaved name goes on");
    }

    #[test]
    fn placement_keys_move_and_choose() {
        let place = |stage| vec![Press::Place { stage, back: false }];
        let c = Content {
            run_presses: vec![place(Stage::Build), place(Stage::Verify)],
            ..Content::default()
        };
        let mut panel = MachinesPanel::default();
        let key = |panel: &mut MachinesPanel, focus, sym| {
            placement_key(&c, focus, panel, &ModifiersState::default(), sym)
        };
        assert_eq!(key(&mut panel, MachinesFocus::Run(0, 0), Keysym::Down), None);
        assert_eq!(panel.focus, MachinesFocus::Run(1, 0));
        let left = key(&mut panel, MachinesFocus::Run(1, 0), Keysym::Left);
        let back = Press::Place { stage: Stage::Verify, back: true };
        assert_eq!(left, Some(Outcome::Press(back)));
        assert_eq!(key(&mut panel, MachinesFocus::Run(1, 0), Keysym::Escape), Some(Outcome::Close));
    }

    #[test]
    fn del_asks_at_once_when_sandboxes_stay() {
        let mut panel = MachinesPanel::default();
        let c = Content {
            asks_remove: true,
            ..content()
        };
        let removed = key(&mut panel, &c, Keysym::Delete, false);
        assert_eq!(removed, Some(Outcome::Press(Press::Remove("a".into()))));
        assert_eq!(
            hints(&c, 0, MachinesFocus::List, false)[3].1,
            "remove\u{2026}"
        );
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
                "Ctrl+E: edit",
                "Ctrl+PgDn: placement",
                "Esc: close"
            ]
        );
        let armed = keys(hints(&c, 0, MachinesFocus::Run(0, 0), true));
        assert!(armed.contains(&"Enter: show".to_owned()));
        assert!(armed.contains(&"Del: remove now".to_owned()));
        assert_eq!(
            keys(hints(&on_add_row(&c), 2, MachinesFocus::List, false)),
            [
                "Up / Down: move",
                "Enter: add machine",
                "Ctrl+PgDn: placement",
                "Esc: close"
            ]
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

    #[test]
    fn ctrl_e_and_f2_open_the_form_but_not_on_the_add_row() {
        let c = content();
        let mut panel = MachinesPanel::default();
        let edit = chord(&mut panel, &c, true, Keysym::e);
        assert_eq!(edit, Some(Outcome::Press(Press::Edit("a".into()))));
        assert_eq!(panel.focus, MachinesFocus::List);
        let f2 = key(&mut panel, &c, Keysym::F2, false);
        assert_eq!(f2, Some(Outcome::Press(Press::Edit("a".into()))));
        assert_eq!(panel.focus, MachinesFocus::EditName);
        let mut panel = MachinesPanel {
            selected: 2,
            ..MachinesPanel::default()
        };
        let add = on_add_row(&c);
        assert_eq!(chord(&mut panel, &add, true, Keysym::e), None);
        assert_eq!(key(&mut panel, &add, Keysym::F2, false), None);
        // Opened, it holds what is saved, the keyboard on the address.
        let panel = open_form();
        let form = panel.edit.as_ref().unwrap();
        assert_eq!((&*form.name, &*form.addresses[0]), ("a", "me@a"));
        assert_eq!(panel.focus, MachinesFocus::EditAddress(0));
    }

    #[test]
    fn tab_goes_round_the_forms_fields_and_buttons() {
        let mut panel = open_form();
        panel.focus = MachinesFocus::EditName;
        let c = editing(&panel, false);
        let labels = &c.detail.as_ref().unwrap().edit.as_ref().unwrap().buttons;
        assert_eq!(labels, &["Cancel", "Save"]);
        let mut seen = Vec::new();
        for _ in 0..4 {
            key(&mut panel, &c, Keysym::Tab, false);
            seen.push(panel.focus);
        }
        assert_eq!(
            seen,
            [
                MachinesFocus::EditAddress(0),
                MachinesFocus::EditButton(0),
                MachinesFocus::EditButton(1),
                MachinesFocus::EditName,
            ]
        );
        key(&mut panel, &c, Keysym::ISO_Left_Tab, true);
        assert_eq!(panel.focus, MachinesFocus::EditButton(1));
        // Up and Down, Ctrl+K and Ctrl+J, go between the fields.
        key(&mut panel, &c, Keysym::Up, false);
        assert_eq!(panel.focus, MachinesFocus::EditAddress(0));
        key(&mut panel, &c, Keysym::Up, false);
        assert_eq!(panel.focus, MachinesFocus::EditName);
        chord(&mut panel, &c, true, Keysym::j);
        assert_eq!(panel.focus, MachinesFocus::EditAddress(0));
        chord(&mut panel, &c, true, Keysym::k);
        assert_eq!(panel.focus, MachinesFocus::EditName);
        // The list stays where it is, and Del is the field's.
        key(&mut panel, &c, Keysym::Down, false);
        key(&mut panel, &c, Keysym::Delete, false);
        assert_eq!((panel.selected, panel.armed.as_ref()), (0, None));
    }

    #[test]
    fn enter_saves_or_presses_the_focused_button() {
        let mut panel = open_form();
        let c = editing(&panel, false);
        let save = key(&mut panel, &c, Keysym::Return, false);
        assert_eq!(save, Some(Outcome::Press(Press::Save)));
        panel.focus = MachinesFocus::EditButton(0);
        let cancel = key(&mut panel, &c, Keysym::Return, false);
        assert_eq!(cancel, Some(Outcome::Press(Press::CancelEdit)));
        let save = chord(&mut panel, &c, true, Keysym::Return);
        assert_eq!(save, Some(Outcome::Press(Press::Save)));
        // After a failure with a fix: Alt+X presses it, and Enter tries again.
        let form = panel.edit.as_mut().unwrap();
        form.failed = Some(("me@a2".into(), Failure::Auth));
        let c = editing(&panel, false);
        let edit = c.detail.as_ref().unwrap().edit.clone().unwrap();
        assert_eq!(
            edit.buttons,
            ["Cancel", "Copy command", "Save anyway", "Try again"]
        );
        assert_eq!(edit.error, Some(Failure::Auth.why()));
        assert!(edit.how.is_some_and(|h| h.contains("ssh-copy-id me@a2")));
        panel.focus = MachinesFocus::EditButton(2);
        let anyway = key(&mut panel, &c, Keysym::Return, false);
        assert_eq!(anyway, Some(Outcome::Press(Press::SaveAnyway)));
        let fix = chord(&mut panel, &c, false, Keysym::x);
        assert_eq!(fix, Some(Outcome::Press(Press::EditFix)));
        let keys: Vec<String> = (hints(&c, 0, MachinesFocus::EditAddress(0), false).into_iter())
            .map(|(k, w)| format!("{k}: {w}"))
            .collect();
        assert_eq!(
            keys,
            [
                "Tab: next",
                "Enter: try again",
                "Ctrl+N: add address",
                "Alt+X: fix",
                "Esc: cancel"
            ]
        );
    }

    #[test]
    fn esc_stops_the_try_then_leaves_the_form_then_closes() {
        let mut panel = open_form();
        let busy = editing(&panel, true);
        // While it tries, only Stop: the fields wait.
        assert_eq!(busy.stops(), [MachinesFocus::EditButton(0)]);
        assert_eq!(
            busy.detail.as_ref().unwrap().edit.as_ref().unwrap().buttons,
            ["Stop"]
        );
        let typed = key(&mut panel, &busy, Keysym::b, false);
        assert_eq!(
            (typed, &*panel.edit.as_ref().unwrap().addresses[0]),
            (None, "me@a")
        );
        assert_eq!(panel.focus, MachinesFocus::EditButton(0));
        let enter = key(&mut panel, &busy, Keysym::Return, false);
        assert_eq!(enter, Some(Outcome::Press(Press::StopTrying)));
        let esc = key(&mut panel, &busy, Keysym::Escape, false);
        assert_eq!(esc, Some(Outcome::Press(Press::StopTrying)));
        assert_eq!(
            keys_of(hints(&busy, 0, MachinesFocus::EditButton(0), false)),
            ["Esc: stop"]
        );
        let idle = editing(&panel, false);
        let esc = key(&mut panel, &idle, Keysym::Escape, false);
        assert_eq!(esc, Some(Outcome::Press(Press::CancelEdit)));
        panel.close_edit();
        let esc = key(&mut panel, &content(), Keysym::Escape, false);
        assert_eq!(esc, Some(Outcome::Close));
    }

    fn keys_of(h: Vec<(String, String)>) -> Vec<String> {
        h.into_iter().map(|(k, w)| format!("{k}: {w}")).collect()
    }

    #[test]
    fn typing_goes_to_the_field_and_clears_the_error() {
        let mut panel = open_form();
        panel.edit.as_mut().unwrap().error = Some("Say which machine, as user@host.".into());
        let c = editing(&panel, false);
        key(&mut panel, &c, Keysym::_2, false);
        let form = panel.edit.as_ref().unwrap();
        assert_eq!((&*form.addresses[0], form.error.as_ref()), ("me@a2", None));
    }

    #[test]
    fn what_was_typed_waits_for_its_machine() {
        let mut panel = open_form();
        panel.edit.as_mut().unwrap().addresses[0].set("me@a.lan");
        // Another row closes the form, keeping the draft.
        panel.select(1);
        assert!(panel.edit.is_none());
        assert_eq!(panel.focus, MachinesFocus::List);
        panel.open_edit("b", &["me@b".to_owned()], true);
        assert_eq!(&*panel.edit.as_ref().unwrap().addresses[0], "me@b");
        assert_eq!(panel.focus, MachinesFocus::EditName);
        panel.close_edit();
        // Nothing typed on b: nothing kept of it.
        assert!(!panel.drafts.contains_key("b"));
        panel.select(0);
        panel.open_edit("a", &["me@a".to_owned()], false);
        assert_eq!(&*panel.edit.as_ref().unwrap().addresses[0], "me@a.lan");
        // A rename takes the draft along.
        panel.close_edit();
        panel.renamed("a", "z");
        assert_eq!(
            panel.drafts["z"],
            ("a".to_owned(), vec!["me@a.lan".to_owned()])
        );
        // A form whose machine is gone closes on the next key, kept too.
        panel.open_edit("z", &["me@a".to_owned()], false);
        key(&mut panel, &content(), Keysym::Down, false);
        assert!(panel.edit.is_none());
        assert!(panel.drafts.contains_key("z"));
    }

    /// A key with these modifiers, as input sends it.
    fn with(
        panel: &mut MachinesPanel,
        c: &Content,
        m: (bool, bool),
        sym: Keysym,
    ) -> Option<Outcome> {
        let mods = ModifiersState {
            ctrl: m.0,
            shift: m.1,
            ..ModifiersState::default()
        };
        panel.key(c, &mods, (sym, sym), (10, 10))
    }

    /// The form's addresses as typed.
    fn listed(panel: &MachinesPanel) -> Vec<String> {
        panel.edit.as_ref().unwrap().typed()
    }

    #[test]
    fn the_form_holds_every_address_and_tab_goes_through_them() {
        let saved = ["me@a".to_owned(), "me@a.lan".to_owned()];
        let mut panel = MachinesPanel::default();
        panel.open_edit("a", &saved, true);
        let c = editing(&panel, false);
        let view = c.detail.as_ref().unwrap().edit.clone().unwrap();
        let lines: Vec<&str> = view.addresses.iter().map(|l| &**l).collect();
        assert_eq!(lines, ["me@a", "me@a.lan"]);
        let mut seen = Vec::new();
        for _ in 0..5 {
            key(&mut panel, &c, Keysym::Tab, false);
            seen.push(panel.focus);
        }
        assert_eq!(
            seen,
            [
                MachinesFocus::EditAddress(0),
                MachinesFocus::EditAddress(1),
                MachinesFocus::EditButton(0),
                MachinesFocus::EditButton(1),
                MachinesFocus::EditName,
            ]
        );
        // Up and Down, Ctrl+K and Ctrl+J, step through the fields; from a
        // button, Up goes to the last address.
        key(&mut panel, &c, Keysym::Down, false);
        key(&mut panel, &c, Keysym::Down, false);
        assert_eq!(panel.focus, MachinesFocus::EditAddress(1));
        key(&mut panel, &c, Keysym::Down, false);
        assert_eq!(panel.focus, MachinesFocus::EditAddress(1));
        chord(&mut panel, &c, true, Keysym::k);
        assert_eq!(panel.focus, MachinesFocus::EditAddress(0));
        chord(&mut panel, &c, true, Keysym::j);
        assert_eq!(panel.focus, MachinesFocus::EditAddress(1));
        panel.focus = MachinesFocus::EditButton(0);
        key(&mut panel, &c, Keysym::Up, false);
        assert_eq!(panel.focus, MachinesFocus::EditAddress(1));
        // Typing goes to the address focused.
        key(&mut panel, &c, Keysym::_2, false);
        assert_eq!(listed(&panel), ["me@a", "me@a.lan2"]);
    }

    #[test]
    fn ctrl_n_adds_an_address_and_ctrl_shift_arrows_move_it() {
        let mut panel = open_form();
        let c = editing(&panel, false);
        let hints = keys_of(hints(&c, 0, MachinesFocus::EditAddress(0), false));
        assert!(hints.contains(&"Ctrl+N: add address".to_owned()));
        assert_eq!(chord(&mut panel, &c, true, Keysym::n), None);
        assert_eq!(listed(&panel), ["me@a", ""]);
        assert_eq!(panel.focus, MachinesFocus::EditAddress(1));
        let c = editing(&panel, false);
        for sym in [Keysym::b, Keysym::at, Keysym::b] {
            key(&mut panel, &c, sym, false);
        }
        assert_eq!(listed(&panel), ["me@a", "b@b"]);
        // Up the list, and it is tried first; the keyboard goes with it.
        with(&mut panel, &c, (true, true), Keysym::Up);
        assert_eq!(listed(&panel), ["b@b", "me@a"]);
        assert_eq!(panel.focus, MachinesFocus::EditAddress(0));
        // Past the top, nothing moves.
        with(&mut panel, &c, (true, true), Keysym::Up);
        assert_eq!(listed(&panel), ["b@b", "me@a"]);
        with(&mut panel, &c, (true, true), Keysym::Down);
        assert_eq!(listed(&panel), ["me@a", "b@b"]);
        assert_eq!(panel.focus, MachinesFocus::EditAddress(1));
        // On the name, nothing moves either.
        panel.focus = MachinesFocus::EditName;
        with(&mut panel, &c, (true, true), Keysym::Up);
        assert_eq!(listed(&panel), ["me@a", "b@b"]);
        // While it tries, the list waits too.
        let busy = editing(&panel, true);
        chord(&mut panel, &busy, true, Keysym::n);
        assert_eq!(listed(&panel).len(), 2);
    }

    #[test]
    fn x_and_del_on_an_empty_address_take_it_out_but_not_the_last() {
        let saved = [
            "me@a".to_owned(),
            "me@a.lan".to_owned(),
            "me@10.0.0.2".to_owned(),
        ];
        let mut panel = MachinesPanel::default();
        panel.open_edit("a", &saved, false);
        // The ×: the keyboard on a later field stays on it.
        panel.focus = MachinesFocus::EditAddress(2);
        panel.remove_address(0);
        assert_eq!(listed(&panel), ["me@a.lan", "me@10.0.0.2"]);
        assert_eq!(panel.focus, MachinesFocus::EditAddress(1));
        // Del on a field holding something deletes in it.
        let c = editing(&panel, false);
        panel.focus = MachinesFocus::EditAddress(0);
        key(&mut panel, &c, Keysym::Home, false);
        key(&mut panel, &c, Keysym::Delete, false);
        assert_eq!(listed(&panel), ["e@a.lan", "me@10.0.0.2"]);
        // Emptied, Del takes it out.
        panel.edit.as_mut().unwrap().addresses[0].set("");
        key(&mut panel, &c, Keysym::Delete, false);
        assert_eq!(listed(&panel), ["me@10.0.0.2"]);
        assert_eq!(panel.focus, MachinesFocus::EditAddress(0));
        // The last one stays, however it is asked.
        panel.edit.as_mut().unwrap().addresses[0].set("");
        let c = editing(&panel, false);
        key(&mut panel, &c, Keysym::Delete, false);
        panel.remove_address(0);
        assert_eq!(listed(&panel), [""]);
    }

    #[test]
    fn drafts_keep_the_whole_list() {
        let saved = ["me@a".to_owned(), "me@a.lan".to_owned()];
        let mut panel = MachinesPanel::default();
        panel.open_edit("a", &saved, false);
        // Only another order is a draft too: it is the order tried.
        panel.focus = MachinesFocus::EditAddress(1);
        panel.move_address(false);
        panel.add_address();
        panel.close_edit();
        let draft = (
            "a".to_owned(),
            vec!["me@a.lan".into(), "me@a".into(), String::new()],
        );
        assert_eq!(panel.drafts["a"], draft);
        panel.open_edit("a", &saved, false);
        assert_eq!(listed(&panel), ["me@a.lan", "me@a", ""]);
        // Back as it was saved, nothing is kept.
        panel.remove_address(2);
        panel.focus = MachinesFocus::EditAddress(0);
        panel.move_address(true);
        panel.close_edit();
        assert!(!panel.drafts.contains_key("a"));
    }

    #[test]
    fn the_same_machine_offers_merge_before_remove() {
        let same = ["b".to_owned()];
        let buttons = machine_buttons("a", "me@a", None, &same);
        let labels: Vec<&str> = buttons.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["Merge b", "Remove", "Edit", "Connect"]);
        let merge = Press::Merge {
            keep: "a".into(),
            drop: "b".into(),
        };
        assert_eq!(buttons[0].1, merge);
        // A merge takes the merged one's draft and form, and selects the one
        // kept.
        let mut panel = MachinesPanel::default();
        panel.open_edit("b", &["me@b".to_owned()], false);
        panel.edit.as_mut().unwrap().addresses[0].set("me@b2");
        panel.merged("b", 0, (10, 3));
        assert!(panel.edit.is_none() && panel.drafts.is_empty());
        assert_eq!((panel.selected, panel.focus), (0, MachinesFocus::List));
    }
}

//! Editing a machine's name and address, from the form in the machines
//! panel (see `machines_panel`): what the form says is checked first, a new
//! address is tried before it is kept, and a new name moves what gerdr keeps
//! under the old one without dropping the link.
//!
//! Trying an address is the connect dialog's way: the machine's link starts
//! again on it, under the same name, and says how far it got. It is kept only
//! once that machine answers there, and only if it is the same machine: one
//! of the gerdr machine ids it was seen by, or any for a machine never seen.
//! Anything else puts the old link back.
//!
//! A rename keeps the link running. The link's thread still says the old name
//! in its events, but they find the link by its serial (`Machines::link_named`).
//! What it moves: `[machines]` in the settings (`config::rename_machine`, its
//! comments and place kept), `Machines::saved`, `links` and `names`, the
//! `place.json` of each id it was seen by, and the projects' `machines` and
//! `machine_folders`. The rest of what gerdr keeps of it goes by its id.

use std::collections::{BTreeMap, HashMap};

use crate::{
    Gerdr,
    chrome::MachinesFocus,
    config::Addresses,
    feature::view,
    machines::{Failure, Fix, LinkInfo, LinkState, default_name, valid_target},
    menu::MenuAction,
    projects::Workflow,
    text_field::TextField,
};

/// What the form holds, as a draft is kept: name, addresses.
pub type Draft = (String, Vec<String>);

/// The form editing a machine, while it is open in the machines panel.
#[derive(Debug, Default)]
pub struct EditForm {
    /// The machine edited, by its name now.
    pub machine: String,
    pub name: TextField,
    /// Its addresses, in the order they are tried: at least one field.
    pub addresses: Vec<TextField>,
    /// What the form held when it opened: a draft is kept only when it
    /// says something else.
    pub from: Draft,
    /// What is wrong with the form.
    pub error: Option<String>,
    /// Why the last try of a new address failed: the address, and what
    /// happened.
    pub failed: Option<(String, Failure)>,
}

impl EditForm {
    /// The form on machine `machine`, holding `name` and `addresses`, as
    /// saved at `from`.
    pub fn new(machine: &str, (name, addresses): (&str, &[String]), from: Draft) -> Self {
        let mut form = Self {
            machine: machine.to_owned(),
            from,
            ..Self::default()
        };
        form.name.set(name);
        form.addresses = addresses.iter().map(|a| a.as_str().into()).collect();
        if form.addresses.is_empty() {
            form.addresses.push(TextField::default());
        }
        form
    }

    /// The addresses as typed, empty fields too.
    pub fn typed(&self) -> Vec<String> {
        self.addresses.iter().map(|a| a.to_string()).collect()
    }

    /// What was typed, if it isn't what the form opened with: name,
    /// addresses.
    pub fn draft(&self) -> Option<Draft> {
        let typed = (self.name.to_string(), self.typed());
        (typed != self.from).then_some(typed)
    }

    /// A new empty address field at the end: its index.
    pub fn add_address(&mut self) -> usize {
        self.addresses.push(TextField::default());
        self.error = None;
        self.addresses.len() - 1
    }

    /// Take address field `i` out, but never the last one left: whether
    /// it went.
    pub fn remove_address(&mut self, i: usize) -> bool {
        if self.addresses.len() < 2 || i >= self.addresses.len() {
            return false;
        }
        self.addresses.remove(i);
        self.error = None;
        true
    }

    /// Move address field `i` one place up, or down with `down`: where it
    /// went, if it could.
    pub fn move_address(&mut self, i: usize, down: bool) -> Option<usize> {
        let to = match down {
            true => i + 1,
            false => i.checked_sub(1)?,
        };
        if to >= self.addresses.len() || i >= self.addresses.len() {
            return None;
        }
        self.addresses.swap(i, to);
        self.error = None;
        Some(to)
    }

    /// The field holding `address`, trimmed as checked: where a failure
    /// there puts the keyboard.
    pub fn field_of(&self, address: &str) -> usize {
        (self.addresses.iter())
            .position(|a| a.trim() == address)
            .unwrap_or(0)
    }

    /// The button fixing what the last try ran into, if one can help.
    pub fn fix(&self) -> Option<Fix> {
        let (target, failure) = self.failed.as_ref()?;
        failure.fix(target)
    }

    /// Whether "Save anyway" is offered: after a try that failed, but for
    /// one that reached another machine, which nothing should keep.
    pub fn untried_offered(&self) -> bool {
        (self.failed.as_ref()).is_some_and(|(_, f)| !matches!(f, Failure::OtherMachine(_)))
    }
}

/// A new address of a machine being tried, before it is kept.
#[derive(Debug)]
pub struct EditTry {
    /// The machine, by its name now.
    pub name: String,
    /// The serial of the link trying it.
    pub serial: u64,
    /// Where it was, and whether its link was running there: put back as it
    /// was if the try fails.
    pub old_target: Addresses,
    pub was_live: bool,
    /// What the form asked for.
    pub new_name: String,
    pub new_target: Addresses,
    /// The ids it was seen by: the one answering must be one of them.
    pub known: Vec<String>,
    /// How far trying got.
    pub progress: String,
}

/// What the form says, checked against the machines saved, machine
/// `machine` among them: the name and addresses to keep, or what is wrong
/// and the field at fault. Each address is trimmed, and empty ones are
/// left out, but one must be left.
pub fn check_edit(
    saved: &BTreeMap<String, Addresses>,
    machine: &str,
    name: &str,
    addresses: &[&str],
) -> Result<(String, Addresses), (String, MachinesFocus)> {
    let name = name.trim();
    let typed: Vec<(usize, &str)> = (addresses.iter().map(|a| a.trim()).enumerate())
        .filter(|(_, a)| !a.is_empty())
        .collect();
    let at = |i: usize, why: String| Err((why, MachinesFocus::EditAddress(i)));
    let Some(&(_, first)) = typed.first() else {
        return at(0, "Say which machine, as user@host.".to_owned());
    };
    if let Some(&(i, _)) = typed.iter().find(|(_, a)| !valid_target(a)) {
        return at(i, Failure::BadTarget.why());
    }
    for (n, &(i, address)) in typed.iter().enumerate() {
        if typed[..n].iter().any(|(_, a)| *a == address) {
            return at(i, format!("{address} is in the list twice."));
        }
    }
    for &(i, address) in &typed {
        if let Some((other, _)) = (saved.iter()).find(|(n, t)| t.contains(address) && *n != machine)
        {
            return at(i, format!("It is here already, as {other}."));
        }
    }
    let name = match name.is_empty() {
        true => default_name(first),
        false => name.to_owned(),
    };
    if name != machine && saved.contains_key(&name) {
        return Err((
            format!("Another machine is called {name}."),
            MachinesFocus::EditName,
        ));
    }
    let addresses = typed.into_iter().map(|(_, a)| a.to_owned()).collect();
    Ok((name, Addresses(addresses)))
}

/// Whether the new addresses `new` of a machine at `old` can be kept
/// without trying them: only reordered or cut, its link not running, or
/// its link up on an address still among them. `up_at`: where its link is
/// up; `live`: whether it runs at all.
pub fn keeps_without_try(
    old: &Addresses,
    new: &Addresses,
    up_at: Option<&str>,
    live: bool,
) -> bool {
    if up_at.is_some_and(|at| new.contains(at)) {
        return true;
    }
    let nothing_new = new.0.iter().all(|a| old.contains(a));
    nothing_new && !live
}

/// Machine `drop`'s addresses after `keep`'s, but for those `keep` has.
pub fn joined_addresses(keep: &Addresses, drop: &Addresses) -> Addresses {
    let mut joined = keep.clone();
    for address in &drop.0 {
        if !joined.contains(address) {
            joined.0.push(address.clone());
        }
    }
    joined
}

/// The ids seen under the name `drop` are seen under `keep` now: those
/// moved, in order.
pub fn move_names(names: &mut HashMap<String, String>, keep: &str, drop: &str) -> Vec<String> {
    let mut moved = Vec::new();
    for (id, name) in names.iter_mut().filter(|(_, n)| *n == drop) {
        *name = keep.to_owned();
        moved.push(id.clone());
    }
    moved.sort();
    moved
}

/// A project's settings name machine `keep` where they named `drop`: its
/// stages, and its folders there joining `keep`'s, `keep`'s own winning
/// where both say one. Whether anything changed, and the folders where
/// `drop`'s was left out.
pub fn merge_workflow(workflow: &mut Workflow, keep: &str, drop: &str) -> (bool, Vec<String>) {
    let mut changed = false;
    for name in workflow.machines.values_mut().filter(|n| *n == drop) {
        *name = keep.to_owned();
        changed = true;
    }
    let mut clashes = Vec::new();
    if let Some(folders) = workflow.machine_folders.remove(drop) {
        changed = true;
        let kept = workflow.machine_folders.entry(keep.to_owned()).or_default();
        for (folder, there) in folders {
            match kept.get(&folder) {
                Some(own) if *own != there => clashes.push(folder),
                Some(_) => {}
                None => _ = kept.insert(folder, there),
            }
        }
    }
    (changed, clashes)
}

/// The machines among `ids` (each machine's ids, by its name) that are
/// machine `name` too: one id is both's.
pub fn same_machines(name: &str, ids: &BTreeMap<String, Vec<String>>) -> Vec<String> {
    let Some(mine) = ids.get(name) else {
        return Vec::new();
    };
    (ids.iter())
        .filter(|(other, theirs)| *other != name && theirs.iter().any(|id| mine.contains(id)))
        .map(|(other, _)| other.clone())
        .collect()
}

/// What the confirm dialog merging `drop` into `keep` says under its title.
pub fn merge_body(keep: &str, drop: &str) -> String {
    format!(
        "{drop}'s addresses join {keep}'s, after them, and {drop} goes. \
         Projects that name {drop} name {keep} instead."
    )
}

/// Whether the machine answering as `answered` is the one seen by the ids
/// `known`: any is, for a machine never seen.
pub fn same_machine(known: &[String], answered: &str) -> bool {
    known.is_empty() || known.iter().any(|id| id == answered)
}

impl Gerdr {
    /// The try of a new address under way, while its link is still that try.
    pub(crate) fn edit_try(&self) -> Option<&EditTry> {
        let tried = self.machines.editing.as_ref()?;
        (self.machines.links.get(&tried.name))
            .is_some_and(|l| l.serial == tried.serial)
            .then_some(tried)
    }

    /// The edit form, if it is open on machine `machine`.
    fn edit_form_mut(&mut self, machine: &str) -> Option<&mut EditForm> {
        (self.machines_panel.as_mut()?.edit.as_mut()).filter(|f| f.machine == machine)
    }

    /// Say what is wrong in the form, the keyboard on the field at fault.
    fn edit_error(&mut self, why: String, field: MachinesFocus) {
        if let Some(panel) = self.machines_panel.as_mut() {
            if let Some(form) = panel.edit.as_mut() {
                form.error = Some(why);
            }
            panel.focus = field;
        }
    }

    /// Whether machine `name` is connecting, or a new address of it is
    /// tried: what moves its link waits for that.
    fn machine_busy(&self, name: &str) -> bool {
        let connecting = matches!(
            self.machines.links.get(name).map(|l| &l.state),
            Some(LinkState::Connecting(_))
        );
        connecting || self.edit_try().is_some_and(|t| t.name == name)
    }

    /// Save the form: a new name at once, new addresses once tried, or
    /// untried with `untried` ("Save anyway"). Addresses only put in
    /// another order, or added while the link is up on one still there,
    /// are kept at once, the link left as it is.
    pub(crate) fn save_edit(&mut self, untried: bool) {
        if self.edit_try().is_some() {
            return;
        }
        let Some(form) = self.machines_panel.as_ref().and_then(|p| p.edit.as_ref()) else {
            return;
        };
        let machine = form.machine.clone();
        let Some(old_target) = self.machines.saved.get(&machine).cloned() else {
            self.edit_closed(None);
            return self.show_warning(format!("{machine} isn't in the settings any more"));
        };
        let typed = form.typed();
        let typed: Vec<&str> = typed.iter().map(String::as_str).collect();
        let checked = check_edit(&self.machines.saved, &machine, &form.name, &typed);
        let (name, target) = match checked {
            Ok(checked) => checked,
            Err((why, field)) => return self.edit_error(why, field),
        };
        if let Some(form) = self.edit_form_mut(&machine) {
            form.error = None;
        }
        if target == old_target {
            if name != machine {
                if let Err(why) = self.rename_machine(&machine, &name) {
                    return self.edit_error(why, MachinesFocus::EditName);
                }
                let undo = MenuAction::RenameMachine {
                    from: name.clone(),
                    to: machine.clone(),
                };
                self.toast_with(
                    format!("Renamed {machine} to {name}"),
                    crate::toast::Level::Success,
                    Some(("Undo".to_owned(), undo)),
                );
            }
            return self.edit_closed(Some(&name));
        }
        let link = self.machines.links.get(&machine);
        let was_live = matches!(
            link.map(|l| &l.state),
            Some(LinkState::Up(_) | LinkState::Connecting(_) | LinkState::Retrying(_))
        );
        let up_at = link
            .filter(|l| matches!(l.state, LinkState::Up(_)))
            .map(|l| l.target());
        if !untried && keeps_without_try(&old_target, &target, up_at, was_live) {
            return self.save_listed(&machine, &name, &target);
        }
        if untried {
            return self.save_untried(&machine, &name, &target);
        }
        let known = self.machine_ids(&machine);
        let before = link.map(|l| l.serial);
        self.start_link(machine.clone(), target.clone(), false);
        let serial = (self.machines.links.get(&machine)).map(|l| l.serial);
        let Some(serial) = serial.filter(|s| Some(*s) != before) else {
            // It couldn't start, and said so.
            return;
        };
        tracing::info!("machine {machine}: trying {target}, known as {known:?}");
        self.machines.editing = Some(EditTry {
            name: machine.clone(),
            serial,
            old_target,
            was_live,
            new_name: name,
            new_target: target,
            known,
            progress: "connecting\u{2026}".into(),
        });
        if let Some(panel) = self.machines_panel.as_mut() {
            if let Some(form) = panel.edit.as_mut() {
                form.failed = None;
            }
            panel.focus = MachinesFocus::EditButton(0);
        }
        self.starts_lost();
        self.report_machines();
    }

    /// Machine `machine` called `name` and kept at `target` without a try,
    /// as nothing there needs one: its link goes on as it is, and takes the
    /// new list the next time it starts.
    fn save_listed(&mut self, machine: &str, name: &str, target: &Addresses) {
        if name != machine
            && let Err(why) = self.rename_machine(machine, name)
        {
            return self.edit_error(why, MachinesFocus::EditName);
        }
        if let Err(why) = crate::config::write_machine(name, Some(target)) {
            tracing::warn!("machines: saving {name} at {target:?}: {why}");
            let why = format!("gerdr couldn't keep it in the settings: {why}.");
            return self.edit_error(why, MachinesFocus::EditAddress(0));
        }
        self.settings_modified = crate::config::modified();
        (self.machines.saved).insert(name.to_owned(), target.clone());
        if let Some(link) = self.machines.links.get_mut(name) {
            link.addresses = target.clone();
        }
        tracing::info!("machine {name}: addresses now {target}, its link kept");
        self.report_machines();
        self.show_success(format!("Saved {name}"));
        self.edit_closed(Some(name));
    }

    /// "Save anyway": machine `machine` called `name` and kept at `target`
    /// without trying it first, its link starting there.
    fn save_untried(&mut self, machine: &str, name: &str, target: &Addresses) {
        if name != machine
            && let Err(why) = self.rename_machine(machine, name)
        {
            return self.edit_error(why, MachinesFocus::EditName);
        }
        if let Err(why) = crate::config::write_machine(name, Some(target)) {
            tracing::warn!("machines: saving {name} at {target:?}: {why}");
            let why = format!("gerdr couldn't keep it in the settings: {why}.");
            return self.edit_error(why, MachinesFocus::EditAddress(0));
        }
        self.settings_modified = crate::config::modified();
        (self.machines.saved).insert(name.to_owned(), target.clone());
        self.start_link(name.to_owned(), target.clone(), true);
        self.starts_lost();
        self.report_machines();
        self.show_success(format!("Saved {name}, not connected yet"));
        self.edit_closed(Some(name));
    }

    /// Stop trying the new address: the machine's link goes back to how it
    /// was.
    pub(crate) fn stop_edit_try(&mut self) {
        let live = self.edit_try().is_some();
        let Some(tried) = self.machines.editing.take() else {
            return;
        };
        if live {
            self.restore_link(&tried);
        }
        // The keyboard goes back to the address, as after a failed try:
        // the only button it was on is gone.
        if let Some(panel) = self.machines_panel.as_mut()
            && panel.edit.is_some()
        {
            panel.focus = MachinesFocus::EditAddress(0);
        }
        self.starts_lost();
        self.report_machines();
    }

    /// Machine `tried.name`'s link as it was before the try: running at its
    /// old address again, or disconnected.
    fn restore_link(&mut self, tried: &EditTry) {
        match tried.was_live {
            true => self.start_link(tried.name.clone(), tried.old_target.clone(), true),
            false => self.machines.link_off(&tried.name),
        }
    }

    /// How far trying the new address got.
    pub(crate) fn edit_progress(&mut self, text: String) {
        if let Some(tried) = self.machines.editing.as_mut() {
            tried.progress = text;
        }
    }

    /// The new address of machine `name` answered, as `info` says: the try,
    /// if that is the same machine, for its link to be up and the address
    /// kept. Another machine is refused, and the old link put back.
    pub(crate) fn edit_answered(&mut self, name: &str, info: &LinkInfo) -> Option<EditTry> {
        let tried = self.machines.editing.take()?;
        if same_machine(&tried.known, &info.site.id) {
            return Some(tried);
        }
        tracing::warn!(
            "machine {name}: {} answers as machine {}, not as one of {:?}",
            tried.new_target,
            info.site.id,
            tried.known
        );
        self.restore_link(&tried);
        self.edit_not_kept(tried, None, Failure::OtherMachine(name.to_owned()));
        None
    }

    /// The new address of machine `name` didn't connect: the old link goes
    /// back, and the form says why.
    pub(crate) fn edit_failed(&mut self, name: &str, failure: Failure) {
        // The address it ran into trouble at: the last it tried.
        let at = self.edit_try().map(|t| t.serial).and_then(|serial| {
            let link = self.machines.links.get(name)?;
            (link.serial == serial).then(|| link.target().to_owned())
        });
        let Some(tried) = self.machines.editing.take() else {
            return;
        };
        tracing::warn!("machine {name}: trying {}: {failure:?}", tried.new_target);
        self.restore_link(&tried);
        self.edit_not_kept(tried, at, failure);
    }

    /// What the try ran into at address `at` (else its first), in the
    /// form, the keyboard on that address; in a toast if it was closed.
    fn edit_not_kept(&mut self, tried: EditTry, at: Option<String>, failure: Failure) {
        let name = tried.name.clone();
        let at = at.unwrap_or_else(|| tried.new_target.first().to_owned());
        match self.edit_form_mut(&name) {
            Some(form) => {
                let field = form.field_of(&at);
                form.failed = Some((at, failure));
                if let Some(panel) = self.machines_panel.as_mut() {
                    panel.focus = MachinesFocus::EditAddress(field);
                }
            }
            None => self.show_warning(format!(
                "{name} stays at {}: {}",
                tried.old_target,
                failure.why().trim_end_matches('.')
            )),
        }
        self.starts_lost();
        self.report_machines();
    }

    /// The new address reached the same machine, its link up there: kept in
    /// the settings, and the new name given.
    pub(crate) fn edit_kept(&mut self, tried: EditTry) {
        let EditTry {
            name,
            new_name,
            new_target,
            ..
        } = tried;
        if let Err(why) = crate::config::write_machine(&name, Some(&new_target)) {
            tracing::warn!("machines: saving {name} at {new_target:?}: {why}");
            let at = new_target.first();
            return self.show_error(format!(
                "Connected to {name} at {at}, but gerdr couldn't keep it in the settings: {why}"
            ));
        }
        self.settings_modified = crate::config::modified();
        (self.machines.saved).insert(name.clone(), new_target);
        let mut called = name.clone();
        if new_name != name {
            match self.rename_machine(&name, &new_name) {
                Ok(()) => called = new_name,
                Err(why) => self.show_warning(format!(
                    "Saved {name}, but not its new name: {}",
                    why.trim_end_matches('.')
                )),
            }
        }
        self.show_success(format!("Saved {called}"));
        self.edit_closed(Some(&called));
    }

    /// The rename toast's Undo: machine `from` is called `to` again.
    pub(crate) fn rename_machine_back(&mut self, from: &str, to: &str) {
        match self.rename_machine(from, to) {
            Ok(()) => {
                if let Some(panel) = self.machines_panel.as_mut()
                    && let Some(row) = self.machines.saved.keys().position(|n| n == to)
                {
                    panel.selected = row;
                }
                self.show_success(format!("{from} is called {to} again"));
            }
            Err(why) => {
                let why = why.trim_end_matches('.');
                self.show_error(format!("gerdr couldn't call {from} {to} again: {why}"));
            }
        }
    }

    /// Call machine `old` `new`, keeping its link up and all gerdr keeps of
    /// it. What can't be done is said as a sentence, for the form.
    pub(crate) fn rename_machine(&mut self, old: &str, new: &str) -> Result<(), String> {
        if old == new {
            return Ok(());
        }
        if !self.machines.saved.contains_key(old) {
            return Err(format!("{old} isn't in the settings any more."));
        }
        if self.machines.saved.contains_key(new) || self.machines.links.contains_key(new) {
            return Err(format!("Another machine is called {new}."));
        }
        if self.machine_busy(old) {
            return Err(format!("Wait for {old} to connect first."));
        }
        crate::config::rename_machine(old, new).map_err(|why| {
            tracing::warn!("machines: renaming {old} to {new} in the settings: {why}");
            format!("gerdr couldn't rename it in the settings: {why}.")
        })?;
        // Written here: the settings read back don't take the old name out.
        self.settings_modified = crate::config::modified();
        let ids = self.machines.rename(old, new);
        self.rename_last(old, new);
        let mut trouble = false;
        let data = crate::projects::data_dir();
        for id in &ids {
            if let Err(err) = view::rename_place(&view::root(&data, id), old, new) {
                tracing::warn!("machine {new}: renaming what is kept of {id}: {err}");
                trouble = true;
            }
        }
        let changed: Vec<String> = (self.projects.projects.iter_mut())
            .filter_map(|p| p.workflow.rename_machine(old, new).then(|| p.id.clone()))
            .collect();
        if !changed.is_empty() {
            tracing::info!(
                "machines: {old} renamed to {new} in the files of {}",
                changed.join(", ")
            );
            if !self.projects.save_projects(&changed) {
                tracing::warn!("machines: couldn't save the rename of {old} in {changed:?}");
                trouble = true;
            }
        }
        if let Some(panel) = self.machines_panel.as_mut() {
            panel.renamed(old, new);
        }
        if trouble {
            self.toast_with(
                format!("{new} is renamed, but gerdr couldn't write all it keeps of it"),
                crate::toast::Level::Warning,
                Some(("Open log".to_owned(), MenuAction::OpenLog)),
            );
        }
        self.report_machines();
        Ok(())
    }

    /// The other saved machines that are machine `name` too, as the ids
    /// each was seen by say.
    pub(crate) fn same_machines_as(&self, name: &str) -> Vec<String> {
        let ids: BTreeMap<String, Vec<String>> = (self.machines.saved.keys())
            .map(|n| (n.clone(), self.machine_ids(n)))
            .collect();
        same_machines(name, &ids)
    }

    /// Why machine `drop` can't merge into `keep` now, as a toast says it.
    fn merge_refused(&self, keep: &str, drop: &str) -> Option<String> {
        if let Some(gone) = [keep, drop]
            .into_iter()
            .find(|n| !self.machines.saved.contains_key(*n))
        {
            return Some(format!("{gone} isn't in the settings any more"));
        }
        if keep == drop {
            return Some(format!("{keep} is one machine already"));
        }
        let busy = [keep, drop].into_iter().find(|n| self.machine_busy(n))?;
        Some(format!("Wait for {busy} to connect first"))
    }

    /// The Merge button of machine `keep`'s detail: asks first, as it has no
    /// undo.
    pub(crate) fn ask_merge_machines(&mut self, keep: &str, drop: &str) {
        if let Some(why) = self.merge_refused(keep, drop) {
            return self.show_warning(why);
        }
        self.ask(
            format!("Merge {drop} into {keep}?"),
            merge_body(keep, drop),
            "Merge",
            crate::confirm::Then::MergeMachines {
                keep: keep.to_owned(),
                drop: drop.to_owned(),
            },
        );
    }

    /// Machine `drop` is machine `keep`: its addresses join `keep`'s, after
    /// them, and it goes, without its link and without forgetting what was
    /// seen of it, as that is `keep`'s now. What was kept under its name
    /// moves to `keep`: the ids it was seen by and their `place.json`, and
    /// the projects' `machines` and `machine_folders`.
    pub(crate) fn merge_machines(&mut self, keep: &str, drop: &str) {
        if let Some(why) = self.merge_refused(keep, drop) {
            return self.show_warning(why);
        }
        let (Some(kept), Some(dropped)) = (
            self.machines.saved.get(keep).cloned(),
            self.machines.saved.get(drop).cloned(),
        ) else {
            return;
        };
        let joined = joined_addresses(&kept, &dropped);
        if let Err(why) = crate::config::write_machine(keep, Some(&joined)) {
            tracing::warn!("machines: merging {drop} into {keep}: {why}");
            return self.show_error(format!(
                "gerdr couldn't merge {drop} into {keep} in the settings: {why}"
            ));
        }
        let dropped_too = crate::config::write_machine(drop, None);
        // Written here: the settings read back don't take it out again.
        self.settings_modified = crate::config::modified();
        if let Err(why) = dropped_too {
            // `keep` has its addresses already: both stay, as the file says.
            tracing::warn!("machines: taking {drop} out of the settings: {why}");
            self.machines.saved.insert(keep.to_owned(), joined.clone());
            if let Some(link) = self.machines.links.get_mut(keep) {
                link.addresses = joined;
            }
            return self.show_error(format!(
                "{keep} has {drop}'s addresses now, but gerdr couldn't take {drop} out of the settings: {why}"
            ));
        }
        tracing::info!("machines: {drop} merged into {keep}, at {joined}");
        self.machines.saved.insert(keep.to_owned(), joined.clone());
        self.machines.saved.remove(drop);
        self.machines.link_off(drop);
        self.machines.links.remove(drop);
        if let Some(link) = self.machines.links.get_mut(keep) {
            link.addresses = joined.clone();
        }
        let ids = move_names(&mut self.machines.names, keep, drop);
        self.merge_remembered(keep, drop);
        let mut trouble = false;
        let data = crate::projects::data_dir();
        for id in &ids {
            if let Err(err) = view::rename_place(&view::root(&data, id), drop, keep) {
                tracing::warn!("machine {keep}: noting {id} under its name: {err}");
                trouble = true;
            }
        }
        let mut changed = Vec::new();
        for project in &mut self.projects.projects {
            let (moved, clashes) = merge_workflow(&mut project.workflow, keep, drop);
            for folder in clashes {
                tracing::warn!(
                    "machines: {} keeps {keep}'s place of {folder}, not {drop}'s",
                    project.id
                );
            }
            if moved {
                changed.push(project.id.clone());
            }
        }
        if !changed.is_empty() {
            tracing::info!(
                "machines: {drop} merged into {keep} in the files of {}",
                changed.join(", ")
            );
            if !self.projects.save_projects(&changed) {
                tracing::warn!("machines: couldn't save the merge of {drop} in {changed:?}");
                trouble = true;
            }
        }
        let up = matches!(
            self.machines.links.get(keep).map(|l| &l.state),
            Some(LinkState::Up(_))
        );
        if !up {
            self.start_link(keep.to_owned(), joined, true);
        }
        let row = self.machines.saved.keys().position(|n| n == keep);
        let count = self.machines.saved.len() + 1;
        let page = crate::chrome::machines_shown(count, self.output_size());
        if let (Some(panel), Some(row)) = (self.machines_panel.as_mut(), row) {
            panel.merged(drop, row, (page, count));
        }
        self.starts_lost();
        self.report_machines();
        match trouble {
            true => self.toast_with(
                format!("Merged {drop} into {keep}, but gerdr couldn't write all it keeps of it"),
                crate::toast::Level::Warning,
                Some(("Open log".to_owned(), MenuAction::OpenLog)),
            ),
            false => self.show_success(format!("Merged {drop} into {keep}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved() -> BTreeMap<String, Addresses> {
        BTreeMap::from([
            ("box".to_owned(), "me@box".into()),
            ("zen".to_owned(), "me@zen".into()),
        ])
    }

    #[test]
    fn the_form_says_what_is_wrong_and_where() {
        let saved = saved();
        let check = |name: &str, address: &str| check_edit(&saved, "box", name, &[address]);
        let at = |why: &str, field| Err((why.to_owned(), field));
        assert_eq!(
            check("box", " "),
            at(
                "Say which machine, as user@host.",
                MachinesFocus::EditAddress(0)
            )
        );
        assert_eq!(
            check("box", "-oProxyCommand=x"),
            at(&Failure::BadTarget.why(), MachinesFocus::EditAddress(0))
        );
        assert_eq!(
            check("box", "me@zen"),
            at("It is here already, as zen.", MachinesFocus::EditAddress(0))
        );
        assert_eq!(
            check("zen", "me@box2"),
            at("Another machine is called zen.", MachinesFocus::EditName)
        );
        // Its own address and name are no trouble; no name is its host's.
        assert_eq!(
            check(" box ", "me@box"),
            Ok(("box".into(), "me@box".into()))
        );
        assert_eq!(
            check("", "me@new.lan"),
            Ok(("new.lan".into(), "me@new.lan".into()))
        );
        assert_eq!(
            check("crate", "me@box"),
            Ok(("crate".into(), "me@box".into()))
        );
    }

    #[test]
    fn only_the_same_machine_answers_for_it() {
        // Never seen: whatever answers is it.
        assert!(same_machine(&[], "abc"));
        let known = ["abc".to_owned(), "def".to_owned()];
        assert!(same_machine(&known, "def"));
        assert!(!same_machine(&known, "xyz"));
        assert!(!same_machine(&known, ""));
        let why = Failure::OtherMachine("box".into()).why();
        assert_eq!(why, "That address reaches another machine, not box.");
    }

    #[test]
    fn a_draft_is_what_differs_and_save_anyway_follows_the_failure() {
        let from = ("box".to_owned(), vec!["me@box".to_owned()]);
        let mut form = EditForm::new("box", ("box", &from.1), from.clone());
        assert_eq!(form.draft(), None);
        form.addresses[0].set("me@box2");
        assert_eq!(form.draft(), Some(("box".into(), vec!["me@box2".into()])));
        assert!(!form.untried_offered());
        form.failed = Some(("me@box2".into(), Failure::Auth));
        assert!(form.untried_offered());
        assert!(form.fix().is_some());
        form.failed = Some(("me@box2".into(), Failure::OtherMachine("box".into())));
        assert!(!form.untried_offered());
        assert!(form.fix().is_none());
        // A form with no address still has a field to type one in.
        let empty = EditForm::new("box", ("box", &[]), from);
        assert_eq!(empty.typed(), [""]);
    }

    #[test]
    fn each_address_of_the_list_is_checked() {
        let saved = saved();
        let check = |addresses: &[&str]| check_edit(&saved, "box", "box", addresses);
        let at = |why: &str, i| Err((why.to_owned(), MachinesFocus::EditAddress(i)));
        // Trimmed, empty ones left out, in the order given.
        assert_eq!(
            check(&[" me@box.lan ", "", "me@box"]),
            Ok((
                "box".into(),
                Addresses(vec!["me@box.lan".into(), "me@box".into()])
            ))
        );
        assert_eq!(check(&["", " "]), at("Say which machine, as user@host.", 0));
        // The one at fault is named by its field, empty ones counted.
        assert_eq!(
            check(&["me@box", "", "-oProxyCommand=x"]),
            at(&Failure::BadTarget.why(), 2)
        );
        assert_eq!(
            check(&["me@box", " me@box"]),
            at("me@box is in the list twice.", 1)
        );
        assert_eq!(
            check(&["me@box", "me@zen"]),
            at("It is here already, as zen.", 1)
        );
        // No name is the first address's host.
        assert_eq!(
            check_edit(&saved, "box", "", &["", "me@new.lan", "me@x"]).map(|(n, _)| n),
            Ok("new.lan".into())
        );
    }

    #[test]
    fn the_list_adds_moves_and_keeps_one() {
        let from = ("box".to_owned(), vec!["a@1".to_owned(), "a@2".to_owned()]);
        let mut form = EditForm::new("box", ("box", &from.1), from.clone());
        form.error = Some("x".into());
        assert_eq!(form.add_address(), 2);
        assert_eq!(form.error, None);
        assert_eq!(form.move_address(0, false), None);
        assert_eq!(form.move_address(2, true), None);
        assert_eq!(form.move_address(0, true), Some(1));
        assert_eq!(form.typed(), ["a@2", "a@1", ""]);
        assert_eq!(form.field_of("a@1"), 1);
        assert_eq!(form.field_of("gone"), 0);
        assert!(form.remove_address(2) && form.remove_address(0));
        assert!(!form.remove_address(0));
        assert_eq!(form.typed(), ["a@1"]);
    }

    #[test]
    fn a_new_list_is_tried_only_when_it_needs_to_be() {
        let old = Addresses(vec!["a@1".into(), "a@2".into()]);
        let list = |a: &[&str]| Addresses(a.iter().map(|a| (*a).to_owned()).collect());
        let reordered = list(&["a@2", "a@1"]);
        let added = list(&["a@1", "a@2", "a@3"]);
        let swapped = list(&["a@3"]);
        // Up on an address still there: kept at once, the link left.
        assert!(keeps_without_try(&old, &reordered, Some("a@1"), true));
        assert!(keeps_without_try(&old, &added, Some("a@2"), true));
        // Up on one taken out, or connecting: tried.
        assert!(!keeps_without_try(&old, &list(&["a@2"]), Some("a@1"), true));
        assert!(!keeps_without_try(&old, &reordered, None, true));
        // Off: only what was there already goes untried.
        assert!(keeps_without_try(&old, &reordered, None, false));
        assert!(!keeps_without_try(&old, &added, None, false));
        assert!(!keeps_without_try(&old, &swapped, Some("a@1"), true));
    }

    #[test]
    fn a_merge_joins_addresses_names_and_folders() {
        let keep = Addresses(vec!["me@zen-office".into(), "me@10.0.0.4".into()]);
        let drop = Addresses(vec!["me@zen-home".into(), "me@10.0.0.4".into()]);
        assert_eq!(
            joined_addresses(&keep, &drop),
            Addresses(vec![
                "me@zen-office".into(),
                "me@10.0.0.4".into(),
                "me@zen-home".into()
            ])
        );
        let mut names = HashMap::from([
            ("id2".to_owned(), "home".to_owned()),
            ("id1".to_owned(), "home".to_owned()),
            ("id3".to_owned(), "office".to_owned()),
        ]);
        assert_eq!(move_names(&mut names, "office", "home"), ["id1", "id2"]);
        assert!(names.values().all(|n| n == "office"));
        let mut workflow = Workflow::default();
        workflow
            .machines
            .insert(crate::feature::Stage::Build, "home".into());
        workflow
            .machines
            .insert(crate::feature::Stage::Plan, "other".into());
        let folders = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            (pairs.iter())
                .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
                .collect()
        };
        (workflow.machine_folders)
            .insert("home".into(), folders(&[("/a", "/h/a"), ("/b", "/h/b")]));
        (workflow.machine_folders).insert("office".into(), folders(&[("/a", "/o/a")]));
        let (changed, clashes) = merge_workflow(&mut workflow, "office", "home");
        assert!(changed);
        assert_eq!(clashes, ["/a"]);
        assert_eq!(workflow.machines[&crate::feature::Stage::Build], "office");
        assert_eq!(workflow.machines[&crate::feature::Stage::Plan], "other");
        assert_eq!(
            workflow.machine_folders["office"],
            folders(&[("/a", "/o/a"), ("/b", "/h/b")])
        );
        assert!(!workflow.machine_folders.contains_key("home"));
        // Nothing of it there: nothing to save.
        assert_eq!(
            merge_workflow(&mut workflow, "office", "home"),
            (false, vec![])
        );
    }

    #[test]
    fn machines_seen_by_one_id_are_the_same() {
        let ids = BTreeMap::from([
            ("home".to_owned(), vec!["id1".to_owned()]),
            (
                "office".to_owned(),
                vec!["id0".to_owned(), "id1".to_owned()],
            ),
            ("box".to_owned(), vec!["id2".to_owned()]),
            ("new".to_owned(), vec![]),
        ]);
        assert_eq!(same_machines("home", &ids), ["office"]);
        assert_eq!(same_machines("office", &ids), ["home"]);
        assert!(same_machines("box", &ids).is_empty());
        assert!(same_machines("new", &ids).is_empty());
        assert!(same_machines("gone", &ids).is_empty());
        let body = merge_body("office", "home");
        assert_eq!(
            body,
            "home's addresses join office's, after them, and home goes. \
             Projects that name home name office instead."
        );
    }
}

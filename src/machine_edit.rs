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

use std::collections::BTreeMap;

use crate::{
    Gerdr,
    chrome::MachinesFocus,
    feature::view,
    machines::{Failure, Fix, LinkInfo, LinkState, default_name, valid_target},
    menu::MenuAction,
    text_field::TextField,
};

/// The form editing a machine, while it is open in the machines panel.
#[derive(Debug, Default)]
pub struct EditForm {
    /// The machine edited, by its name now.
    pub machine: String,
    pub name: TextField,
    pub address: TextField,
    /// What the form held when it opened: a draft is kept only when it
    /// says something else.
    pub from: (String, String),
    /// What is wrong with the form.
    pub error: Option<String>,
    /// Why the last try of a new address failed: the address, and what
    /// happened.
    pub failed: Option<(String, Failure)>,
}

impl EditForm {
    /// The form on machine `machine`, holding `name` and `address`, as saved
    /// at `from`.
    pub fn new(machine: &str, (name, address): (&str, &str), from: (String, String)) -> Self {
        let mut form = Self {
            machine: machine.to_owned(),
            from,
            ..Self::default()
        };
        form.name.set(name);
        form.address.set(address);
        form
    }

    /// What was typed, if it isn't what the form opened with: name, address.
    pub fn draft(&self) -> Option<(String, String)> {
        let typed = (self.name.to_string(), self.address.to_string());
        (typed != self.from).then_some(typed)
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
    pub old_target: String,
    pub was_live: bool,
    /// What the form asked for.
    pub new_name: String,
    pub new_target: String,
    /// The ids it was seen by: the one answering must be one of them.
    pub known: Vec<String>,
    /// How far trying got.
    pub progress: String,
}

/// What the form says, checked against the machines saved, machine
/// `machine` among them: the name and address to keep, or what is wrong
/// and the field at fault.
pub fn check_edit(
    saved: &BTreeMap<String, String>,
    machine: &str,
    name: &str,
    address: &str,
) -> Result<(String, String), (String, MachinesFocus)> {
    let name = name.trim();
    let address = address.trim();
    let at_address = |why: String| Err((why, MachinesFocus::EditAddress));
    if address.is_empty() {
        return at_address("Say which machine, as user@host.".to_owned());
    }
    if !valid_target(address) {
        return at_address(Failure::BadTarget.why());
    }
    if let Some((other, _)) = (saved.iter()).find(|(n, t)| *t == address && *n != machine) {
        return at_address(format!("It is here already, as {other}."));
    }
    let name = match name.is_empty() {
        true => default_name(address),
        false => name.to_owned(),
    };
    if name != machine && saved.contains_key(&name) {
        return Err((
            format!("Another machine is called {name}."),
            MachinesFocus::EditName,
        ));
    }
    Ok((name, address.to_owned()))
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

    /// Save the form: a new name at once, a new address once tried, or
    /// untried with `untried` ("Save anyway").
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
        let checked = check_edit(&self.machines.saved, &machine, &form.name, &form.address);
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
        if untried {
            return self.save_untried(&machine, &name, &target);
        }
        let known = self.machine_ids(&machine);
        let link = self.machines.links.get(&machine);
        let was_live = matches!(
            link.map(|l| &l.state),
            Some(LinkState::Up(_) | LinkState::Connecting(_) | LinkState::Retrying(_))
        );
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

    /// "Save anyway": machine `machine` called `name` and kept at `target`
    /// without trying it first, its link starting there.
    fn save_untried(&mut self, machine: &str, name: &str, target: &str) {
        if name != machine
            && let Err(why) = self.rename_machine(machine, name)
        {
            return self.edit_error(why, MachinesFocus::EditName);
        }
        if let Err(why) = crate::config::write_machine(name, Some(target)) {
            tracing::warn!("machines: saving {name} at {target}: {why}");
            let why = format!("gerdr couldn't keep it in the settings: {why}.");
            return self.edit_error(why, MachinesFocus::EditAddress);
        }
        self.settings_modified = crate::config::modified();
        (self.machines.saved).insert(name.to_owned(), target.to_owned());
        self.start_link(name.to_owned(), target.to_owned(), true);
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
        self.edit_not_kept(tried, Failure::OtherMachine(name.to_owned()));
        None
    }

    /// The new address of machine `name` didn't connect: the old link goes
    /// back, and the form says why.
    pub(crate) fn edit_failed(&mut self, name: &str, failure: Failure) {
        let Some(tried) = self.machines.editing.take() else {
            return;
        };
        tracing::warn!("machine {name}: trying {}: {failure:?}", tried.new_target);
        self.restore_link(&tried);
        self.edit_not_kept(tried, failure);
    }

    /// What the try ran into, in the form; in a toast if it was closed.
    fn edit_not_kept(&mut self, tried: EditTry, failure: Failure) {
        let name = tried.name.clone();
        match self.edit_form_mut(&name) {
            Some(form) => {
                form.failed = Some((tried.new_target, failure));
                if let Some(panel) = self.machines_panel.as_mut() {
                    panel.focus = MachinesFocus::EditAddress;
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
            tracing::warn!("machines: saving {name} at {new_target}: {why}");
            return self.show_error(format!(
                "Connected to {name} at {new_target}, but gerdr couldn't keep it in the settings: {why}"
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
        let connecting = matches!(
            self.machines.links.get(old).map(|l| &l.state),
            Some(LinkState::Connecting(_))
        );
        if connecting || self.edit_try().is_some_and(|t| t.name == old) {
            return Err(format!("Wait for {old} to connect first."));
        }
        crate::config::rename_machine(old, new).map_err(|why| {
            tracing::warn!("machines: renaming {old} to {new} in the settings: {why}");
            format!("gerdr couldn't rename it in the settings: {why}.")
        })?;
        // Written here: the settings read back don't take the old name out.
        self.settings_modified = crate::config::modified();
        let ids = self.machines.rename(old, new);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("box".to_owned(), "me@box".to_owned()),
            ("zen".to_owned(), "me@zen".to_owned()),
        ])
    }

    #[test]
    fn the_form_says_what_is_wrong_and_where() {
        let saved = saved();
        let check = |name: &str, address: &str| check_edit(&saved, "box", name, address);
        let at = |why: &str, field| Err((why.to_owned(), field));
        assert_eq!(
            check("box", " "),
            at(
                "Say which machine, as user@host.",
                MachinesFocus::EditAddress
            )
        );
        assert_eq!(
            check("box", "-oProxyCommand=x"),
            at(&Failure::BadTarget.why(), MachinesFocus::EditAddress)
        );
        assert_eq!(
            check("box", "me@zen"),
            at("It is here already, as zen.", MachinesFocus::EditAddress)
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
        let from = ("box".to_owned(), "me@box".to_owned());
        let mut form = EditForm::new("box", ("box", "me@box"), from.clone());
        assert_eq!(form.draft(), None);
        form.address.set("me@box2");
        assert_eq!(form.draft(), Some(("box".into(), "me@box2".into())));
        assert!(!form.untried_offered());
        form.failed = Some(("me@box2".into(), Failure::Auth));
        assert!(form.untried_offered());
        assert!(form.fix().is_some());
        form.failed = Some(("me@box2".into(), Failure::OtherMachine("box".into())));
        assert!(!form.untried_offered());
        assert!(form.fix().is_none());
    }
}

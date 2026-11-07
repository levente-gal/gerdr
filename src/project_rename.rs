//! Changing a project's id (Shift+F2 in the project menu, or its row's
//! button). The id names the project's file and its folders of data, and
//! other records name it, so all of them change together, or none do.
//!
//! Under the data folder, `<id>` becomes the new id in:
//! - `projects/<id>.json`, and the `id` in it (by the store's own save);
//! - `features/<id>/` and `features/<id>.lock`, `ai-work/<id>/` and
//!   `ai-work/<id>.lock`, `instances/<id>/` and `instances/<id>@<socket>/`,
//!   `inbox/<id>.json`, `reviews/<id>.json`;
//! - inside the JSON and JSON lines files of every feature, of this project
//!   and of those linking it, of its AI work and of its inbox (`Rewrite`):
//!   a `project` naming it, repository keys `<id>:<name>`, inbox keys
//!   `service:<id>|…`, and paths into the folders above (agents' commands,
//!   prompts);
//! - each feature folder's `feature-docs` link;
//! - `state.json` and `frecency.json`, through what gerdr holds of them.
//!
//! What stays: the herdr session's name (`Project::session` is kept, not
//! made from the id), the hooks' logs, the AI work stats' index (it draws
//! itself again from the folders), the copies of other machines' features
//! under `machines/`, and anything on other machines. Those show this
//! project's features again once they use the same id.
//!
//! It is refused while the project is shown, open in another gerdr, being
//! stopped, has a sandbox (its containers carry the old id) or work running
//! on another machine, while a program still uses its folders, or while its
//! file changed outside gerdr. A project still running asks to be stopped
//! first.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::{Gerdr, confirm::Then, feature::Feature, keep, menu::MenuAction, toast::Level};

/// The folders under the data folder holding a folder per project.
const FOLDERS: [&str; 3] = ["features", "ai-work", "instances"];
/// The folders under the data folder holding a file per project.
const FILES: [&str; 2] = ["inbox", "reviews"];
/// The inbox keys that name a project: `<kind><id>|…`.
const INBOX_KEYS: [&str; 3] = ["service:", "ignored:", "gate:"];

/// Whether `b` can be part of an id.
fn id_byte(b: u8) -> bool {
    b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'
}

/// `text` with each `from` that no id character follows made `to`; `None`
/// if there is none.
fn replace_bounded(text: &str, from: &str, to: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut changed = false;
    while let Some(at) = rest.find(from) {
        let end = at + from.len();
        out.push_str(&rest[..at]);
        if rest.as_bytes().get(end).is_none_or(|b| !id_byte(*b)) {
            out.push_str(to);
            changed = true;
        } else {
            out.push_str(from);
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    changed.then_some(out)
}

/// How what names project `old` comes to name `new`.
#[derive(Clone, Debug)]
pub struct Rewrite {
    pub old: String,
    pub new: String,
    /// The data folder, as written in paths: no `/` at the end.
    data: String,
}

impl Rewrite {
    pub fn new(old: &str, new: &str, data: &Path) -> Self {
        let data = data.to_string_lossy();
        Self {
            old: old.to_owned(),
            new: new.to_owned(),
            data: data.trim_end_matches('/').to_owned(),
        }
    }

    /// The same, the other way.
    pub fn back(&self) -> Self {
        Self {
            old: self.new.clone(),
            new: self.old.clone(),
            data: self.data.clone(),
        }
    }

    /// What string `text` becomes, the value of `key` in its object if
    /// any; `None` if it stays.
    fn text(&self, text: &str, key: Option<&str>) -> Option<String> {
        let (old, new) = (self.old.as_str(), self.new.as_str());
        if key == Some("project") && text == old {
            return Some(new.to_owned());
        }
        // A repository of a linked project: `<project>:<name>`. Text
        // written by people has spaces.
        if let Some(name) = text.strip_prefix(old).and_then(|r| r.strip_prefix(':'))
            && !text.contains(char::is_whitespace)
        {
            return Some(format!("{new}:{name}"));
        }
        for kind in INBOX_KEYS {
            let rest = text.strip_prefix(kind).and_then(|r| r.strip_prefix(old));
            if let Some(rest) = rest.and_then(|r| r.strip_prefix('|')) {
                return Some(format!("{kind}{new}|{rest}"));
            }
        }
        let mut out: Option<String> = None;
        for folder in FOLDERS {
            let from = format!("{}/{folder}/{old}", self.data);
            let to = format!("{}/{folder}/{new}", self.data);
            if let Some(done) = replace_bounded(out.as_deref().unwrap_or(text), &from, &to) {
                out = Some(done);
            }
        }
        out
    }

    /// Change what in `value` names the old id, `key` being what its object
    /// calls it; whether anything changed.
    pub fn value(&self, value: &mut Value, key: Option<&str>) -> bool {
        match value {
            Value::String(text) => match self.text(text, key) {
                Some(done) => {
                    *text = done;
                    true
                }
                None => false,
            },
            Value::Array(items) => items
                .iter_mut()
                .fold(false, |changed, item| self.value(item, None) | changed),
            Value::Object(map) => {
                let mut changed = false;
                for (k, mut v) in std::mem::take(map) {
                    let k = match self.text(&k, None) {
                        Some(done) => {
                            changed = true;
                            done
                        }
                        None => k,
                    };
                    changed |= self.value(&mut v, Some(&k));
                    map.insert(k, v);
                }
                changed
            }
            _ => false,
        }
    }

    /// Change what in `thing` names the old id; whether anything changed.
    pub fn typed<T: Serialize + DeserializeOwned>(&self, thing: &mut T) -> Result<bool, String> {
        let mut value = serde_json::to_value(&*thing).map_err(|err| err.to_string())?;
        if !self.value(&mut value, None) {
            return Ok(false);
        }
        *thing = serde_json::from_value(value).map_err(|err| err.to_string())?;
        Ok(true)
    }

    /// File `bytes` changed, as JSON or as JSON lines (`lines`); `None` if
    /// nothing in it names the old id. A line that isn't JSON stays as it is.
    fn file(&self, bytes: &[u8], lines: bool) -> Result<Option<Vec<u8>>, String> {
        if !contains(bytes, self.old.as_bytes()) {
            return Ok(None);
        }
        let text = std::str::from_utf8(bytes).map_err(|err| err.to_string())?;
        if !lines {
            let mut value: Value = serde_json::from_str(text).map_err(|err| err.to_string())?;
            if !self.value(&mut value, None) {
                return Ok(None);
            }
            let mut out = serde_json::to_vec_pretty(&value).map_err(|err| err.to_string())?;
            if text.ends_with('\n') {
                out.push(b'\n');
            }
            return Ok(Some(out));
        }
        let mut changed = false;
        let mut out = String::with_capacity(text.len());
        for line in text.split_inclusive('\n') {
            let (body, end) = match line.strip_suffix('\n') {
                Some(body) => (body, "\n"),
                None => (line, ""),
            };
            let named = contains(body.as_bytes(), self.old.as_bytes());
            let mut value = serde_json::from_str::<Value>(body).ok().filter(|_| named);
            let done = value.as_mut().is_some_and(|value| self.value(value, None));
            match value {
                Some(value) if done => {
                    out.push_str(&serde_json::to_string(&value).map_err(|err| err.to_string())?);
                    changed = true;
                }
                _ => out.push_str(body),
            }
            out.push_str(end);
        }
        Ok(changed.then(|| out.into_bytes()))
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

/// What `move_data` did, to take back in reverse.
#[derive(Debug)]
enum Step {
    /// Renamed from the first to the second.
    Moved(PathBuf, PathBuf),
    /// Wrote over this file, which held these bytes.
    Wrote(PathBuf, Vec<u8>),
    /// Pointed this link elsewhere; it pointed here.
    Linked(PathBuf, PathBuf),
}

/// What `move_data` did.
#[derive(Debug, Default)]
pub struct Moved {
    steps: Vec<Step>,
}

impl Moved {
    /// Put everything back as it was; what couldn't be.
    pub fn undo(self) -> Vec<String> {
        let mut failed = Vec::new();
        for step in self.steps.into_iter().rev() {
            let done = match &step {
                Step::Moved(from, to) => fs::rename(to, from),
                Step::Wrote(path, bytes) => crate::projects::write_atomic(path, bytes),
                Step::Linked(link, target) => relink(link, target),
            };
            if let Err(err) = done {
                tracing::error!("changing a project's id back: {step:?}: {err}");
                failed.push(format!("{step:?}: {err}"));
            }
        }
        failed
    }
}

fn relink(link: &Path, target: &Path) -> std::io::Result<()> {
    fs::remove_file(link)?;
    std::os::unix::fs::symlink(target, link)
}

/// What is named by the old id under `data`, and where each goes: only
/// those there.
fn moves(data: &Path, rewrite: &Rewrite) -> Vec<(PathBuf, PathBuf)> {
    let (old, new) = (&rewrite.old, &rewrite.new);
    let mut moves = Vec::new();
    for folder in FOLDERS {
        let dir = data.join(folder);
        moves.push((dir.join(old), dir.join(new)));
        moves.push((
            dir.join(format!("{old}.lock")),
            dir.join(format!("{new}.lock")),
        ));
    }
    // An `--attach` gerdr's instances of it: `<id>@<socket name>`.
    let instances = data.join("instances");
    let prefix = format!("{old}@");
    let mut attached: Vec<String> = fs::read_dir(&instances)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
        .filter(|name| name.starts_with(&prefix))
        .collect();
    attached.sort();
    for name in attached {
        let socket = &name[prefix.len()..];
        moves.push((
            instances.join(&name),
            instances.join(format!("{new}@{socket}")),
        ));
    }
    for folder in FILES {
        let dir = data.join(folder);
        moves.push((
            dir.join(format!("{old}.json")),
            dir.join(format!("{new}.json")),
        ));
    }
    moves.retain(|(from, _)| from.symlink_metadata().is_ok());
    moves
}

/// The folders whose files may name the old id once moved: every
/// feature's (of every project), this project's AI work, and its inbox
/// file. Only their own files are read, not those in folders under them
/// (a sandbox's browser profile, say).
fn rewritten_files(data: &Path, rewrite: &Rewrite) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let features = data.join("features");
    for project in subdirs(&features) {
        dirs.extend(subdirs(&project));
    }
    dirs.extend(subdirs(&data.join("ai-work").join(&rewrite.new)));
    let mut files: Vec<PathBuf> = dirs
        .iter()
        .flat_map(|dir| fs::read_dir(dir).into_iter().flatten().flatten())
        .map(|e| e.path())
        .filter(|p| json_kind(p).is_some() && p.is_file())
        .collect();
    let inbox = data.join("inbox").join(format!("{}.json", rewrite.new));
    if inbox.is_file() {
        files.push(inbox);
    }
    files.sort();
    files
}

/// Whether `path` is JSON (`Some(false)`) or JSON lines (`Some(true)`).
fn json_kind(path: &Path) -> Option<bool> {
    match path.extension()?.to_str()? {
        "json" => Some(false),
        "jsonl" => Some(true),
        _ => None,
    }
}

/// The folders in `dir`, in name order; none if it isn't there or can't
/// be listed (logged: its files keep the old id).
fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            if err.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("changing a project's id: listing {}: {err}", dir.display());
            }
            return Vec::new();
        }
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    dirs.sort();
    dirs
}

/// What is already there that the new id would name; `None` if nothing.
pub fn taken_on_disk(data: &Path, new: &str) -> Option<PathBuf> {
    let mut paths = vec![data.join("projects").join(format!("{new}.json"))];
    for folder in FOLDERS {
        paths.push(data.join(folder).join(new));
        paths.push(data.join(folder).join(format!("{new}.lock")));
    }
    for folder in FILES {
        paths.push(data.join(folder).join(format!("{new}.json")));
    }
    let attached = format!("{new}@");
    paths.extend(
        fs::read_dir(data.join("instances"))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(&attached))
            .map(|e| e.path()),
    );
    paths.into_iter().find(|p| p.symlink_metadata().is_ok())
}

/// Move what is named by `rewrite.old` under `data` to `rewrite.new`, and
/// change what names it inside (see the module's doc); all or nothing.
/// Not the project's own file: the store saves that.
pub fn move_data(data: &Path, rewrite: &Rewrite) -> Result<Moved, String> {
    if let Some(there) = taken_on_disk(data, &rewrite.new) {
        return Err(format!("{} is already there", there.display()));
    }
    let mut moved = Moved::default();
    match move_steps(data, rewrite, &mut moved) {
        Ok(()) => Ok(moved),
        Err(why) => {
            let failed = moved.undo();
            if failed.is_empty() {
                Err(why)
            } else {
                Err(format!(
                    "{why}; and putting it back failed: {}",
                    failed.join("; ")
                ))
            }
        }
    }
}

fn move_steps(data: &Path, rewrite: &Rewrite, moved: &mut Moved) -> Result<(), String> {
    for (from, to) in moves(data, rewrite) {
        fs::rename(&from, &to).map_err(|err| format!("couldn't move {}: {err}", from.display()))?;
        moved.steps.push(Step::Moved(from, to));
    }
    for path in rewritten_files(data, rewrite) {
        let lines = json_kind(&path) == Some(true);
        let bytes =
            fs::read(&path).map_err(|err| format!("couldn't read {}: {err}", path.display()))?;
        let done = match rewrite.file(&bytes, lines) {
            Ok(done) => done,
            // A file that can't be read as JSON is left as it is: what reads
            // it sets it aside the same way.
            Err(why) => {
                tracing::warn!(
                    "changing a project's id: not changing {}: {why}",
                    path.display()
                );
                continue;
            }
        };
        if let Some(done) = done {
            crate::projects::write_atomic(&path, &done)
                .map_err(|err| format!("couldn't write {}: {err}", path.display()))?;
            moved.steps.push(Step::Wrote(path, bytes));
        }
    }
    let features = data.join("features");
    let (old_dir, new_dir) = (features.join(&rewrite.old), features.join(&rewrite.new));
    for slug in subdirs(&new_dir) {
        let Some(name) = slug.file_name() else {
            continue;
        };
        let Ok(text) = fs::read_to_string(slug.join("feature.json")) else {
            continue;
        };
        let Some(dir) = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v.get("dir")?.as_str().map(PathBuf::from))
        else {
            continue;
        };
        let link = dir.join(crate::feature::layout::DOCS_LINK);
        let was = old_dir.join(name);
        if fs::read_link(&link).ok().as_deref() == Some(was.as_path()) {
            relink(&link, &slug)
                .map_err(|err| format!("couldn't link {}: {err}", link.display()))?;
            moved.steps.push(Step::Linked(link, was));
        }
    }
    Ok(())
}

/// The programs whose command line names the folders of project `old`
/// under `data` (a Zed or browser on its profile, an agent on its
/// features), by name; not this gerdr.
pub fn users(data: &Path, old: &str) -> Vec<String> {
    let data = data.to_string_lossy();
    let data = data.trim_end_matches('/');
    let paths: Vec<String> = FOLDERS
        .iter()
        .map(|folder| format!("{data}/{folder}/{old}"))
        .collect();
    let me = std::process::id().to_string();
    let mut names = BTreeSet::new();
    for entry in fs::read_dir("/proc").into_iter().flatten().flatten() {
        let pid = entry.file_name();
        let pid = pid.to_string_lossy();
        if pid == me || !pid.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(line) = fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        let line = String::from_utf8_lossy(&line).replace('\0', " ");
        if !paths
            .iter()
            .any(|p| replace_bounded(&line, p, "").is_some())
        {
            continue;
        }
        let name = fs::read_to_string(entry.path().join("comm")).unwrap_or_default();
        names.insert(name.trim().to_owned());
    }
    names.into_iter().filter(|n| !n.is_empty()).collect()
}

/// Why feature `f`'s project can't change its id now, if it can't: a
/// sandbox (its containers carry the old id) or work running on another
/// machine (its files there are named by the old id).
fn feature_holds(f: &Feature) -> Option<String> {
    let title = &f.title;
    if f.sandbox.is_some() {
        return Some(format!(
            "Remove the sandbox of \u{201c}{title}\u{201d} first: its containers carry the old id"
        ));
    }
    let agent = f
        .runs
        .values()
        .any(|r| r.machine.is_some() && r.pane.is_some());
    let setup = (f.setup.as_ref()).is_some_and(|r| r.machine.is_some() && r.finished.is_none());
    let verify = (f.verify.as_ref()).is_some_and(|r| r.machine.is_some() && r.finished.is_none());
    (agent || setup || verify)
        .then(|| format!("\u{201c}{title}\u{201d} has work on another machine: let it end first"))
}

/// Whether feature `f` ran anything on another machine.
fn ran_elsewhere(f: &Feature) -> bool {
    f.runs.values().any(|r| r.machine.is_some())
        || f.setup.as_ref().is_some_and(|r| r.machine.is_some())
        || f.verify.as_ref().is_some_and(|r| r.machine.is_some())
}

impl Gerdr {
    /// Change project `id`'s id to `new`, asking first while it runs: it is
    /// stopped first. Says why when it can't.
    pub fn change_project_id(&mut self, id: &str, new: &str) {
        let Some(name) = self.projects.project(id).map(|p| p.name.clone()) else {
            return tracing::warn!("no project {id} to change the id of");
        };
        if id == new {
            return;
        }
        if let Err(why) = self.can_change_id(id, new) {
            return self.show_warning(why);
        }
        if !self.session_runs(id) && keep::app_names(id).is_empty() {
            return self.change_project_id_now(id, new);
        }
        self.ask(
            format!("Change the id of \u{201c}{name}\u{201d}?"),
            "It still runs: changing its id stops its terminals, agents and apps".to_owned(),
            "Stop and change id",
            Then::ChangeProjectId(id.to_owned(), new.to_owned()),
        );
    }

    /// Why project `id` can't have its id changed to `new` now, if it can't.
    fn can_change_id(&self, id: &str, new: &str) -> Result<(), String> {
        let name = (self.projects.project(id).map(|p| p.name.as_str())).unwrap_or(id);
        if !crate::projects::valid_id(new) {
            return Err(format!(
                "{new:?} can't be an id: only lowercase letters, digits and dashes, not first a dash"
            ));
        }
        if self.projects.project(new).is_some() {
            return Err(format!("Another project has the id {new}"));
        }
        if self.project.as_deref() == Some(id) {
            return Err(format!(
                "Switch to another project to change {name}\u{2019}s id"
            ));
        }
        if self.stopping.contains(id) {
            return Err(format!("{name} is still stopping"));
        }
        if crate::project_lock::dir()
            .is_some_and(|dir| crate::project_lock::is_open_elsewhere(&dir, id))
        {
            return Err(format!("{name} is open in another gerdr"));
        }
        if self.projects.file_changed(id) {
            return Err(format!(
                "{name}\u{2019}s project file changed outside gerdr: reload it first"
            ));
        }
        let features = self.features.all.get(id).into_iter().flatten();
        if let Some(why) = features.filter_map(feature_holds).next() {
            return Err(why);
        }
        if let Some(there) = taken_on_disk(&crate::projects::data_dir(), new) {
            return Err(format!(
                "{} is already there",
                crate::paths::tilde(&there, None)
            ));
        }
        Ok(())
    }

    /// Change project `id`'s id to `new` without asking (see the module's
    /// doc): not while it runs.
    pub fn change_project_id_now(&mut self, id: &str, new: &str) {
        let Some(name) = self.projects.project(id).map(|p| p.name.clone()) else {
            return tracing::warn!("no project {id} to change the id of");
        };
        if let Err(why) = self.can_change_id(id, new) {
            return self.show_warning(why);
        }
        if self.session_runs(id) || !keep::app_names(id).is_empty() {
            return self.show_warning(format!("Stop {name} first: it still runs"));
        }
        let data = crate::projects::data_dir();
        let users = users(&data, id);
        if !users.is_empty() {
            return self.show_warning(format!(
                "Close what still uses {name}\u{2019}s folders first: {}",
                users.join(", ")
            ));
        }
        let elsewhere = (self.features.all.get(id).into_iter().flatten()).any(ran_elsewhere);
        let rewrite = Rewrite::new(id, new, &data);
        let moved = match move_data(&data, &rewrite) {
            Ok(moved) => moved,
            Err(why) => {
                tracing::error!("changing project {id}'s id to {new}: {why}");
                return self.show_error(format!("Couldn't change {name}\u{2019}s id: {why}"));
            }
        };
        if let Err(why) = self.projects.change_id(&rewrite) {
            let failed = moved.undo();
            tracing::error!("changing project {id}'s id to {new}: {why}; {failed:?}");
            return self.show_error(format!("Couldn't change {name}\u{2019}s id: {why}"));
        }
        if !self.projects.save_projects(&[new.to_owned()]) {
            let back = self.projects.change_id(&rewrite.back());
            let failed = moved.undo();
            self.projects.save();
            tracing::error!("saving project {new} after its id changed: {back:?}; {failed:?}");
            return self.show_error(format!(
                "Couldn't change {name}\u{2019}s id: its project file couldn't be saved"
            ));
        }
        self.renamed_in_memory(&rewrite);
        self.load_features();
        self.refresh_project_menu();
        let rows = self.project_rows();
        if let Some(welcome) = self.welcome.as_mut() {
            welcome.set_projects(rows);
        }
        let undo = MenuAction::ChangeProjectId(new.to_owned(), id.to_owned());
        self.toast_with(
            format!("Changed {name}\u{2019}s id to {new}"),
            Level::Success,
            Some(("Undo".to_owned(), undo)),
        );
        if elsewhere {
            self.show_warning(format!(
                "Other machines keep {name}\u{2019}s work under {id}: change its id there too"
            ));
        }
    }

    /// What gerdr holds of project `rewrite.old`, made `rewrite.new`'s.
    fn renamed_in_memory(&mut self, rewrite: &Rewrite) {
        let (old, new) = (rewrite.old.as_str(), rewrite.new.as_str());
        if let Some(running) = self.running.remove(old) {
            self.running.insert(new.to_owned(), running);
        }
        if let Some(held) = self.held.remove(old) {
            self.held.insert(new.to_owned(), held);
        }
        for set in [&mut self.order_pending, &mut self.starting] {
            if set.remove(old) {
                set.insert(new.to_owned());
            }
        }
        if self.previous_project.as_deref() == Some(old) {
            self.previous_project = Some(new.to_owned());
        }
        if self.just_created.as_deref() == Some(old) {
            self.just_created = Some(new.to_owned());
        }
        for nav in std::iter::once(&mut self.nav).chain(self.nav_kept.as_mut()) {
            if let Err(why) = rewrite.typed(nav) {
                tracing::warn!("changing project {old}'s id in the tab history: {why}");
            }
        }
        // Cached under the old id, they would be saved back there.
        if self.inbox.store.as_ref().is_some_and(|(id, _)| id == old) {
            self.inbox.store = None;
        }
        if (self.repos.kept.as_ref()).is_some_and(|(id, _)| id.as_deref() == Some(old)) {
            self.repos.kept = None;
        }
        if self
            .frecency
            .rename(&format!("project:{old}"), &format!("project:{new}"))
        {
            self.frecency.save();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rw() -> Rewrite {
        Rewrite::new("app", "web", Path::new("/d/"))
    }

    #[test]
    fn replaces_only_whole_ids() {
        assert_eq!(
            replace_bounded("/d/features/app/x", "/d/features/app", "N"),
            Some("N/x".into())
        );
        assert_eq!(
            replace_bounded("/d/features/apple/x", "/d/features/app", "N"),
            None
        );
        assert_eq!(
            replace_bounded("a /d/features/app", "/d/features/app", "N"),
            Some("a N".into())
        );
        assert_eq!(
            replace_bounded("/d/instances/app@wayland-1", "/d/instances/app", "N"),
            Some("N@wayland-1".into())
        );
    }

    #[test]
    fn rewrites_what_names_the_project() {
        let r = rw();
        let mut v = serde_json::json!({
            "project": "app",
            "title": "app: keep this",
            "slug": "app",
            "repos": [{"name": "app", "project": "app"}, {"name": "x", "project": "apple"}],
            "checks": {"app:api": true, "apple:api": true, "api": false},
            "heads": [["app:api", "abc"]],
            "command": "claude --add-dir /d/features/app/s --resume 1",
            "prompt": "/d/features/app/s/prompts/plan-1.md",
            "triage": {"service:app|p1|f": 1, "gate:app|s": 2, "gate:apple|s": 3},
            "other": "/d/features/apple/s",
        });
        assert!(r.value(&mut v, None));
        assert_eq!(
            v,
            serde_json::json!({
                "project": "web",
                "title": "app: keep this",
                "slug": "app",
                "repos": [{"name": "app", "project": "web"}, {"name": "x", "project": "apple"}],
                "checks": {"web:api": true, "apple:api": true, "api": false},
                "heads": [["web:api", "abc"]],
                "command": "claude --add-dir /d/features/web/s --resume 1",
                "prompt": "/d/features/web/s/prompts/plan-1.md",
                "triage": {"service:web|p1|f": 1, "gate:web|s": 2, "gate:apple|s": 3},
                "other": "/d/features/apple/s",
            })
        );
        // And back again.
        assert!(r.back().value(&mut v, None));
        assert_eq!(v["checks"]["app:api"], true);
        assert_eq!(v["project"], "app");
    }

    #[test]
    fn rewrites_json_lines_keeping_others() {
        let r = rw();
        let text = b"{\"project\":\"app\"}\nnot json app\n{\"project\":\"z\"}\n";
        let done = r.file(text, true).unwrap().unwrap();
        assert_eq!(
            String::from_utf8(done).unwrap(),
            "{\"project\":\"web\"}\nnot json app\n{\"project\":\"z\"}\n"
        );
        assert_eq!(r.file(b"{\"project\":\"z\"}", false).unwrap(), None);
    }

    /// A data folder with project `app`, a feature of it with its docs link,
    /// and another project linking it.
    fn data_folder(name: &str) -> PathBuf {
        let tmp = std::env::temp_dir().join(format!("gerdr-rename-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let data = tmp.join("data");
        let work = tmp.join("work/.features/f1");
        fs::create_dir_all(&work).unwrap();
        let f1 = data.join("features/app/f1");
        fs::create_dir_all(f1.join("prompts")).unwrap();
        let prompt = f1.join("prompts/plan-1.md");
        let record = serde_json::json!({
            "project": "app",
            "dir": work,
            "runs": {"plan": {"prompt": prompt, "command": format!("claude --add-dir {}", f1.display())}},
        });
        fs::write(
            f1.join("feature.json"),
            serde_json::to_vec_pretty(&record).unwrap(),
        )
        .unwrap();
        fs::write(
            f1.join("history.jsonl"),
            "{\"repo\":\"app:api\",\"what\":\"moved app:api\"}\n",
        )
        .unwrap();
        // A sandbox's files aren't read.
        fs::create_dir_all(f1.join("sandbox")).unwrap();
        fs::write(f1.join("sandbox/x.json"), "{\"project\":\"app\"}").unwrap();
        std::os::unix::fs::symlink(&f1, work.join("feature-docs")).unwrap();
        fs::write(data.join("features/app.lock"), "").unwrap();
        let other = data.join("features/other/f2");
        fs::create_dir_all(&other).unwrap();
        fs::write(
            other.join("feature.json"),
            "{\"project\":\"other\",\"links\":[{\"project\":\"app\"}],\"checks\":{\"app:api\":1}}",
        )
        .unwrap();
        fs::create_dir_all(data.join("instances/app/zed")).unwrap();
        fs::create_dir_all(data.join("instances/app@wayland-2")).unwrap();
        fs::create_dir_all(data.join("instances/apple")).unwrap();
        fs::create_dir_all(data.join("inbox")).unwrap();
        fs::write(
            data.join("inbox/app.json"),
            "{\"triage\":{\"gate:app|f1\":1}}",
        )
        .unwrap();
        fs::create_dir_all(data.join("ai-work/app/f1")).unwrap();
        fs::write(
            data.join("ai-work/app/f1/record.json"),
            "{\"project\":\"app\"}",
        )
        .unwrap();
        tmp
    }

    fn read(path: PathBuf) -> Value {
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn moves_and_rewrites_everything_and_back() {
        let tmp = data_folder("all");
        let data = tmp.join("data");
        let r = Rewrite::new("app", "web", &data);
        let before = snapshot(&tmp);
        let moved = move_data(&data, &r).unwrap();

        let f1 = data.join("features/web/f1");
        let record = read(f1.join("feature.json"));
        assert_eq!(record["project"], "web");
        let plan = &record["runs"]["plan"];
        assert_eq!(
            plan["prompt"],
            f1.join("prompts/plan-1.md").to_string_lossy().as_ref()
        );
        assert_eq!(
            plan["command"],
            format!("claude --add-dir {}", f1.display())
        );
        assert!(
            fs::read_to_string(f1.join("history.jsonl"))
                .unwrap()
                .contains("web:api")
        );
        assert!(
            fs::read_to_string(f1.join("sandbox/x.json"))
                .unwrap()
                .contains("\"app\"")
        );
        let link = PathBuf::from(record["dir"].as_str().unwrap()).join("feature-docs");
        assert_eq!(fs::read_link(&link).unwrap(), f1);
        assert!(data.join("features/web.lock").exists());
        let other = read(data.join("features/other/f2/feature.json"));
        assert_eq!(other["links"][0]["project"], "web");
        assert_eq!(other["checks"]["web:api"], 1);
        assert_eq!(other["project"], "other");
        assert!(data.join("instances/web/zed").is_dir());
        assert!(data.join("instances/web@wayland-2").is_dir());
        assert!(data.join("instances/apple").is_dir());
        assert_eq!(
            read(data.join("inbox/web.json"))["triage"]["gate:web|f1"],
            1
        );
        assert_eq!(
            read(data.join("ai-work/web/f1/record.json"))["project"],
            "web"
        );
        assert!(!data.join("features/app").exists());

        assert!(moved.undo().is_empty());
        assert_eq!(snapshot(&tmp), before);
    }

    #[test]
    fn refuses_a_taken_id_and_leaves_everything() {
        let tmp = data_folder("taken");
        let data = tmp.join("data");
        let before = snapshot(&tmp);
        let r = Rewrite::new("app", "apple", &data);
        let why = move_data(&data, &r).unwrap_err();
        assert!(why.contains("instances/apple"), "{why}");
        assert_eq!(snapshot(&tmp), before);
    }

    #[test]
    fn a_failed_step_puts_back_what_was_done() {
        let tmp = data_folder("failed");
        let data = tmp.join("data");
        let before = snapshot(&tmp);
        // The other project's record can't be written over.
        let other = data.join("features/other/f2");
        let mut perms = fs::metadata(&other).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o555);
        fs::set_permissions(&other, perms.clone()).unwrap();
        let why = move_data(&data, &Rewrite::new("app", "web", &data)).unwrap_err();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        fs::set_permissions(&other, perms).unwrap();
        assert!(why.contains("couldn't write"), "{why}");
        assert_eq!(snapshot(&tmp), before);
    }

    /// Every file and link under `dir`, with what it holds or points to.
    fn snapshot(dir: &Path) -> Vec<(PathBuf, String)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                let t = e.file_type().unwrap();
                if t.is_symlink() {
                    out.push((
                        p.clone(),
                        format!("-> {}", fs::read_link(&p).unwrap().display()),
                    ));
                } else if t.is_dir() {
                    out.push((p.clone(), "dir".into()));
                    stack.push(p);
                } else {
                    out.push((p.clone(), fs::read_to_string(&p).unwrap()));
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn finds_programs_on_its_folders() {
        // No program runs on a folder that isn't there.
        assert!(users(Path::new("/nonexistent-gerdr-data"), "app").is_empty());
    }
}

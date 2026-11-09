//! Features another machine's gerdr writes, as this one sees them.
//!
//! Each feature has one writer: the gerdr that made it. Another gerdr connected
//! to that machine keeps a copy of what the writer has under its own data
//! folder (`root`), apart from its own features, and never writes to it or back
//! to the writer: what would change a feature stays with the machine that writes
//! it. Its record names the writer's folders, so it is translated as it is read
//! (`rebased`) and never saved here.
//!
//! The writer's panes aren't this machine's to ask, and some may be on machines
//! it isn't connected to, so the writer says how they stand in a file of its own
//! (`Live`), which comes along with the copy.
//!
//! The code comes too, once the user opens a feature: fetched into the repository
//! here and checked out detached, so a diff reads as it does there. Only commits
//! come: changes not committed there show when they are.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    Feature,
    remote::{Paths, Side},
    store,
};
use crate::{desktop::shell_quote, herdr::AgentStatus, machine_work::qualified};

/// The file a gerdr keeps in its project's features folder saying how the panes
/// of its features stand.
pub const LIVE: &str = ".live.json";
/// How often a writer with panes to say something about says it again, for
/// another gerdr to tell it is still open.
pub const LIVE_EVERY: u64 = 15;
/// How long what a writer last said stands: after that it is taken to be
/// closed, and its panes not to be known.
const LIVE_STANDS: u64 = 45;
/// Under the data folder: what is kept of each other machine.
const MACHINES: &str = "machines";
/// What says which machine a copy is of, and how paths read there.
const PLACE: &str = "place.json";
/// Under a machine's copy: which features' code was placed here.
const CODE: &str = "code";

/// How the panes of a gerdr's features stand.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Live {
    /// Its clock, in Unix seconds.
    pub at: u64,
    /// The herdr session of its own panes.
    pub session: String,
    /// What it calls the machines its records name, by id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub names: BTreeMap<String, String>,
    /// By the pane's id in its records.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub panes: BTreeMap<String, LivePane>,
}

/// How a pane stands, by the gerdr that started it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LivePane {
    /// herdr's terminal in it, to show it by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<AgentStatus>,
    pub alive: bool,
    /// How many rows it has, for a look at all of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u32>,
    /// The herdr session it is in, for a tab of another machine's own gerdr:
    /// a sandbox's that went on there when its feature moved here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

/// A pane of a feature another machine writes, as last said by that machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeenPane {
    /// The machine that says.
    pub writer: String,
    /// When it said it, by its clock.
    pub at: u64,
    pub terminal: Option<String>,
    pub status: Option<AgentStatus>,
    pub alive: bool,
    /// The writer's own session, for a pane on that machine and not one of
    /// its workers: a terminal that machine's own gerdr shows.
    pub session: Option<String>,
    pub rows: Option<u32>,
}

impl SeenPane {
    /// Whether what it says still stands at `now`.
    pub fn stands(&self, now: u64) -> bool {
        now.saturating_sub(self.at) <= LIVE_STANDS
    }
}

/// What gerdr keeps of another machine under the data folder `data`.
pub fn root(data: &Path, id: &str) -> PathBuf {
    data.join(MACHINES).join(id)
}

/// The machines gerdr keeps something of, under the data folder `data`.
pub fn kept_ids(data: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(data.join(MACHINES)) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().join(PLACE).is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    ids.sort();
    ids
}

/// How paths read on a machine, kept beside its copy: its records can be read
/// with the machine away, and it is called the same.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    pub name: String,
    pub home: String,
    pub data: String,
}

impl Place {
    pub fn side(&self) -> Side {
        Side {
            home: self.home.clone(),
            data: self.data.clone(),
        }
    }
}

pub fn write_place(root: &Path, place: &Place) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(place).map_err(std::io::Error::other)?;
    crate::projects::write_atomic(&root.join(PLACE), &bytes)
}

pub fn read_place(root: &Path) -> Option<Place> {
    let raw = std::fs::read(root.join(PLACE)).ok()?;
    serde_json::from_slice(&raw).ok()
}

/// The folder paths here read as, for the copy of machine `id`: its data
/// folder's are in the copy, its home's in the home here.
pub fn side_here(home: &str, root: &Path) -> Side {
    Side {
        home: home.to_owned(),
        data: root.to_string_lossy().into_owned(),
    }
}

/// The ids of the panes `feature`'s records name.
pub fn panes_of(feature: &Feature) -> Vec<&str> {
    let runs = feature.runs.values().filter_map(|r| r.pane.as_deref());
    let checks = (feature.verify.iter())
        .flat_map(|v| &v.checks)
        .filter_map(|c| c.pane.as_deref());
    let setups = (feature.setup.iter())
        .chain(&feature.setups_elsewhere)
        .filter_map(|s| s.pane.as_deref());
    let sandbox = (feature.sandbox.iter())
        .flat_map(|s| &s.panes)
        .map(String::as_str);
    runs.chain(checks).chain(setups).chain(sandbox).collect()
}

/// `feature` as read on this machine, written by machine `id`: its paths are
/// this machine's, and what ran on its writer names that machine as what ran on
/// a worker does.
pub fn rebased(feature: &Feature, paths: &Paths, id: &str) -> Result<Feature, String> {
    let mut value = serde_json::to_value(feature).map_err(|err| err.to_string())?;
    translate(&mut value, paths);
    let mut out: Feature = serde_json::from_value(value).map_err(|err| err.to_string())?;
    let name = |pane: &mut Option<String>| {
        if let Some(bare) = pane.as_deref().filter(|p| !p.contains('/')) {
            *pane = Some(qualified(id, bare));
        }
    };
    for run in out.runs.values_mut() {
        name(&mut run.pane);
        if run.machine.is_none() && run.pane.is_some() {
            run.machine = Some(id.to_owned());
        }
    }
    if let Some(verify) = &mut out.verify {
        for check in &mut verify.checks {
            name(&mut check.pane);
        }
        verify.machine.get_or_insert_with(|| id.to_owned());
    }
    for setup in out.setup.iter_mut().chain(&mut out.setups_elsewhere) {
        name(&mut setup.pane);
        setup.machine.get_or_insert_with(|| id.to_owned());
    }
    if let Some(sandbox) = &mut out.sandbox {
        // Not known yet: never what those numbers happen to be here.
        sandbox.elsewhere = Some(Default::default());
        sandbox.machine.get_or_insert_with(|| id.to_owned());
        for pane in &mut sandbox.panes {
            let mut one = Some(std::mem::take(pane));
            name(&mut one);
            *pane = one.unwrap_or_default();
        }
    }
    out.writer = Some(id.to_owned());
    Ok(out)
}

/// Every text of a record, as it reads here: a path holder added later is
/// translated as well, and what it says of a path reads the same as the path.
pub(super) fn translate(value: &mut Value, paths: &Paths) {
    match value {
        Value::String(text) => {
            if text.contains('/') {
                *text = paths.text(text);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|v| translate(v, paths)),
        Value::Object(fields) => fields.values_mut().for_each(|v| translate(v, paths)),
        _ => {}
    }
}

/// The panes `features` name (read by `rebased`), by what `live` said of them,
/// with machine `id` the writer. A pane it doesn't list is gone, unless it
/// said nothing at all yet.
pub fn seen_panes(
    features: &[Feature],
    live: Option<&Live>,
    id: &str,
) -> BTreeMap<String, SeenPane> {
    seen_of(features.iter().flat_map(panes_of), live, id)
}

/// Panes `panes`, by what machine `id`'s `live` said of them: those of its
/// records, or tabs of its own a feature written elsewhere names.
pub fn seen_of<'a>(
    panes: impl IntoIterator<Item = &'a str>,
    live: Option<&Live>,
    id: &str,
) -> BTreeMap<String, SeenPane> {
    let mut seen = BTreeMap::new();
    let listed = live.map(|live| {
        (live.panes.iter())
            .map(|(pane, state)| {
                let own = !pane.contains('/');
                let full = match own {
                    true => qualified(id, pane),
                    false => pane.clone(),
                };
                (full, (state, own))
            })
            .collect::<BTreeMap<_, _>>()
    });
    for pane in panes {
        let found = listed.as_ref().and_then(|l| l.get(pane));
        let (terminal, status, alive, session, rows) = match found {
            Some((state, own)) => (
                state.terminal.clone(),
                state.status,
                state.alive,
                match *own {
                    true => live.map(|l| l.session.clone()),
                    false => state.session.clone(),
                },
                state.rows,
            ),
            None => (None, None, false, None, None),
        };
        seen.insert(
            pane.to_owned(),
            SeenPane {
                writer: id.to_owned(),
                at: live.map_or(0, |l| l.at),
                terminal,
                status,
                alive,
                session,
                rows,
            },
        );
    }
    seen
}

/// The script saying which folders of `dir` hold a record (`R <folder>`) and
/// which don't (`N <folder>`): those are the files of an agent working there
/// for another machine. Nothing is said of a folder that isn't there.
pub fn list_script(dir: &str) -> String {
    format!(
        r#"cd {} 2>/dev/null || exit 0
for d in */; do
  d=${{d%/}}
  [ -d "$d" ] || continue
  if [ -f "$d/{file}" ]; then echo "R $d"
  elif [ -f "$d/{handed}" ]; then echo "H $d $(cat "$d/{handed}")"
  else echo "N $d"; fi
done
"#,
        shell_quote(dir),
        file = store::FILE,
        handed = super::lease::HANDED_TO,
    )
}

/// What `list_script` said: the folders with a record, and those without.
pub fn parse_list(text: &str) -> (Vec<String>, Vec<String>) {
    let mut with = Vec::new();
    let mut without = Vec::new();
    for line in text.lines() {
        if let Some(slug) = line.strip_prefix("R ") {
            with.push(slug.to_owned());
        } else if let Some(slug) = line.strip_prefix("N ") {
            without.push(slug.to_owned());
        } else if let Some((slug, _)) = line.strip_prefix("H ").and_then(|l| l.split_once(' ')) {
            without.push(slug.to_owned());
        }
    }
    (with, without)
}

/// What `list_script` said of the features that moved: the machine each went to.
pub fn parse_moved(text: &str) -> BTreeMap<String, String> {
    (text.lines())
        .filter_map(|line| line.strip_prefix("H ")?.split_once(' '))
        .map(|(slug, machine)| (slug.to_owned(), machine.trim().to_owned()))
        .filter(|(_, machine)| !machine.is_empty())
        .collect()
}

/// A record kept of a feature, as it came, and where it is.
pub struct Kept {
    pub raw: Vec<u8>,
    /// Its folder: what is kept beside the record is read from there
    /// (`store::attach_sides`).
    pub dir: PathBuf,
    /// How the files beside it stand: they change without the record.
    pub stamp: store::Stamp,
}

/// A record kept of a feature, as it came.
pub fn read_record(root: &Path, project: &str, slug: &str) -> std::io::Result<Kept> {
    let dir = store::dir(Some(root), project, slug);
    let raw = std::fs::read(dir.join(store::FILE))?;
    let stamp = store::sides_stamp(&dir);
    Ok(Kept { raw, dir, stamp })
}

/// What the copy of a project's features has for `Live`.
pub fn read_live(root: &Path, project: &str) -> Option<Live> {
    let raw = std::fs::read(store::project_dir(Some(root), project).join(LIVE)).ok()?;
    serde_json::from_slice(&raw).ok()
}

/// What was placed here of a feature's code: all it takes to take it away.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placed {
    pub dir: PathBuf,
    pub repos: Vec<PlacedRepo>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacedRepo {
    pub main: PathBuf,
    pub worktree: PathBuf,
    /// The refs made for it.
    pub refs: Vec<String>,
}

fn placed_file(root: &Path, project: &str, slug: &str) -> PathBuf {
    root.join(CODE).join(project).join(format!("{slug}.json"))
}

pub fn note_placed(root: &Path, project: &str, slug: &str, placed: &Placed) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(placed).map_err(std::io::Error::other)?;
    crate::projects::write_atomic(&placed_file(root, project, slug), &bytes)
}

/// What was placed from machine `root` of feature `slug` is no look any more.
pub fn forget_placed(root: &Path, project: &str, slug: &str) {
    if let Err(err) = std::fs::remove_file(placed_file(root, project, slug))
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("forgetting what came of {project}/{slug}: {err}");
    }
}

/// Feature `slug` moved from machine `from` to machine `to` (both as `root` gives
/// them): what is here of its code is looked at from that one now.
pub fn move_placed(from: &Path, to: &Path, project: &str, slug: &str) {
    let (old, new) = (placed_file(from, project, slug), placed_file(to, project, slug));
    if !old.exists() {
        return;
    }
    let moved = new
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::rename(&old, &new));
    if let Err(err) = moved {
        tracing::warn!("following {project}/{slug} to another machine: {err}");
    }
}

/// The features of `project` whose code was placed here from machine `root`.
pub fn placed_slugs(root: &Path, project: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root.join(CODE).join(project)) else {
        return Vec::new();
    };
    (entries.filter_map(Result::ok))
        .filter_map(|e| Some(e.file_name().to_str()?.strip_suffix(".json")?.to_owned()))
        .collect()
}

/// The script taking away what was placed of a feature: changes in a worktree
/// keep it, as nothing says they aren't wanted, and the rest goes.
pub fn unplace_script(placed: &Placed) -> String {
    let mut script = String::new();
    for repo in &placed.repos {
        let main = shell_quote(&repo.main.to_string_lossy());
        script.push_str(&format!(
            "git -C {main} worktree remove {} 2>/dev/null\n",
            shell_quote(&repo.worktree.to_string_lossy())
        ));
        for name in &repo.refs {
            script.push_str(&format!(
                "git -C {main} update-ref -d {} 2>/dev/null\n",
                shell_quote(name)
            ));
        }
    }
    let dir = shell_quote(&placed.dir.to_string_lossy());
    script.push_str(&format!("rmdir {dir} 2>/dev/null\n"));
    script.push_str(&format!("rmdir \"$(dirname {dir})\" 2>/dev/null\n"));
    script.push_str("exit 0\n");
    script
}

/// Take away what was placed from machine `root` of feature `slug`. What it
/// couldn't is said, and its note stays gone: nothing else would come back to it.
pub fn unplace(root: &Path, project: &str, slug: &str, cancel: &AtomicBool) -> Result<(), String> {
    let file = placed_file(root, project, slug);
    let raw = std::fs::read(&file).map_err(|err| err.to_string())?;
    let placed: Placed = serde_json::from_slice(&raw).map_err(|err| err.to_string())?;
    let done = super::remote::local_script(&unplace_script(&placed), cancel);
    let _ = std::fs::remove_file(&file);
    done.map(drop).map_err(|t| t.text().to_owned())
}

/// Take away all that is kept of machine `root`: the code placed from it, then
/// its copy. What is under `keep` stays, though it no longer counts as a machine
/// kept (`kept_ids`): it is for that machine, if it is ever added again.
pub fn forget_machine(root: &Path, keep: &[&str], cancel: &AtomicBool) {
    if let Ok(projects) = std::fs::read_dir(root.join(CODE)) {
        for project in projects.filter_map(Result::ok) {
            let Some(project) = project.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            for slug in placed_slugs(root, &project) {
                if let Err(why) = unplace(root, &project, &slug, cancel) {
                    tracing::warn!("taking away the code of {project}/{slug}: {why}");
                }
            }
        }
    }
    let kept = |path: &Path| keep.iter().any(|name| path.file_name() == Some(name.as_ref()));
    let entries = std::fs::read_dir(root).into_iter().flatten().flatten();
    for path in entries.map(|e| e.path()).filter(|path| !kept(path)) {
        let removed = match path.is_dir() {
            true => std::fs::remove_dir_all(&path),
            false => std::fs::remove_file(&path),
        };
        if let Err(err) = removed {
            tracing::warn!("removing {}: {err}", path.display());
        }
    }
    // Only if nothing was kept.
    let _ = std::fs::remove_dir(root);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::{agents::StageRun, tests::feature};

    fn paths() -> Paths {
        Paths::new(
            &Side {
                home: "/home/ana".into(),
                data: "/home/ana/.local/share/gerdr".into(),
            },
            &Side {
                home: "/home/lg".into(),
                data: "/home/lg/.local/share/gerdr/machines/m1".into(),
            },
            &BTreeMap::new(),
            None,
        )
    }

    fn there() -> Feature {
        let mut f = feature("a", 5);
        f.root = "/home/ana/app".into();
        f.dir = "/home/ana/app/.features/a".into();
        let run = StageRun {
            pane: Some("w1:p1".into()),
            prompt: "/home/ana/.local/share/gerdr/features/p/a/prompt.md".into(),
            ..StageRun::default()
        };
        f.runs.insert(crate::feature::Stage::Plan, run);
        f
    }

    #[test]
    fn a_record_reads_with_this_machines_paths_and_names_its_writer() {
        let out = rebased(&there(), &paths(), "m1").unwrap();
        assert_eq!(out.root, Path::new("/home/lg/app"));
        assert_eq!(out.dir, Path::new("/home/lg/app/.features/a"));
        assert_eq!(out.writer.as_deref(), Some("m1"));
        let run = &out.runs[&crate::feature::Stage::Plan];
        assert_eq!(
            run.prompt,
            Path::new("/home/lg/.local/share/gerdr/machines/m1/features/p/a/prompt.md")
        );
        assert_eq!(run.pane.as_deref(), Some("m1/w1:p1"));
        assert_eq!(run.machine.as_deref(), Some("m1"));
    }

    #[test]
    fn a_sandbox_read_from_there_says_which_machine_has_it() {
        use crate::feature::sandbox_run::SandboxRun;
        let mut f = there();
        f.sandbox = Some(SandboxRun::default());
        let on_writer = rebased(&f, &paths(), "w1").unwrap();
        assert_eq!(on_writer.sandbox.unwrap().machine.as_deref(), Some("w1"));
        f.sandbox = Some(SandboxRun {
            machine: Some("w2".into()),
            panes: vec!["w2/w1:p1".into()],
            ..SandboxRun::default()
        });
        let on_worker = rebased(&f, &paths(), "w1").unwrap().sandbox.unwrap();
        assert_eq!(on_worker.machine.as_deref(), Some("w2"));
        assert_eq!(on_worker.panes, ["w2/w1:p1"]);
    }

    #[test]
    fn a_sandbox_read_from_there_is_not_looked_up_here() {
        let mut f = there();
        let me = crate::feature::sandbox_run::SandboxRun {
            panes: vec!["w3:p1".into()],
            ..Default::default()
        };
        f.sandbox = Some(me);
        let out = rebased(&f, &paths(), "m1").unwrap();
        let run = out.sandbox.as_ref().unwrap();
        assert!(run.elsewhere.is_some());
        assert_eq!(run.panes, ["m1/w3:p1"]);
        assert_eq!(panes_of(&out), ["m1/w1:p1", "m1/w3:p1"]);
    }

    #[test]
    fn a_pane_on_a_worker_keeps_the_worker_it_names() {
        let mut f = there();
        let run = f.runs.get_mut(&crate::feature::Stage::Plan).unwrap();
        run.pane = Some("w9/w1:p1".into());
        run.machine = Some("w9".into());
        let out = rebased(&f, &paths(), "m1").unwrap();
        let run = &out.runs[&crate::feature::Stage::Plan];
        assert_eq!(run.pane.as_deref(), Some("w9/w1:p1"));
        assert_eq!(run.machine.as_deref(), Some("w9"));
    }

    #[test]
    fn nothing_saved_says_who_wrote_it() {
        let out = rebased(&there(), &paths(), "m1").unwrap();
        let json = serde_json::to_string(&out).unwrap();
        assert!(!json.contains("\"writer\""), "{json}");
    }

    #[test]
    fn the_writer_says_how_its_panes_stand() {
        let mut f = there();
        let run = StageRun {
            pane: Some("w9/w2:p2".into()),
            machine: Some("w9".into()),
            ..StageRun::default()
        };
        f.runs.insert(crate::feature::Stage::Build, run);
        let out = rebased(&f, &paths(), "m1").unwrap();
        let live = Live {
            at: 100,
            session: "gerdr-p".into(),
            panes: BTreeMap::from([(
                "w1:p1".to_owned(),
                LivePane {
                    terminal: Some("t1".into()),
                    status: Some(AgentStatus::Working),
                    alive: true,
                    rows: None,
                    session: None,
                },
            )]),
            ..Live::default()
        };
        let seen = seen_panes(std::slice::from_ref(&out), Some(&live), "m1");
        let own = &seen["m1/w1:p1"];
        assert_eq!(own.status, Some(AgentStatus::Working));
        assert_eq!(own.session.as_deref(), Some("gerdr-p"));
        assert!(own.alive);
        // In the record and not in what it says: gone.
        let gone = &seen["w9/w2:p2"];
        assert!(!gone.alive);
        assert_eq!(gone.session, None);
    }

    #[test]
    fn a_writer_silent_for_long_is_not_known() {
        let seen = SeenPane {
            writer: "m1".into(),
            at: 100,
            terminal: None,
            status: None,
            alive: true,
            session: None,
            rows: None,
        };
        assert!(seen.stands(100 + LIVE_STANDS));
        assert!(!seen.stands(100 + LIVE_STANDS + 1));
        // Never heard of says nothing either.
        let never = seen_panes(&[rebased(&there(), &paths(), "m1").unwrap()], None, "m1");
        assert!(!never["m1/w1:p1"].stands(1000));
    }

    #[test]
    fn a_tab_of_another_machines_gerdr_is_shown_by_its_session() {
        // What this machine's own record names, as machine m1's gerdr tells of it.
        let there = Live {
            at: 100,
            session: "gerdr-p".into(),
            panes: BTreeMap::from([(
                "w1:p4".to_owned(),
                LivePane {
                    terminal: Some("t4".into()),
                    alive: true,
                    ..LivePane::default()
                },
            )]),
            ..Live::default()
        };
        let seen = seen_of(["m1/w1:p4", "m1/w1:p5"], Some(&there), "m1");
        assert_eq!(seen["m1/w1:p4"].terminal.as_deref(), Some("t4"));
        assert_eq!(seen["m1/w1:p4"].session.as_deref(), Some("gerdr-p"));
        assert!(!seen["m1/w1:p5"].alive);
        // The same, passed on by the gerdr that writes the feature now.
        let passed = Live {
            at: 101,
            session: "gerdr-q".into(),
            panes: BTreeMap::from([(
                "m1/w1:p4".to_owned(),
                LivePane {
                    terminal: Some("t4".into()),
                    alive: true,
                    session: Some("gerdr-p".into()),
                    ..LivePane::default()
                },
            )]),
            ..Live::default()
        };
        let seen = seen_of(["m1/w1:p4"], Some(&passed), "m2");
        assert_eq!(seen["m1/w1:p4"].session.as_deref(), Some("gerdr-p"));
        assert_eq!(seen["m1/w1:p4"].writer, "m2");
    }

    #[test]
    fn what_is_kept_of_a_machine_names_it_and_goes_with_it() {
        let t = crate::feature::tests::Temp::new();
        let data = t.0.as_path();
        assert!(kept_ids(data).is_empty());
        let there = root(data, "m1");
        let place = Place {
            name: "box".into(),
            home: "/home/ana".into(),
            data: "/home/ana/.local/share/gerdr".into(),
        };
        write_place(&there, &place).unwrap();
        // A folder saying nothing of itself isn't one.
        std::fs::create_dir_all(root(data, "m2")).unwrap();
        assert_eq!(kept_ids(data), ["m1"]);
        assert_eq!(read_place(&there), Some(place.clone()));
        forget_machine(&there, &[], &AtomicBool::new(false));
        assert!(!there.exists());
        assert!(kept_ids(data).is_empty());
        // What's asked to stay does, and the machine isn't kept for it.
        write_place(&there, &place).unwrap();
        std::fs::create_dir_all(there.join("left")).unwrap();
        std::fs::write(there.join("left/x.json"), "{}").unwrap();
        forget_machine(&there, &["left"], &AtomicBool::new(false));
        assert!(there.join("left/x.json").exists());
        assert!(kept_ids(data).is_empty());
    }

    #[test]
    fn the_folders_listed_tell_records_from_a_workers_files() {
        let t = crate::feature::tests::Temp::new();
        let dir = t.0.as_path();
        std::fs::create_dir_all(dir.join("mine")).unwrap();
        std::fs::write(dir.join("mine").join(store::FILE), "{}").unwrap();
        std::fs::create_dir_all(dir.join("theirs")).unwrap();
        std::fs::write(dir.join(LIVE), "{}").unwrap();
        let out = std::process::Command::new("sh")
            .args(["-c", &list_script(&dir.to_string_lossy())])
            .output()
            .unwrap();
        let (with, without) = parse_list(&String::from_utf8_lossy(&out.stdout));
        assert_eq!(with, ["mine"]);
        assert_eq!(without, ["theirs"]);
        // No folder is nothing, not a failure.
        let out = std::process::Command::new("sh")
            .args(["-c", &list_script("/no/such/folder")])
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(parse_list(&String::from_utf8_lossy(&out.stdout)).0.len(), 0);
    }

    #[test]
    fn only_folders_with_a_record_are_features() {
        let (with, without) = parse_list("R one\nN two\nnoise\nR three\nH four m9\n");
        assert_eq!(with, ["one", "three"]);
        assert_eq!(without, ["two", "four"]);
    }

    #[test]
    fn a_feature_gone_to_another_machine_is_told_from_one_over() {
        let t = crate::feature::tests::Temp::new();
        let dir = t.0.as_path();
        std::fs::create_dir_all(dir.join("gone")).unwrap();
        std::fs::write(dir.join("gone").join(super::super::lease::HANDED_TO), "m9").unwrap();
        std::fs::create_dir_all(dir.join("files")).unwrap();
        let out = std::process::Command::new("sh")
            .args(["-c", &list_script(&dir.to_string_lossy())])
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&out.stdout);
        assert_eq!(
            parse_moved(&said),
            BTreeMap::from([("gone".to_owned(), "m9".to_owned())])
        );
        let (with, without) = parse_list(&said);
        assert!(with.is_empty());
        assert_eq!(without.len(), 2);
    }

    #[test]
    fn what_came_of_a_feature_that_moved_is_kept_under_where_it_went() {
        let t = crate::feature::tests::Temp::new();
        let (a, b) = (t.0.join("a"), t.0.join("b"));
        note_placed(&a, "p", "s", &Placed::default()).unwrap();
        assert_eq!(placed_slugs(&a, "p"), ["s"]);
        move_placed(&a, &b, "p", "s");
        assert!(placed_slugs(&a, "p").is_empty());
        assert_eq!(placed_slugs(&b, "p"), ["s"]);
        forget_placed(&b, "p", "s");
        assert!(placed_slugs(&b, "p").is_empty());
        // Nothing to move or forget is nothing.
        move_placed(&a, &b, "p", "s");
        forget_placed(&b, "p", "s");
    }

    #[test]
    fn what_was_placed_goes_without_forcing_a_changed_worktree() {
        let script = unplace_script(&Placed {
            dir: "/r/.features/a".into(),
            repos: vec![PlacedRepo {
                main: "/r".into(),
                worktree: "/r/.features/a".into(),
                refs: vec!["refs/gerdr/view-head/a/r".into()],
            }],
        });
        assert!(script.contains("worktree remove"), "{script}");
        assert!(!script.contains("--force"), "{script}");
        assert!(script.contains("update-ref -d"), "{script}");
    }
}

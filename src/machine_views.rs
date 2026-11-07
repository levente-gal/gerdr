//! Features other machines' gerdrs write, shown here (see `feature::view`).
//!
//! A link that is up has one more thread: while a project is open here, it keeps
//! a copy of what that machine has of the same project, and tells what changed.
//! The features it tells of stay apart from this machine's own
//! (`Features::viewed`), so nothing that walks those runs for them: no agent is
//! started or judged, no check waited for, and no record saved. What is pressed
//! on one goes through `PanelOutcome::reads_only`.
//!
//! This also says how this gerdr's own panes stand, for those looking from
//! elsewhere (`publish_live`).

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use crate::{
    Gerdr,
    feature::{
        self, Feature, Status,
        lease::{self, Rival},
        remote::{self, Paths, Reach, Side, Trouble},
        sandbox_run, store,
        view::{self, Live, LivePane, Placed, PlacedRepo, SeenPane},
    },
    machine_work::{WorkEvent, split_pane},
};

/// How often a machine's copy is brought up to date.
const VIEW_EVERY: Duration = Duration::from_secs(3);

/// What this gerdr wants of a machine now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ViewAsk {
    /// The project open here; `None`: nothing.
    pub project: Option<String>,
    /// Its folders there to here, where they don't follow the home.
    pub folders: BTreeMap<String, String>,
    /// The features whose code is wanted here now.
    pub code: Vec<String>,
    /// The features being taken over: what's here of them isn't to be touched,
    /// even with their record gone there.
    pub holding: Vec<String>,
}

/// What the machine told, once it changed.
#[derive(Debug)]
pub struct ViewNews {
    pub project: String,
    pub features: Vec<Feature>,
    pub panes: BTreeMap<String, SeenPane>,
    pub names: BTreeMap<String, String>,
    /// The features that went to another machine, with its id.
    pub moved: BTreeMap<String, String>,
}

/// Where a feature's code here stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodeHere {
    /// Asked for: the first time, all its history comes.
    Coming,
    Here,
    /// Why it isn't.
    Failed(String),
}

/// What a machine's view needs to know of it.
#[derive(Clone)]
pub struct ViewPlace {
    pub id: String,
    pub reach: Reach,
    pub there: Side,
    pub home: String,
    /// What's kept of it here.
    pub root: PathBuf,
}

/// Bring machine `place`'s features of the project asked for here, until told to
/// stop. What can't be read now changes nothing: what was read last still shows.
/// Their code comes on a thread of its own: a first fetch can take long, and how
/// things stand goes on coming meanwhile.
pub fn watch_views(
    place: ViewPlace,
    ask: Arc<Mutex<ViewAsk>>,
    stop: Arc<AtomicBool>,
    tell: impl Fn(WorkEvent) + Clone + Send + 'static,
) {
    let latest: Arc<Mutex<Option<Latest>>> = Arc::default();
    let code = {
        let (place, ask, latest, stop, tell) = (
            place.clone(),
            ask.clone(),
            latest.clone(),
            stop.clone(),
            tell.clone(),
        );
        move || watch_code(place, ask, latest, stop, tell)
    };
    if let Err(err) = std::thread::Builder::new()
        .name("machine code".into())
        .spawn(code)
    {
        tracing::warn!("machine {}: bringing code here: {err}", place.id);
    }
    let mut rounds = Rounds::default();
    while !stop.load(Ordering::Relaxed) {
        let asked = ask.lock().map(|a| a.clone()).unwrap_or_default();
        if asked.project.is_some() {
            match view_round(&place, &asked, &mut rounds, &latest, &stop, &tell) {
                Ok(()) => {
                    if let Some(over) = rounds.said.take() {
                        tell(WorkEvent::ViewTroubleOver(over));
                    }
                }
                // The link says for itself that it is lost.
                Err(Trouble::Link(why)) => tracing::debug!("machine {}: {why}", place.id),
                Err(Trouble::Work(why)) => {
                    if rounds.said.as_ref() != Some(&why) {
                        rounds.said = Some(why.clone());
                        tell(WorkEvent::ViewTrouble(why));
                    }
                }
            }
        }
        rest(VIEW_EVERY, &stop);
    }
}

/// Wait `time`, or until told to stop.
fn rest(time: Duration, stop: &AtomicBool) {
    let until = Instant::now() + time;
    while Instant::now() < until && !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The features a machine's project has, as last read: the project, those read
/// and the folders that hold a record.
type Latest = (String, Vec<Feature>, Vec<String>, BTreeMap<String, String>);

/// Bring the code of the features asked for here, and take away that of those
/// over, until told to stop.
fn watch_code(
    place: ViewPlace,
    ask: Arc<Mutex<ViewAsk>>,
    latest: Arc<Mutex<Option<Latest>>>,
    stop: Arc<AtomicBool>,
    tell: impl Fn(WorkEvent),
) {
    let mut code = CodeRounds::default();
    while !stop.load(Ordering::Relaxed) {
        let asked = ask.lock().map(|a| a.clone()).unwrap_or_default();
        let read = latest.lock().ok().and_then(|l| l.clone());
        if let Some((project, features, with, moved)) = read
            && asked.project.as_deref() == Some(project.as_str())
        {
            let told = Told {
                features: &features,
                with: &with,
                moved: &moved,
            };
            code_round(&place, &asked, &project, told, &mut code, &stop, &tell);
        }
        rest(VIEW_EVERY, &stop);
    }
}

/// What's kept from one round of bringing code to the next.
#[derive(Default)]
struct CodeRounds {
    /// Where each feature's repositories stood when last brought here.
    heads: HashMap<(String, String), BTreeMap<String, (String, String)>>,
    /// Why each feature's code last didn't come.
    failed: HashMap<(String, String), String>,
}

/// What a machine last said of a project's features.
#[derive(Clone, Copy)]
struct Told<'a> {
    features: &'a [Feature],
    with: &'a [String],
    moved: &'a BTreeMap<String, String>,
}

fn code_round(
    place: &ViewPlace,
    asked: &ViewAsk,
    project: &str,
    told: Told,
    rounds: &mut CodeRounds,
    cancel: &AtomicBool,
    tell: &impl Fn(WorkEvent),
) {
    let Told {
        features,
        with,
        moved,
    } = told;
    // What's over there, or gone, goes here. A record that couldn't be read says
    // nothing of that.
    let over = |slug: &String| {
        !with.contains(slug) || (features.iter()).any(|f| f.slug == *slug && f.status != Status::Open)
    };
    for slug in view::placed_slugs(&place.root, project) {
        if asked.holding.contains(&slug) {
            continue;
        }
        // Looked at from where it went from now on, which may be this machine.
        if let Some(to) = moved.get(&slug) {
            rounds.heads.remove(&(project.to_owned(), slug.clone()));
            let machines = place.root.parent().unwrap_or(&place.root);
            view::move_placed(&place.root, &machines.join(to), project, &slug);
            continue;
        }
        if over(&slug) {
            rounds.heads.remove(&(project.to_owned(), slug.clone()));
            if let Err(why) = view::unplace(&place.root, project, &slug, cancel) {
                tracing::warn!("taking away the code of {project}/{slug}: {why}");
            }
        }
    }
    for slug in &asked.code {
        let Some(feature) = (features.iter())
            .find(|f| f.slug == *slug)
            .filter(|f| f.status == Status::Open)
        else {
            continue;
        };
        let key = (project.to_owned(), slug.clone());
        let known = rounds.heads.entry(key.clone()).or_default();
        let state = match bring_code(place, feature, known, asked, cancel) {
            Ok(false) => continue,
            Ok(true) => {
                rounds.failed.remove(&key);
                CodeHere::Here
            }
            Err(Trouble::Link(why)) => {
                tracing::debug!("machine {}: {why}", place.id);
                return;
            }
            Err(Trouble::Work(why)) => {
                if rounds.failed.get(&key) == Some(&why) {
                    continue;
                }
                rounds.failed.insert(key, why.clone());
                CodeHere::Failed(why)
            }
        };
        tell(WorkEvent::ViewCode {
            project: project.to_owned(),
            slug: slug.clone(),
            state,
        });
    }
}

/// What's kept from one round to the next.
#[derive(Default)]
struct Rounds {
    /// Each record as it came, and as it reads here.
    records: HashMap<(String, String), (Vec<u8>, Feature)>,
    /// The project last told of, and what.
    told: Option<(String, Vec<Feature>, BTreeMap<String, SeenPane>)>,
    /// The trouble last said.
    said: Option<String>,
}

fn view_round(
    place: &ViewPlace,
    asked: &ViewAsk,
    rounds: &mut Rounds,
    latest: &Mutex<Option<Latest>>,
    cancel: &AtomicBool,
    tell: &impl Fn(WorkEvent),
) -> Result<(), Trouble> {
    let Some(project) = asked.project.as_deref() else {
        return Ok(());
    };
    let there = format!("{}/features/{project}", place.there.data);
    let listed = place.reach.script(&view::list_script(&there), cancel)?;
    let (with, without) = view::parse_list(&listed);
    let moved = view::parse_moved(&listed);
    let local = store::project_dir(Some(&place.root), project);
    if with.is_empty() {
        // That machine has none now: none is kept.
        if let Err(err) = std::fs::remove_dir_all(&local)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!("removing {}: {err}", local.display());
        }
    } else {
        std::fs::create_dir_all(&local).map_err(|err| Trouble::Work(err.to_string()))?;
        place.reach.mirror(&there, &local, &without, cancel)?;
        // What's left out above stays as it was: a record that isn't there any more
        // would read as one next time.
        for slug in &without {
            let gone = local.join(slug);
            if let Err(err) = std::fs::remove_dir_all(&gone)
                && err.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!("removing {}: {err}", gone.display());
            }
        }
    }
    let here = view::side_here(&place.home, &place.root);
    let mut features = Vec::with_capacity(with.len());
    for slug in &with {
        let key = (project.to_owned(), slug.clone());
        let raw = match view::read_record(&place.root, project, slug) {
            Ok(raw) => raw,
            Err(err) => {
                tracing::debug!("reading the copy of {project}/{slug}: {err}");
                continue;
            }
        };
        if let Some((same, feature)) = rounds.records.get(&key)
            && *same == raw
        {
            features.push(feature.clone());
            continue;
        }
        match read_copy(&raw, &place.there, &here, &asked.folders, &place.id) {
            Ok(feature) => {
                rounds.records.insert(key, (raw, feature.clone()));
                features.push(feature);
            }
            Err(why) => tracing::warn!("reading the copy of {project}/{slug}: {why}"),
        }
    }
    features.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.slug.cmp(&b.slug)));
    ask_sandboxes(place, &mut features, cancel)?;
    let live = view::read_live(&place.root, project);
    let panes = view::seen_panes(&features, live.as_ref(), &place.id);
    let news = (rounds.told.as_ref())
        .is_none_or(|(p, f, s)| (p.as_str(), f, s) != (project, &features, &panes));
    if news {
        rounds.told = Some((project.to_owned(), features.clone(), panes.clone()));
        tell(WorkEvent::Viewed(Box::new(ViewNews {
            project: project.to_owned(),
            features: features.clone(),
            panes,
            names: live.map(|l| l.names).unwrap_or_default(),
            moved: moved.clone(),
        })));
    }
    if let Ok(mut mine) = latest.lock() {
        *mine = Some((project.to_owned(), features, with, moved));
    }
    Ok(())
}

/// Ask the machine how the sandboxes of `features` stand. Its own `/proc` says,
/// with its gerdr open or not: a sandbox goes on without it.
fn ask_sandboxes(
    place: &ViewPlace,
    features: &mut [Feature],
    cancel: &AtomicBool,
) -> Result<(), Trouble> {
    // One on another machine isn't this one's to say.
    let running = |f: &&Feature| {
        f.status == Status::Open
            && (f.sandbox.as_ref()).is_some_and(|run| {
                sandbox_run::to_ask_there(run) && run.machine.as_deref() == Some(place.id.as_str())
            })
    };
    if !features.iter().any(|f| running(&f)) {
        return Ok(());
    }
    let asked = (features.iter().filter(running))
        .filter_map(|f| Some((f.slug.as_str(), f.sandbox.as_ref()?)));
    let said = place.reach.script(&sandbox_run::there_script(asked), cancel)?;
    for feature in features {
        let here = Some(place.id.as_str());
        if let Some(run) = (feature.sandbox.as_mut())
            .filter(|r| sandbox_run::to_ask_there(r) && r.machine.as_deref() == here)
        {
            run.elsewhere = Some(sandbox_run::read_there(&said, &feature.slug, run));
        }
    }
    Ok(())
}

/// A record kept of machine `id`, as it reads here.
pub fn read_copy(
    raw: &[u8],
    there: &Side,
    here: &Side,
    folders: &BTreeMap<String, String>,
    id: &str,
) -> Result<Feature, String> {
    let mut feature: Feature = serde_json::from_slice(raw).map_err(|err| err.to_string())?;
    // Not saved: only what is read now is brought up to date.
    feature.migrate(feature::now());
    let paths = Paths::new(there, here, folders, Some(&feature));
    view::rebased(&feature, &paths, id)
}

/// Bring what's new of `feature`'s code here. Whether anything came.
fn bring_code(
    place: &ViewPlace,
    feature: &Feature,
    known: &mut BTreeMap<String, (String, String)>,
    asked: &ViewAsk,
    cancel: &AtomicBool,
) -> Result<bool, Trouble> {
    let here = view::side_here(&place.home, &place.root);
    let back = Paths::new(&here, &place.there, &flipped(&asked.folders), Some(feature));
    let places = remote::places(feature, &back);
    let came = place.reach.view_code(feature, &places, known, cancel)?;
    if came {
        let repos = (places.iter())
            .filter_map(|p| {
                let worktree = p.local_worktree.clone()?;
                let mut refs = vec![remote::view_head_ref(&feature.slug, &p.key)];
                refs.extend(p.branch.as_deref().map(remote::view_base_ref));
                Some(PlacedRepo {
                    main: p.local_main.clone(),
                    worktree,
                    refs,
                })
            })
            .collect();
        let note = Placed {
            dir: feature.dir.clone(),
            repos,
        };
        if let Err(err) = view::note_placed(&place.root, &feature.project, &feature.slug, &note) {
            tracing::warn!("noting what came of {}: {err}", feature.slug);
        }
    }
    Ok(came)
}

/// What's said of machine `name` when its features don't come.
pub fn view_trouble_text(name: &str, why: &str) -> String {
    format!("{name}'s features can't be shown here: {why}")
}

/// The same translation read the other way.
pub fn flipped(folders: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    (folders.iter())
        .map(|(from, to)| (to.clone(), from.clone()))
        .collect()
}

impl Gerdr {
    /// Which machine writes feature `slug` of `project`, when it is shown here
    /// and written on another.
    pub(crate) fn feature_writer(&self, project: &str, slug: &str) -> Option<&str> {
        self.feature(project, slug)?.writer.as_deref()
    }

    /// This project's features other machines write.
    pub(crate) fn viewed_of(&self, project: &str) -> &[Feature] {
        self.features.viewed.get(project).map_or(&[], Vec::as_slice)
    }

    /// Whether this machine is `id`.
    pub(crate) fn is_this_machine(id: &str) -> bool {
        own_id().as_deref() == Some(id)
    }

    /// How pane `pane` of a feature another machine writes stands, unless it is
    /// no such pane: what can't be told is taken to be there, as ever.
    pub(crate) fn seen_pane_state(
        &self,
        pane: &str,
    ) -> Option<(Option<crate::herdr::AgentStatus>, bool)> {
        let seen = self.features.seen.get(pane)?;
        let known = seen.stands(feature::now()) && self.work_of(&seen.writer).is_some();
        Some(match known {
            true => (seen.status, seen.alive),
            false => (Some(crate::herdr::AgentStatus::Unknown), true),
        })
    }

    /// Whether what pane `pane`'s writer says can't be told now.
    pub(crate) fn seen_out_of_reach(&self, pane: &str) -> Option<bool> {
        let seen = self.features.seen.get(pane)?;
        Some(!seen.stands(feature::now()) || self.work_of(&seen.writer).is_none())
    }

    /// The line showing pane `pane` of a feature another machine writes, in a
    /// terminal here, or why there is none. One in its writer's own session shows
    /// in that machine's gerdr: it is only looked at. One on a worker goes to
    /// whoever asks last, as there nothing is taken from anyone's screen.
    pub(crate) fn seen_pane_line(&self, pane: &str) -> Option<Result<String, String>> {
        let seen = self.features.seen.get(pane)?;
        let (id, _) = split_pane(pane)?;
        let writer = self.machine_name(&seen.writer);
        let Some(terminal) = seen.terminal.as_deref().filter(|_| seen.alive) else {
            return Some(Err(format!("That terminal is gone from {writer}")));
        };
        if !seen.stands(feature::now()) {
            return Some(Err(format!(
                "gerdr on {writer} isn't saying how its work stands: open it there"
            )));
        }
        if Self::is_this_machine(id) {
            return Some(Ok(crate::machine_work::attach_here(terminal)));
        }
        let name = self.machine_name(id);
        let Some(work) = self.work_of(id) else {
            return Some(Err(format!("Connect {name} to show what runs there")));
        };
        Some(Ok(match &seen.session {
            Some(session) => work.observe_line(terminal, session, seen.rows, &name),
            None => work.attach_line(terminal, &name),
        }))
    }

    /// What a machine told of its features: they replace what it told before.
    pub(crate) fn machine_viewed(&mut self, id: &str, news: ViewNews) {
        let ViewNews {
            project,
            features,
            panes,
            names,
            moved,
        } = news;
        for (machine, name) in names {
            self.machines.names.entry(machine).or_insert(name);
        }
        let mut features = self.rivals(id, &project, features);
        // One running on this very machine: its numbers mean what they say here.
        for feature in &mut features {
            if let Some(run) = feature.sandbox.as_mut().filter(|run| {
                run.machine.as_deref().is_some_and(Self::is_this_machine)
            }) {
                run.machine = None;
                run.elsewhere = None;
            }
        }
        let own: Vec<String> = (self.features.all.get(&project).into_iter().flatten())
            .map(|f| f.slug.clone())
            .collect();
        let others: Vec<String> = (self.viewed_of(&project).iter())
            .filter(|f| f.writer.as_deref() != Some(id))
            .map(|f| f.slug.clone())
            .collect();
        let (kept, clashing): (Vec<Feature>, Vec<Feature>) = (features.into_iter())
            .partition(|f| !own.contains(&f.slug) && !others.contains(&f.slug));
        for feature in clashing {
            if self.features.clashes.insert((id.to_owned(), feature.slug.clone())) {
                let name = self.machine_name(id);
                self.show_warning(format!(
                    "\u{201c}{}\u{201d} on {name} isn't shown: a feature here has its name",
                    feature.title
                ));
            }
        }
        self.set_viewed(id, &project, kept, panes, &moved);
    }

    /// The features machine `id` tells of, without those that lost to a record
    /// seen here of the same feature. A record of this machine's that lost goes,
    /// and that machine is told of one of its own.
    fn rivals(&mut self, id: &str, project: &str, features: Vec<Feature>) -> Vec<Feature> {
        let me = own_id().unwrap_or_default();
        let mut kept = Vec::with_capacity(features.len());
        for feature in features {
            let mine = (self.features.all.get(project).into_iter().flatten())
                .find(|f| f.slug == feature.slug);
            match mine.map(|mine| lease::rival(mine, &me, &feature, id)) {
                Some(Rival::IWin) => {
                    self.claim_feature(id, project, &feature.slug);
                    continue;
                }
                Some(Rival::TheyWin) => self.lost_feature(project, &feature.slug, id),
                Some(Rival::Clash) | None => {}
            }
            // Two others having it, the higher shows.
            let shown = (self.viewed_of(project).iter())
                .find(|f| f.slug == feature.slug && f.writer.as_deref() != Some(id))
                .map(|f| (f.created, f.writer.clone().unwrap_or_default(), f.clone()));
            if let Some((created, writer, other)) = shown
                && created == feature.created
                && (other.epoch != 0 || feature.epoch != 0)
            {
                if lease::rival(&other, &writer, &feature, id) == Rival::IWin {
                    continue;
                }
                if let Some(list) = self.features.viewed.get_mut(project) {
                    list.retain(|f| f.slug != feature.slug);
                }
            }
            kept.push(feature);
        }
        kept
    }

    /// Machine `id`'s features of `project` are `features` now.
    fn set_viewed(
        &mut self,
        id: &str,
        project: &str,
        mut features: Vec<Feature>,
        panes: BTreeMap<String, SeenPane>,
        moved: &BTreeMap<String, String>,
    ) {
        // While it is being taken over it shows as it was, whatever that machine
        // has of it by now, and so does one that moved to a machine connected here,
        // until that one tells of it: its workspace would go with it.
        let holds = |slug: &str| {
            let key = (project.to_owned(), slug.to_owned());
            self.features.taking.contains_key(&key)
                || moved.get(slug).is_some_and(|to| self.work_of(to).is_some())
        };
        let held: Vec<Feature> = (self.viewed_of(project).iter())
            .filter(|f| f.writer.as_deref() == Some(id) && holds(&f.slug))
            .cloned()
            .collect();
        features.retain(|f| !holds(&f.slug));
        features.extend(held);
        // Over there, or not open any more: its code here goes the same. What only
        // moved to another machine shows from there.
        let gone: Vec<(String, String)> = (self.viewed_of(project).iter())
            .filter(|f| f.writer.as_deref() == Some(id) && f.status == Status::Open)
            .filter(|f| {
                !(features.iter()).any(|new| new.slug == f.slug && new.status == Status::Open)
            })
            .map(|f| (f.slug.clone(), f.title.clone()))
            .collect();
        for (slug, title) in gone {
            match moved.get(&slug) {
                Some(to) => self.say_moved(project, &slug, &title, to),
                None => self.close_gone_workspace(id, project, &slug, &title),
            }
            (self.features.code_here).remove(&(project.to_owned(), slug));
        }
        let list = self.features.viewed.entry(project.to_owned()).or_default();
        let before: Vec<Feature> = (list.iter())
            .filter(|f| f.writer.as_deref() == Some(id))
            .cloned()
            .collect();
        let changed = before != features;
        list.retain(|f| f.writer.as_deref() != Some(id));
        list.extend(features);
        list.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.slug.cmp(&b.slug)));
        self.features.seen.retain(|_, s| s.writer != id);
        self.features.seen.extend(panes);
        if changed {
            self.seen_folders_changed();
            self.prune();
            self.sync_feature_panel();
            self.sync_feature_page();
        }
    }

    /// Feature `slug` is over on machine `id`, and its code goes from here: its
    /// workspace here goes first, unless what the user started runs in it. Views
    /// of what ran there aren't that.
    fn close_gone_workspace(&mut self, id: &str, project: &str, slug: &str, title: &str) {
        let name = self.machine_name(id);
        let running = format!(
            "\u{201c}{title}\u{201d} is over on {name}: its folder here goes, and something runs in it"
        );
        let closed = format!("\u{201c}{title}\u{201d} is over on {name}");
        self.close_feature_workspace(project, slug, running, closed);
    }

    /// Close feature `slug`'s workspace here and say `closed`, unless what the user
    /// started runs in it: then only say `running`. Views of what ran elsewhere
    /// aren't that.
    pub(crate) fn close_feature_workspace(
        &mut self,
        project: &str,
        slug: &str,
        running: String,
        closed: String,
    ) {
        let Some(w) = self.feature_workspace(project, slug) else {
            return;
        };
        let panes: Vec<_> = (self.workspaces[w].tabs.iter())
            .flat_map(|tab| &tab.panes)
            .filter(|p| {
                let herdr = p.herdr.as_ref().map(|h| h.pane_id.as_str());
                !herdr.is_some_and(|pane| self.is_machine_view(pane))
            })
            .filter_map(|p| self.running_in(p))
            .collect();
        if !panes.is_empty() {
            return self.show_warning(running);
        }
        self.close_workspace_now(w);
        self.show_warning(closed);
    }

    /// Everything reading a feature's files finds those of the ones shown here.
    pub(crate) fn seen_folders_changed(&self) {
        let data = crate::projects::data_dir();
        let own = |project: &str, slug: &str| {
            (self.features.all.get(project).into_iter().flatten()).any(|f| f.slug == slug)
        };
        let seen = (self.features.viewed.iter())
            .flat_map(|(project, all)| all.iter().map(move |f| (project, f)))
            .filter(|(project, f)| !own(project, &f.slug))
            .filter_map(|(project, f)| {
                let root = view::root(&data, f.writer.as_deref()?);
                let dir = store::dir(Some(&root), project, &f.slug);
                // Just moved away, its copy isn't here yet: its files still are.
                dir.is_dir().then(|| ((project.clone(), f.slug.clone()), dir))
            })
            .collect();
        store::set_seen(seen);
    }

    /// Read what's kept of other machines, from before: their features show with
    /// the machines away too. Only the machines saved: a copy of one taken out of
    /// the settings by hand stays where it is, shown once it is back.
    pub(crate) fn load_viewed(&mut self) {
        let data = crate::projects::data_dir();
        let Some(home) = crate::home_dir().map(|h| h.to_string_lossy().into_owned()) else {
            return;
        };
        let projects: Vec<String> = (self.projects.projects.iter())
            .map(|p| p.id.clone())
            .collect();
        for id in view::kept_ids(&data) {
            let root = view::root(&data, &id);
            let Some(place) = view::read_place(&root) else {
                continue;
            };
            if !self.machines.saved.contains_key(&place.name) {
                continue;
            }
            self.machines
                .names
                .entry(id.clone())
                .or_insert_with(|| place.name.clone());
            for project in &projects {
                let (features, panes, names) =
                    self.read_kept(&root, &place, &id, project, &home);
                if features.is_empty() {
                    continue;
                }
                let news = ViewNews {
                    project: project.clone(),
                    features,
                    panes,
                    names,
                    moved: BTreeMap::new(),
                };
                self.machine_viewed(&id, news);
            }
        }
    }

    /// What's kept of machine `id`'s features of `project`.
    fn read_kept(
        &self,
        root: &Path,
        place: &view::Place,
        id: &str,
        project: &str,
        home: &str,
    ) -> (
        Vec<Feature>,
        BTreeMap<String, SeenPane>,
        BTreeMap<String, String>,
    ) {
        let here = view::side_here(home, root);
        let folders = self.folders_from(&place.name, project);
        let dir = store::project_dir(Some(root), project);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Default::default();
        };
        let mut features: Vec<Feature> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| std::fs::read(entry.path().join(store::FILE)).ok())
            .filter_map(|raw| {
                read_copy(&raw, &place.side(), &here, &folders, id)
                    .inspect_err(|why| tracing::warn!("reading a copy of {project}: {why}"))
                    .ok()
            })
            .collect();
        features.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.slug.cmp(&b.slug)));
        let live = view::read_live(root, project);
        let panes = view::seen_panes(&features, live.as_ref(), id);
        (features, panes, live.map(|l| l.names).unwrap_or_default())
    }

    /// `project`'s folders on machine `name` to here: its file says them the
    /// other way, as work goes there.
    pub(crate) fn folders_from(&self, name: &str, project: &str) -> BTreeMap<String, String> {
        (self.projects.project(project))
            .and_then(|p| p.workflow.machine_folders.get(name))
            .map(flipped)
            .unwrap_or_default()
    }

    /// Say what's wanted of each machine, and how this gerdr's panes stand.
    pub(crate) fn views_tick(&mut self) {
        let Some(project) = self.project.clone() else {
            return;
        };
        let names: Vec<String> = self.machines.links.keys().cloned().collect();
        for name in names {
            let Some(work) = self.machines.links.get(&name).and_then(|l| l.work.as_ref()) else {
                continue;
            };
            let id = work.id.clone();
            let holding: Vec<String> = (self.features.taking.keys())
                .filter(|(p, _)| *p == project)
                .map(|(_, slug)| slug.clone())
                .collect();
            let code: Vec<String> = (self.viewed_of(&project).iter())
                .filter(|f| f.writer.as_deref() == Some(id.as_str()))
                .filter(|f| !holding.contains(&f.slug))
                .filter(|f| {
                    let shown = (self.features.panel.as_ref())
                        .is_some_and(|p| p.project == project && p.slug == f.slug);
                    shown || self.feature_workspace(&project, &f.slug).is_some()
                })
                .map(|f| f.slug.clone())
                .collect();
            for slug in &code {
                (self.features.code_here)
                    .entry((project.clone(), slug.clone()))
                    .or_insert(CodeHere::Coming);
            }
            let ask = ViewAsk {
                project: Some(project.clone()),
                folders: self.folders_from(&name, &project),
                code,
                holding,
            };
            if let Some(work) = self.machines.links.get(&name).and_then(|l| l.work.as_ref()) {
                work.ask_views(ask);
            }
        }
    }

    /// Say how the panes of this gerdr's own features stand, when that changed or
    /// it has been a while: another gerdr looking can't ask them itself, and may
    /// have no link to the machines they are on.
    pub(crate) fn publish_live(&mut self) {
        let Some(project) = self.project.clone() else {
            return;
        };
        let Some(all) = self.features.all.get(&project).filter(|all| !all.is_empty()) else {
            return;
        };
        let local = |pane: &str| {
            (self.herdr_tabs.values())
                .flat_map(|d| &d.panes)
                .find(|p| p.pane_id == pane)
        };
        let mut panes = BTreeMap::new();
        for feature in all.iter().filter(|f| f.status == Status::Open) {
            for pane in view::panes_of(feature) {
                let key = crate::agents::AgentKey::Herdr(pane.to_owned());
                let (status, alive) = self.agent_key_state(&key);
                let (terminal, rows) = match split_pane(pane) {
                    Some((id, bare)) => (
                        (self.work_of(id))
                            .and_then(|w| w.panes.as_ref())
                            .and_then(|p| p.get(bare))
                            .map(|l| l.terminal.clone()),
                        None,
                    ),
                    None => {
                        let here = local(pane);
                        (
                            here.map(|p| p.terminal_id.clone()),
                            here.and_then(|p| p.scroll.as_ref())
                                .map(|s| s.viewport_rows as u32),
                        )
                    }
                };
                panes.insert(
                    pane.to_owned(),
                    LivePane {
                        terminal,
                        status,
                        alive,
                        rows,
                    },
                );
            }
        }
        let session = (self.projects.project(&project))
            .map(|p| p.session.clone())
            .unwrap_or_default();
        let names = (self.machines.names.iter())
            .map(|(id, name)| (id.clone(), name.clone()))
            .collect();
        let mut live = Live {
            at: 0,
            session,
            names,
            panes,
        };
        let now = feature::now();
        let due = (self.features.live_last.as_ref()).is_none_or(|(last, at)| {
            *last != live
                || (!live.panes.is_empty() && now.saturating_sub(*at) >= view::LIVE_EVERY)
        });
        if !due {
            return;
        }
        self.features.live_last = Some((live.clone(), now));
        live.at = now;
        let path = store::project_dir(None, &project).join(view::LIVE);
        let written = serde_json::to_vec_pretty(&live)
            .map_err(std::io::Error::other)
            .and_then(|bytes| crate::projects::write_atomic(&path, &bytes));
        if let Err(err) = written {
            tracing::warn!("saying how the panes of {project} stand: {err}");
        }
    }

    /// The machine called `name` is taken away: every id seen under that name,
    /// as a machine set up anew gets another.
    pub(crate) fn forget_viewed_of(&mut self, name: &str) {
        let ids: Vec<String> = (self.machines.names.iter())
            .filter(|(_, called)| called.as_str() == name)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.left_there_forgotten(&id);
            self.machines.names.remove(&id);
            self.forget_viewed(&id);
        }
    }

    /// Machine `id` is taken away: what was kept of it goes, off the thread, and
    /// its features no longer show.
    fn forget_viewed(&mut self, id: &str) {
        let projects: Vec<String> = self.features.viewed.keys().cloned().collect();
        for project in projects {
            self.set_viewed(id, &project, Vec::new(), BTreeMap::new(), &BTreeMap::new());
        }
        let root = view::root(&crate::projects::data_dir(), id);
        let spawned = std::thread::Builder::new()
            .name("forget a machine".into())
            .spawn(move || view::forget_machine(&root, &[crate::machine_sandbox::LEFT], &AtomicBool::new(false)));
        if let Err(err) = spawned {
            tracing::warn!("removing what's kept of a machine: {err}");
        }
    }

    /// What the page of feature `f`, written on another machine, says of that.
    pub(crate) fn seen_note(&self, f: &Feature) -> Option<(String, bool)> {
        if let Some(note) = self.take_note(f) {
            return Some(note);
        }
        let id = f.writer.as_deref()?;
        let name = self.machine_name(id);
        if self.work_of(id).is_none() {
            return Some((
                format!("Written on {name}, which can't be reached: this is how it last was. Changing it here asks first"),
                true,
            ));
        }
        let key = (f.project.clone(), f.slug.clone());
        match self.features.code_here.get(&key) {
            Some(CodeHere::Failed(why)) => {
                return Some((format!("Written on {name}. Its code didn't come: {why}"), true));
            }
            Some(CodeHere::Coming) => {
                return Some((format!("Written on {name}. Its code is on its way"), false));
            }
            _ => {}
        }
        let stands = view::panes_of(f)
            .into_iter()
            .filter_map(|pane| self.features.seen.get(pane))
            .all(|seen| seen.stands(feature::now()));
        if !stands {
            return Some((
                format!("Written on {name}, whose gerdr isn't open: agents there keep running"),
                true,
            ));
        }
        Some((
            format!("Written on {name}: changing it here moves it here. Only its commits show"),
            false,
        ))
    }
}

/// This machine's id, if it has one: it has once another connected to it.
pub(crate) fn own_id() -> Option<String> {
    // Once it is there it stays; until then, asking again costs a failed open.
    static ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    if let Some(id) = ID.get() {
        return Some(id.clone());
    }
    let raw = std::fs::read_to_string(crate::projects::data_dir().join("machine-id")).ok()?;
    let id = raw.trim().to_owned();
    (!id.is_empty()).then(|| ID.get_or_init(|| id).clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_read_the_other_way() {
        let there = BTreeMap::from([("/here/app".to_owned(), "/there/work/app".to_owned())]);
        assert_eq!(
            flipped(&there),
            BTreeMap::from([("/there/work/app".to_owned(), "/here/app".to_owned())])
        );
    }

    #[test]
    fn a_copy_reads_with_this_machines_paths() {
        let there = Side {
            home: "/home/ana".into(),
            data: "/home/ana/.local/share/gerdr".into(),
        };
        let here = Side {
            home: "/home/lg".into(),
            data: "/home/lg/.local/share/gerdr/machines/m1".into(),
        };
        let mut f = crate::feature::tests::feature("a", 5);
        f.root = "/home/ana/app".into();
        f.dir = "/home/ana/app/.features/a".into();
        let raw = serde_json::to_vec(&f).unwrap();
        let out = read_copy(&raw, &there, &here, &BTreeMap::new(), "m1").unwrap();
        assert_eq!(out.root, Path::new("/home/lg/app"));
        assert_eq!(out.writer.as_deref(), Some("m1"));
        // A folder named in the project's file goes where it says.
        let moved = BTreeMap::from([("/home/ana/app".to_owned(), "/src/app".to_owned())]);
        let out = read_copy(&raw, &there, &here, &moved, "m1").unwrap();
        assert_eq!(out.dir, Path::new("/src/app/.features/a"));
    }
}

//! A press on a feature another machine writes moves it here (see `feature::lease`).
//!
//! The press is held (`Taking`) while a thread of its own brings the feature's
//! files and commits, gets the writer to let go and makes the record this machine's.
//! Small ignored files of its worktrees there, like `.env`, come too: a setup
//! here copies only from the main worktree here, which may never have had them.
//! Then the press goes on as if nothing happened, unless the feature changed on
//! the way: what the user pressed on isn't there to press any more.
//!
//! The other side is `lease_tick`: a gerdr open on the writer looks for a request,
//! and refuses it while something of the feature only it can follow is at work.
//! With the writer out of reach the user is asked, and whichever record lost goes
//! aside once both can be seen (`rivals`).

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use crate::{
    Gerdr,
    chrome::Dot,
    confirm::Then,
    feature::{
        self, Feature, Status,
        lease::{self, Answer, Request},
        remote::{self, Paths, Reach, Side, Trouble},
        sandbox_run::{self, SandboxRun},
        setup, store, view,
    },
    feature_checks::checks_running,
    features::{Job, PanelOutcome},
    herdr::AgentStatus,
    machine_views::{flipped, own_id},
    machine_work::{WorkEvent, split_pane},
    menu::MenuAction,
    tasks::Task,
};

/// How often the thread looks for the answer of a gerdr open on the writer.
const ANSWER_EVERY: Duration = Duration::from_millis(400);
/// How often a record that lost is told again to go, on a machine that keeps it.
const CLAIM_EVERY: Duration = Duration::from_secs(15);

/// A feature being taken over.
#[derive(Debug)]
pub struct Taking {
    /// The machine that wrote it.
    pub from: String,
    /// What was pressed: it goes on once this is over.
    pub press: Option<PanelOutcome>,
    /// What it does now, for the page and the tasks panel.
    pub step: String,
    stop: Arc<AtomicBool>,
}

/// What a take is told by its thread.
#[derive(Debug)]
pub enum TakeEvent {
    Step {
        project: String,
        slug: String,
        what: String,
    },
    Ended {
        project: String,
        slug: String,
        result: Result<Box<Taken>, TakeEnd>,
    },
}

/// A feature that is this machine's now.
#[derive(Debug)]
pub struct Taken {
    pub feature: Feature,
    /// It is as it was when the user pressed.
    pub same: bool,
    /// Ignored files of its worktrees there that didn't come, a line each.
    pub missed: Vec<String>,
}

/// Why a feature stays where it is.
#[derive(Debug, PartialEq, Eq)]
pub enum TakeEnd {
    Stopped,
    /// The writer says why not, or its code says.
    Refused(String),
    /// A gerdr is open there and doesn't say anything.
    NoAnswer,
    Failed(String),
}

impl From<Trouble> for TakeEnd {
    fn from(trouble: Trouble) -> Self {
        Self::Failed(trouble.text().to_owned())
    }
}

/// All a take's thread needs.
struct TakeJob {
    project: String,
    slug: String,
    title: String,
    me: String,
    my_name: String,
    from: String,
    from_name: String,
    reach: Option<Reach>,
    there: Side,
    here: Side,
    /// The project's folders there to here.
    folders: BTreeMap<String, String>,
    /// What's kept of that machine here.
    root: PathBuf,
    /// The record as the user saw it.
    before: Feature,
    forced: bool,
    /// Taken because its writer handed it back: it isn't to be handed back.
    handed: bool,
}

impl TakeJob {
    /// Its folder there.
    fn dir_there(&self) -> String {
        format!("{}/features/{}/{}", self.there.data, self.project, self.slug)
    }

    /// Its folder here, as this machine's: `store::dir` still finds the copy.
    fn dir_here(&self) -> PathBuf {
        store::project_dir(Some(Path::new(&self.here.data)), &self.project).join(&self.slug)
    }

    fn parse(&self, raw: &str) -> Result<Feature, TakeEnd> {
        if raw.trim().is_empty() {
            return Err(TakeEnd::Failed(format!(
                "\u{201c}{}\u{201d} isn't on {} any more",
                self.title, self.from_name
            )));
        }
        let mut record: Feature =
            serde_json::from_str(raw).map_err(|err| TakeEnd::Failed(err.to_string()))?;
        record.migrate(feature::now());
        Ok(record)
    }

    /// `record` as this machine's. Over a link, a sandbox running on the writer
    /// goes on there. One kept here when the feature left (`lease::KEPT`) has its
    /// tabs back.
    fn taken(&self, record: &Feature) -> Result<Feature, TakeEnd> {
        let paths = Paths::new(&self.there, &self.here, &self.folders, Some(record));
        let keep = self.reach.is_some();
        let (from, me) = (self.from.as_str(), self.me.as_str());
        let mut out = lease::taken(record, &paths, from, &self.from_name, me, keep, feature::now())
            .map_err(TakeEnd::Failed)?;
        if self.handed {
            out.came_from = None;
        }
        if let Some(run) = out.sandbox.as_mut().filter(|run| run.machine.is_none())
            && let Some(kept) = lease::read_kept(&self.dir_here().join(SANDBOX))
            && kept.started == run.started
        {
            run.panes = kept.panes;
        }
        Ok(out)
    }

    /// Whether `record` is still what the user saw.
    fn same(&self, record: &Feature) -> bool {
        let home = self.here.home.as_str();
        let side = view::side_here(home, &self.root);
        let paths = Paths::new(&self.there, &side, &self.folders, Some(record));
        let Ok(mut now) = view::rebased(record, &paths, &self.from) else {
            return false;
        };
        let mut then = self.before.clone();
        for f in [&mut now, &mut then] {
            if let Some(sandbox) = &mut f.sandbox {
                sandbox.elsewhere = None;
            }
        }
        now == then
    }

    /// `feature`'s repositories, as seen from there.
    fn places(&self, feature: &Feature) -> Vec<remote::Place> {
        let back = Paths::new(&self.here, &self.there, &flipped(&self.folders), Some(feature));
        remote::places(feature, &back)
    }

    /// Put the worktrees on their branches, and make the record here. What came
    /// must be here already. Nothing looks at the old copy any more after this.
    fn settle(&self, feature: &Feature, cancel: &AtomicBool) -> Result<(), TakeEnd> {
        let at = feature::now();
        for repo in &feature.repos {
            let Some(branch) = repo.branch.as_deref().filter(|_| repo.touched) else {
                continue;
            };
            let head = remote::view_head_ref(&feature.slug, &repo.key());
            let script = lease::branch_script(&repo.main, &repo.worktree, branch, &head, at);
            remote::local_script(&script, cancel)?;
        }
        let dir = self.dir_here();
        feature::layout::settle_taken(feature, &dir);
        store::save_at(&dir, feature).map_err(|err| {
            TakeEnd::Failed(format!("Couldn't save the record here: {err}"))
        })?;
        // From when it left here last.
        for file in [lease::HANDED_TO, lease::REQUEST, lease::ANSWER, lease::LEFT, lease::TOLD] {
            if let Err(err) = std::fs::remove_file(dir.join(file))
                && err.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!("{}: {err}", self.slug);
            }
        }
        let copy = store::project_dir(Some(&self.root), &self.project).join(&self.slug);
        if let Err(err) = std::fs::remove_dir_all(&copy)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!("removing {}: {err}", copy.display());
        }
        // It is no look at another machine's any more: nothing takes it away.
        view::forget_placed(&self.root, &self.project, &self.slug);
        Ok(())
    }

    /// Take it from the writer, over the link.
    fn over_link(
        &self,
        reach: &Reach,
        cancel: &AtomicBool,
        step: &impl Fn(String),
    ) -> Result<Taken, TakeEnd> {
        let dir = self.dir_there();
        let stopped = || match cancel.load(Ordering::Relaxed) {
            true => Err(TakeEnd::Stopped),
            false => Ok(()),
        };
        let raw = reach.script(&lease::record_script(&dir, store::FILE), cancel)?;
        let record = self.parse(&raw)?;
        let first = self.taken(&record)?;
        let places = self.places(&first);
        if !self.forced {
            let said = remote::parse_status(&reach.script(&remote::status_script(&places), cancel)?);
            if let Some(dirty) = said.iter().find(|s| s.dirty) {
                return Err(TakeEnd::Refused(format!(
                    "{} has changes not committed on {}: commit them there first",
                    dirty.key, self.from_name
                )));
            }
        }
        step("bringing its files and commits".to_owned());
        let local = self.dir_here();
        reach.pull_data(&local, &dir, cancel)?;
        let mut known = BTreeMap::new();
        reach.view_code(&first, &places, &mut known, cancel)?;
        stopped()?;
        step(format!("asking {} to let go", self.from_name));
        self.let_go(reach, &record, cancel)?;
        step("bringing what came last".to_owned());
        let finished = (|| {
            let raw = reach.script(&lease::record_script(&dir, lease::HANDED), cancel)?;
            let mut record = self.parse(&raw)?;
            reach.pull_data(&local, &dir, cancel)?;
            // Saved there before it was handed: what came with it is its own.
            // `pull_data` leaves a copy here newer by its time, but what is
            // kept beside the record must be the record's.
            for side in lease::SIDES {
                let raw = reach.script(&lease::record_script(&dir, side), cancel)?;
                store::put_side(&local, side, raw.as_bytes()).map_err(|err| {
                    TakeEnd::Failed(format!("Couldn't save its {side} here: {err}"))
                })?;
            }
            store::attach_sides(&local, &mut record);
            let feature = self.taken(&record)?;
            reach.view_code(&feature, &self.places(&feature), &mut known, cancel)?;
            stopped()?;
            // Bringing it may have outlasted `RESTORE_AFTER`, and the record
            // gone back there: then both would write it. Held, it waits anew.
            let hold = format!(
                "cd {} || exit 3\n[ -e {h} ] && touch {h} && echo HELD\nexit 0\n",
                crate::desktop::shell_quote(&dir),
                h = lease::HANDED,
            );
            if reach.script(&hold, cancel)?.trim() != "HELD" {
                return Err(TakeEnd::Failed(format!(
                    "{} took \u{201c}{}\u{201d} back while it was on the way: try again",
                    self.from_name, self.title
                )));
            }
            self.settle(&feature, cancel)?;
            Ok((record, feature))
        })();
        match &finished {
            // It would be back there in a while anyway: sooner, if it can.
            Err(end) => {
                tracing::info!("{}/{}: taking it over ended: {end:?}", self.project, self.slug);
                let quiet = AtomicBool::new(false);
                if let Err(trouble) = reach.script(&lease::undo_script(&dir), &quiet) {
                    tracing::warn!("giving {} back: {}", self.slug, trouble.text());
                }
            }
            // Should this not get there, the record comes back there, at the
            // lower epoch, and goes when the two are seen together.
            Ok(_) => {
                let quiet = AtomicBool::new(false);
                if let Err(trouble) = reach.script(&lease::done_script(&dir), &quiet) {
                    tracing::warn!("clearing up on {}: {}", self.from_name, trouble.text());
                }
            }
        }
        let (record, feature) = finished?;
        step("bringing its ignored files".to_owned());
        let missed = self.bring_ignored(reach, &feature, cancel);
        Ok(Taken {
            same: self.same(&record),
            feature,
            missed,
        })
    }

    /// Copy the small ignored files of the feature's worktrees there, like
    /// `.env`, into those here that lack them. The main worktree here may never
    /// have had them, and a setup here copies only from it. What didn't come is
    /// said a line each: the feature is here already.
    fn bring_ignored(&self, reach: &Reach, feature: &Feature, cancel: &AtomicBool) -> Vec<String> {
        let mut missed = Vec::new();
        for place in self.places(feature) {
            let (Some(there), Some(local)) = (&place.worktree, &place.local_worktree) else {
                continue;
            };
            if !local.is_dir() {
                continue;
            }
            let key = &place.key;
            let script = remote::ignored_files_script(there, setup::MAX_FILE);
            let listed = match reach.script(&script, cancel) {
                Ok(out) => out,
                Err(trouble) => {
                    let why = trouble.text();
                    tracing::warn!("{key}: listing its ignored files there: {why}");
                    missed.push(format!("{key} (couldn't list them: {why})"));
                    continue;
                }
            };
            let items: Vec<String> = (listed.lines())
                .filter(|item| local.join(item).symlink_metadata().is_err())
                .map(str::to_owned)
                .collect();
            if items.is_empty() {
                continue;
            }
            if let Err(trouble) = reach.pull_ignored(there, local, &items, cancel) {
                let why = trouble.text();
                tracing::warn!("{key}: bringing {items:?}: {why}");
                missed.push(format!("{key}: {} ({why})", items.join(", ")));
                continue;
            }
            // What this branch doesn't ignore would show as a change here.
            let wt = local.to_string_lossy();
            match remote::local_script(&remote::unignored_script(&wt, &items), cancel) {
                Ok(out) => {
                    let left: Vec<&str> = out.lines().collect();
                    tracing::info!("{key}: brought {items:?}, not kept as not ignored: {left:?}");
                }
                Err(trouble) => tracing::warn!("{key}: ignored files brought: {}", trouble.text()),
            }
        }
        missed
    }

    /// Get the writer to let go, as it has to with a gerdr open there.
    fn let_go(&self, reach: &Reach, record: &Feature, cancel: &AtomicBool) -> Result<(), TakeEnd> {
        let dir = self.dir_there();
        let open = match self.forced {
            true => "FREE".to_owned(),
            false => reach.script(&lease::open_script(&self.there.data, &self.project), cancel)?,
        };
        if open.trim() != "FREE" {
            return self.ask(reach, record, cancel);
        }
        // Its sandbox goes on there with no gerdr to say so: it is noted there
        // first, for that gerdr's next start to tell of its tabs.
        if let Some(kept) = lease::kept_of(record) {
            let json = serde_json::to_string(&kept).map_err(|e| TakeEnd::Failed(e.to_string()))?;
            let script = lease::kept_script(&format!("{dir}/{SANDBOX}"), &json);
            reach.script(&script, cancel).map_err(|trouble| {
                TakeEnd::Failed(format!(
                    "Couldn't note its running sandbox on {}: {}",
                    self.from_name,
                    trouble.text()
                ))
            })?;
        }
        let said = reach.script(&lease::yield_script(&dir, &self.me), cancel)?;
        if said.trim() == "GONE" {
            return Err(TakeEnd::Failed(format!(
                "\u{201c}{}\u{201d} isn't on {} any more",
                self.title, self.from_name
            )));
        }
        Ok(())
    }

    /// Ask a gerdr open there, and wait for what it says.
    fn ask(&self, reach: &Reach, record: &Feature, cancel: &AtomicBool) -> Result<(), TakeEnd> {
        let dir = self.dir_there();
        let epoch = record.epoch + 1;
        let request = Request {
            machine: self.me.clone(),
            name: self.my_name.clone(),
            epoch,
            forced: false,
        };
        let json = serde_json::to_string(&request).map_err(|e| TakeEnd::Failed(e.to_string()))?;
        reach.script(&lease::ask_script(&dir, &json), cancel)?;
        let answer = self.wait_for_answer(reach, epoch, cancel);
        // What was asked may have been said yes to just now: the record goes back.
        if !matches!(answer, Ok(()) | Err(TakeEnd::Refused(_))) {
            let quiet = AtomicBool::new(false);
            if let Err(trouble) = reach.script(&lease::undo_script(&dir), &quiet) {
                tracing::warn!("taking back a request: {}", trouble.text());
            }
        }
        answer
    }

    /// What a gerdr open there says to the request for `epoch`.
    fn wait_for_answer(
        &self,
        reach: &Reach,
        epoch: u64,
        cancel: &AtomicBool,
    ) -> Result<(), TakeEnd> {
        let dir = self.dir_there();
        let until = Instant::now() + lease::ASK_FOR;
        while Instant::now() < until {
            if cancel.load(Ordering::Relaxed) {
                return Err(TakeEnd::Stopped);
            }
            let raw = reach.script(&lease::record_script(&dir, lease::ANSWER), cancel)?;
            if let Ok(answer) = serde_json::from_str::<Answer>(&raw)
                && answer.epoch == epoch
            {
                return match answer.refused {
                    Some(why) => Err(TakeEnd::Refused(why)),
                    None => Ok(()),
                };
            }
            std::thread::sleep(ANSWER_EVERY);
        }
        Err(TakeEnd::NoAnswer)
    }

    /// Take it from what's kept here: the writer can't be asked.
    fn taken_here(&self, cancel: &AtomicBool) -> Result<Taken, TakeEnd> {
        let copy = store::project_dir(Some(&self.root), &self.project).join(&self.slug);
        let raw = std::fs::read_to_string(copy.join(store::FILE))
            .map_err(|err| TakeEnd::Failed(format!("Couldn't read what's kept here: {err}")))?;
        let mut record = self.parse(&raw)?;
        store::attach_sides(&copy, &mut record);
        let feature = self.taken(&record)?;
        if let Some(repo) = (feature.repos.iter()).find(|r| !r.worktree.join(".git").exists()) {
            return Err(TakeEnd::Failed(format!("{}'s code never came here", repo.key())));
        }
        let local = self.dir_here();
        remote::copy_data(&copy, &local, cancel)?;
        self.settle(&feature, cancel)?;
        Ok(Taken {
            same: self.same(&record),
            feature,
            missed: Vec::new(),
        })
    }

    fn run(&self, cancel: &AtomicBool, step: &impl Fn(String)) -> Result<Taken, TakeEnd> {
        let result = match &self.reach {
            Some(reach) => self.over_link(reach, cancel, step),
            None => self.taken_here(cancel),
        };
        // A command stopped halfway says only that it failed.
        match result {
            Err(TakeEnd::Failed(_)) if cancel.load(Ordering::Relaxed) => Err(TakeEnd::Stopped),
            other => other,
        }
    }
}

impl Gerdr {
    /// The user pressed `press` on a feature another machine writes, or asked for
    /// it as such: start moving it here.
    pub(crate) fn take_feature(
        &mut self,
        project: &str,
        slug: &str,
        press: Option<PanelOutcome>,
    ) {
        let key = (project.to_owned(), slug.to_owned());
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let Some(id) = feature.writer.clone() else {
            return;
        };
        let (name, title) = (self.machine_name(&id), feature.title.clone());
        if self.features.taking.contains_key(&key) {
            return self.show_warning(format!(
                "\u{201c}{title}\u{201d} is being taken over from {name}"
            ));
        }
        if feature.status != Status::Open {
            return self.show_warning(format!(
                "\u{201c}{title}\u{201d} is over on {name}: only gerdr there can change it"
            ));
        }
        let Some(me) = own_id_made() else {
            return self.show_error("gerdr couldn't save this machine's id".to_owned());
        };
        let left = self.take_blocker(&feature, &me);
        if self.work_of(&id).is_none() {
            let root = view::root(&crate::projects::data_dir(), &id);
            if !view::placed_slugs(&root, project).iter().any(|s| s == slug) {
                // Shown as it left here: what was done there since is there only.
                let dir = store::project_dir(None, project).join(slug);
                if lease::left_here(&dir).is_some_and(|(_, to)| to == id) {
                    return self.show_warning(format!(
                        "{name} can't be reached, and \u{201c}{title}\u{201d} went on there: it can change once {name} is back"
                    ));
                }
                return self.show_warning(format!(
                    "{name} can't be reached, and the code of \u{201c}{title}\u{201d} never came here"
                ));
            }
            let says = match left {
                Some(blocker) => format!("{}. It goes on there, and nothing here follows it", blocker.why),
                None => "What was done there since it was last seen is kept aside once it is back".to_owned(),
            };
            self.features.take_asked.insert(key, press);
            return self.ask(
                format!("Take over \u{201c}{title}\u{201d}?"),
                format!("{name} can't be reached. {says}"),
                "Take over",
                Then::TakeOver(project.to_owned(), slug.to_owned()),
            );
        }
        match left {
            Some(blocker) if blocker.stale => {
                self.features.take_asked.insert(key, press);
                self.ask(
                    format!("Take over \u{201c}{title}\u{201d}?"),
                    format!("{}. Nothing here follows it", blocker.why),
                    "Take over",
                    Then::TakeOver(project.to_owned(), slug.to_owned()),
                );
            }
            Some(blocker) => self.show_warning(blocker.why),
            None => self.start_take(project, slug, press, false),
        }
    }

    /// The user said to take it over all the same.
    pub(crate) fn take_feature_anyway(&mut self, project: &str, slug: &str) {
        let key = (project.to_owned(), slug.to_owned());
        let press = self.features.take_asked.remove(&key).flatten();
        self.start_take(project, slug, press, true);
    }

    /// Start the thread taking it over.
    pub(crate) fn start_take(
        &mut self,
        project: &str,
        slug: &str,
        press: Option<PanelOutcome>,
        forced: bool,
    ) {
        let key = (project.to_owned(), slug.to_owned());
        let Some(before) = self.feature(project, slug).cloned() else {
            return;
        };
        let Some(id) = before.writer.clone() else {
            return;
        };
        let title = before.title.clone();
        let from_name = self.machine_name(&id);
        let data = crate::projects::data_dir();
        let root = view::root(&data, &id);
        let (Some(me), Some(home), Some(place)) = (
            own_id_made(),
            crate::home_dir().map(|h| h.to_string_lossy().into_owned()),
            view::read_place(&root),
        ) else {
            return self.show_error(format!(
                "\u{201c}{title}\u{201d} can't be taken over: gerdr doesn't know how {from_name} is laid out"
            ));
        };
        let Some(tx) = self.work_sender() else {
            return self.show_error(format!("\u{201c}{title}\u{201d} can't be taken over"));
        };
        let stop = Arc::new(AtomicBool::new(false));
        let job = TakeJob {
            project: project.to_owned(),
            slug: slug.to_owned(),
            title: title.clone(),
            me,
            my_name: this_machines_name(),
            from: id.clone(),
            from_name,
            reach: self.work_of(&id).map(|work| work.reach()),
            there: place.side(),
            here: view::side_here(&home, &data),
            folders: self.folders_from(&place.name, project),
            root,
            before,
            forced,
            handed: self.features.taking_back.remove(&key),
        };
        self.features.taking.insert(
            key.clone(),
            Taking {
                from: id.clone(),
                press,
                step: "starting".to_owned(),
                stop: stop.clone(),
            },
        );
        // Before the thread: what's asked of that machine leaves this one's code alone.
        self.views_tick();
        let thread = {
            let (id, project, slug) = (id.clone(), project.to_owned(), slug.to_owned());
            let tx = tx.clone();
            move || {
                let say = |event| {
                    if let Err(err) = tx.send((id.clone(), WorkEvent::Take(event))) {
                        tracing::warn!("taking over {slug}: {err}");
                    }
                };
                let step = |what: String| {
                    say(TakeEvent::Step {
                        project: project.clone(),
                        slug: slug.clone(),
                        what,
                    });
                };
                let result = job.run(&stop, &step).map(Box::new);
                say(TakeEvent::Ended {
                    project: project.clone(),
                    slug: slug.clone(),
                    result,
                });
            }
        };
        if let Err(err) = std::thread::Builder::new().name("take over".into()).spawn(thread) {
            tracing::warn!("taking over {slug}: {err}");
            self.features.taking.remove(&key);
            self.show_error(format!("\u{201c}{title}\u{201d} can't be taken over: {err}"));
        }
        self.sync_feature_page();
    }

    /// What a take's thread says.
    pub(crate) fn take_event(&mut self, event: TakeEvent) {
        match event {
            TakeEvent::Step { project, slug, what } => {
                if let Some(taking) = self.features.taking.get_mut(&(project, slug)) {
                    taking.step = what;
                }
                self.sync_feature_page();
            }
            TakeEvent::Ended { project, slug, result } => {
                self.take_ended(&project, &slug, result);
            }
        }
    }

    fn take_ended(
        &mut self,
        project: &str,
        slug: &str,
        result: Result<Box<Taken>, TakeEnd>,
    ) {
        let key = (project.to_owned(), slug.to_owned());
        let Some(taking) = self.features.taking.remove(&key) else {
            return;
        };
        let name = self.machine_name(&taking.from);
        let title = (self.feature(project, slug))
            .map_or_else(|| slug.to_owned(), |f| f.title.clone());
        let taken = match result {
            Ok(taken) => taken,
            Err(end) => {
                self.sync_feature_page();
                return match end {
                    TakeEnd::Stopped => {
                        self.show_notice(format!("\u{201c}{title}\u{201d} stays on {name}"));
                    }
                    TakeEnd::Refused(why) => self.show_warning(why),
                    TakeEnd::NoAnswer => {
                        self.features.take_asked.insert(key, taking.press);
                        self.ask(
                            format!("Take over \u{201c}{title}\u{201d}?"),
                            format!(
                                "gerdr on {name} doesn't answer. What it does from now on is kept aside"
                            ),
                            "Take over",
                            Then::TakeOver(project.to_owned(), slug.to_owned()),
                        );
                    }
                    TakeEnd::Failed(why) => {
                        tracing::warn!("{project}/{slug}: taking it over: {why}");
                        self.show_error(format!(
                            "\u{201c}{title}\u{201d} stays on {name}: {why}"
                        ));
                    }
                };
            }
        };
        let Taken {
            feature,
            same,
            missed,
        } = *taken;
        // What it was seen as goes, what it names is asked of those machines now.
        // The tabs of a sandbox that went on on the writer are still told of there.
        let still: Vec<String> = view::panes_of(&feature).into_iter().map(str::to_owned).collect();
        if let Some(list) = self.features.viewed.get_mut(project) {
            if let Some(old) = list.iter().find(|f| f.slug == slug) {
                let names: Vec<String> = (view::panes_of(old).into_iter())
                    .filter(|pane| !still.iter().any(|p| p == pane))
                    .map(str::to_owned)
                    .collect();
                for pane in names {
                    self.features.seen.remove(&pane);
                }
            }
            list.retain(|f| f.slug != slug);
        }
        let started_here = (feature.sandbox.as_ref())
            .filter(|run| run.machine.is_none())
            .map(|run| run.started);
        let lan_again = self.end_kept(project, slug, started_here);
        let kept_there = (feature.sandbox.as_ref())
            .is_some_and(|run| run.kept && run.machine.as_deref() == Some(taking.from.as_str()));
        self.features.code_here.remove(&key);
        self.features.clashes.remove(&(taking.from.clone(), slug.to_owned()));
        let all = self.features.all.entry(project.to_owned()).or_default();
        all.retain(|f| f.slug != slug);
        all.push(feature);
        all.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.slug.cmp(&b.slug)));
        // What it holds happened there: the AI work tells what changes here.
        self.ai_work_seed(project, slug);
        self.pull_ai_work(project, slug, &taking.from, &title);
        self.seen_folders_changed();
        self.prune();
        self.sync_feature_panel();
        self.sync_feature_page();
        self.probe_feature(project, slug);
        self.find_repo_sources();
        self.adopt_checks(project);
        if lan_again {
            self.open_sandbox_lan(project, slug);
        }
        self.show_success(match kept_there {
            true => format!(
                "\u{201c}{title}\u{201d} is written here now. Its sandbox keeps running on {name}"
            ),
            false => format!("\u{201c}{title}\u{201d} is written here now"),
        });
        if !missed.is_empty() {
            self.show_warning(format!(
                "Ignored files like .env didn't come from {name}: {}. Copy them into the worktrees here",
                missed.join("; ")
            ));
        }
        let Some(press) = taking.press else {
            return;
        };
        let on_it = (self.features.panel.as_ref())
            .is_some_and(|p| p.project == project && p.slug == slug);
        if !same {
            return self.show_warning(format!(
                "\u{201c}{title}\u{201d} changed on {name} just before: look, then press again"
            ));
        }
        if on_it {
            self.apply_feature_panel(press);
        }
    }

    /// Bring the AI work of feature `slug` (`title`) from machine `from`, its
    /// writer before, on a thread of its own: what its agents did there.
    /// Only what is newer there comes, and nothing here goes. With no link
    /// it stays there; a copy that fails is said, and stays there too.
    fn pull_ai_work(&mut self, project: &str, slug: &str, from: &str, title: &str) {
        let Some(local) = self.ai_work.dir(project, slug) else {
            return;
        };
        let Some((reach, side)) = self.archive_reach(from) else {
            return tracing::info!("{project}/{slug}: its AI work stays on {from}: no link");
        };
        let there = crate::ai_work::dir(Path::new(&side.data), project, slug);
        let there = there.to_string_lossy().into_owned();
        let (jobs, done) = (self.ai_work.sender(), self.feature_sender());
        let (name, shown) = (self.machine_name(from), title.to_owned());
        let (project, slug) = (project.to_owned(), slug.to_owned());
        let (title, name_there, from) = (title.to_owned(), name.clone(), from.to_owned());
        let history = self.feature(&project, &slug).map_or(0, |f| f.history.len());
        let stop = Arc::new(AtomicBool::new(false));
        let key = (project.clone(), slug.clone());
        let pulling = crate::ai_work::Pulling {
            title: shown.clone(),
            from: name.clone(),
            stop: stop.clone(),
        };
        let pull = move || {
            let name = name_there;
            let ended = crate::ai_work::Report::PullEnded {
                project: project.clone(),
                slug: slug.clone(),
            };
            match reach.pull_ai_work(&there, &local, &from, &stop) {
                Ok(true) => {
                    if let Some(jobs) = jobs {
                        // What came holds the earlier work: it isn't kept again.
                        let job = crate::ai_work::thread::Job::Brought {
                            project,
                            slug,
                            history,
                        };
                        // Gone only when the AI work's thread is: gerdr is quitting.
                        let _ = jobs.send(job);
                    }
                }
                Ok(false) => {}
                // Asked to stop: what came stays, the rest comes another time.
                Err(_) if stop.load(Ordering::Relaxed) => {
                    tracing::info!("{project}/{slug}: bringing its AI work from {name}: stopped");
                }
                Err(trouble) => {
                    let why = trouble.text().to_owned();
                    tracing::warn!("{project}/{slug}: bringing its AI work from {name}: {why}");
                    if let Some(done) = &done {
                        let report = crate::ai_work::Report::NotBrought {
                            title,
                            from: name,
                            why,
                        };
                        // Gone only when the loop is: there is no one left to tell.
                        let _ = done.send(crate::features::Done::AiWork(report));
                    }
                }
            }
            if let Some(done) = &done {
                let _ = done.send(crate::features::Done::AiWork(ended));
            }
        };
        match std::thread::Builder::new()
            .name("ai work pull".into())
            .spawn(pull)
        {
            Ok(_) => {
                self.ai_work.pulls.insert(key, pulling);
            }
            Err(err) => {
                tracing::warn!("bringing the AI work of {shown} from {name}: {err}");
                self.show_warning(format!(
                    "The AI work of \u{201c}{shown}\u{201d} stays on {name}: {err}"
                ));
            }
        }
    }

    /// Why `feature`, written on another machine, can't move now: something at
    /// work that nothing here could follow.
    pub(crate) fn take_blocker(&self, feature: &Feature, me: &str) -> Option<Blocker> {
        let writer = feature.writer.as_deref()?;
        let name = self.machine_name(writer);
        // One running there goes on there, followed as one on a worker is: over
        // the link, once it is up.
        if let Some(run) = (feature.sandbox.as_ref())
            .filter(|run| run.machine.as_deref() == Some(writer))
            .filter(|run| run.may_run_there())
        {
            let why = match (self.work_of(writer).is_some(), run.finished.is_some()) {
                (true, true) => None,
                (true, false) => Some(format!("Its sandbox is starting on {name}: wait for it")),
                (false, _) => Some(format!("Its sandbox runs on {name}: stop it first")),
            };
            if let Some(why) = why {
                return Some(Blocker { why, stale: false });
            }
        }
        if (feature.setup.iter()).any(|s| s.finished.is_none() && s.started != 0) {
            return Some(Blocker {
                why: format!("Its setup runs on {name}: wait for it"),
                stale: false,
            });
        }
        let at_work = |pane: &str, what: &str| {
            let seen = self.features.seen.get(pane)?;
            let works = seen.alive
                && matches!(seen.status, Some(AgentStatus::Working | AgentStatus::Blocked));
            let stale = !seen.stands(feature::now());
            let on = split_pane(pane).map(|(id, _)| id);
            if on == Some(me) && works {
                return Some(Blocker {
                    why: format!("{what} works on this machine for {name}: wait for it"),
                    stale,
                });
            }
            if seen.session.is_some() && works {
                return Some(Blocker {
                    why: format!("{what} works in gerdr's own tabs on {name}: wait for it, or stop it there"),
                    stale,
                });
            }
            let elsewhere = on.filter(|id| *id != writer && *id != me)?;
            (works && self.work_of(elsewhere).is_none()).then(|| Blocker {
                why: format!(
                    "{what} works on {}: connect it here first",
                    self.machine_name(elsewhere)
                ),
                stale: false,
            })
        };
        for (stage, run) in &feature.runs {
            let what = format!("{}'s agent", stage.label());
            if run.t3.is_some() && run.ready.is_none() {
                return Some(Blocker {
                    why: format!("{what} works in T3 Code on {name}: wait for it"),
                    stale: false,
                });
            }
            if let Some(blocker) = run.pane.as_deref().and_then(|p| at_work(p, &what)) {
                return Some(blocker);
            }
        }
        if checks_running(feature) {
            let verify = feature.verify.as_ref()?;
            let unwatched = verify.checks.iter().filter_map(|c| c.pane.as_deref()).find(|pane| {
                let on = split_pane(pane).map(|(id, _)| id);
                let seen = self.features.seen.get(*pane);
                on == Some(me) || seen.is_some_and(|s| s.session.is_some())
            });
            if unwatched.is_some() {
                return Some(Blocker {
                    why: format!("Its checks run in gerdr's own tabs on {name}: wait for them"),
                    stale: false,
                });
            }
            if let Some(id) = verify.machine.as_deref()
                && id != writer
                && self.work_of(id).is_none()
            {
                return Some(Blocker {
                    why: format!(
                        "Its checks run on {}: connect it here first",
                        self.machine_name(id)
                    ),
                    stale: false,
                });
            }
        }
        None
    }

    /// Stop taking this feature over (the tasks panel).
    pub(crate) fn stop_take(&mut self, project: &str, slug: &str) {
        if let Some(taking) = self.features.taking.get(&(project.to_owned(), slug.to_owned())) {
            taking.stop.store(true, Ordering::Relaxed);
        }
    }

    /// Features being taken over, for the tasks panel.
    pub fn take_tasks(&self) -> Vec<Task> {
        (self.features.taking.iter())
            .map(|((project, slug), taking)| {
                let name = self.machine_name(&taking.from);
                let title = (self.feature(project, slug)).map_or(slug.as_str(), |f| &f.title);
                Task {
                    title: format!("Taking over \u{201c}{title}\u{201d} from {name}"),
                    chip: "taking over".to_owned(),
                    detail: taking.step.clone(),
                    dot: Dot::Working,
                    actions: vec![(
                        "Stop",
                        MenuAction::StopTakeOver(project.clone(), slug.clone()),
                    )],
                }
            })
            .collect()
    }

    /// What the page of a feature being taken over says of that.
    pub(crate) fn take_note(&self, f: &Feature) -> Option<(String, bool)> {
        let taking = self.features.taking.get(&(f.project.clone(), f.slug.clone()))?;
        let name = self.machine_name(&taking.from);
        Some((format!("Taking it over from {name}: {}", taking.step), false))
    }

    /// Look for what other machines ask of this gerdr's features, and put back
    /// what they asked for and never finished taking.
    pub(crate) fn lease_tick(&mut self) {
        self.kept_tick();
        let Some(project) = self.project.clone() else {
            return;
        };
        let asked: Vec<(String, Request)> = (self.features.all.get(&project).into_iter().flatten())
            .filter(|f| f.status == Status::Open)
            .filter_map(|f| {
                let file = store::dir(None, &project, &f.slug).join(lease::REQUEST);
                let age = std::fs::metadata(&file).ok()?.modified().ok()?.elapsed().ok();
                let raw = std::fs::read(&file).ok()?;
                // Nobody waits for that one any more.
                if age.is_some_and(|age| age > lease::REQUEST_STANDS) {
                    if let Err(err) = std::fs::remove_file(&file) {
                        tracing::warn!("removing {}: {err}", file.display());
                    }
                    return None;
                }
                Some((f.slug.clone(), serde_json::from_slice(&raw).ok()?))
            })
            .collect();
        for (slug, request) in asked {
            self.answer_request(&project, &slug, &request);
        }
        self.answer_hand_backs(&project);
        self.answer_sandbox_asks(&project);
        self.restore_handed();
        self.notice_moved(&project);
    }

    /// What other machines asked of the sandboxes of the features written here, or
    /// did with no gerdr to ask. Taken at once: the one asking only waits to see that
    /// it was, and to read why not.
    fn answer_sandbox_asks(&mut self, project: &str) {
        let read = |file: &Path| -> Option<lease::SandboxAsk> {
            serde_json::from_slice(&std::fs::read(file).ok()?).ok()
        };
        let gone = |file: &Path| {
            if let Err(err) = std::fs::remove_file(file) {
                tracing::warn!("removing {}: {err}", file.display());
            }
        };
        let slugs: Vec<String> = (self.features.all.get(project).into_iter().flatten())
            .filter(|f| f.status == Status::Open)
            .map(|f| f.slug.clone())
            .collect();
        for slug in slugs {
            let dir = store::dir(None, project, &slug);
            let told = dir.join(lease::SANDBOX_TOLD);
            if told.exists() {
                let ask = read(&told);
                gone(&told);
                if let Some(ask) = ask {
                    self.sandbox_told(project, &slug, &ask);
                }
            }
            let file = dir.join(lease::SANDBOX_ASK);
            if !file.exists() {
                continue;
            }
            let fresh = (std::fs::metadata(&file).and_then(|m| m.modified()).ok())
                .and_then(|at| at.elapsed().ok())
                .is_none_or(|age| age <= lease::REQUEST_STANDS);
            if let Some(ask) = read(&file).filter(|_| fresh) {
                match self.sandbox_refusal(project, &slug, &ask) {
                    Some(why) => write_refusal(&dir, why),
                    None => self.answer_sandbox_ask(project, &slug, &ask),
                }
            }
            // Last: the one asking reads the answer once this is gone.
            gone(&file);
        }
    }

    /// Features of the project that left while it wasn't shown here: their records
    /// went with no gerdr to ask, and what's held of them here is out of date.
    fn notice_moved(&mut self, project: &str) {
        let moved: Vec<(Feature, String)> = (self.features.all.get(project).into_iter().flatten())
            .filter(|f| {
                let key = (project.to_owned(), f.slug.clone());
                !self.features.handing.contains_key(&key)
            })
            .filter_map(|f| {
                let dir = store::project_dir(None, project).join(&f.slug);
                if dir.join(store::FILE).exists() {
                    return None;
                }
                let to = std::fs::read_to_string(dir.join(lease::HANDED_TO)).ok()?;
                Some((f.clone(), to.trim().to_owned()))
            })
            .collect();
        for (feature, to) in moved {
            self.leave_feature(project, &feature.slug, &to, &feature, true);
            self.say_moved(project, &feature.slug, &feature.title, &to, true);
        }
    }

    /// Machine `request.machine` wants feature `slug`.
    fn answer_request(&mut self, project: &str, slug: &str, request: &Request) {
        let dir = store::project_dir(None, project).join(slug);
        let say = |answer: &Answer| {
            let written = serde_json::to_vec(answer)
                .map_err(std::io::Error::other)
                .and_then(|bytes| crate::projects::write_atomic(&dir.join(lease::ANSWER), &bytes));
            if let Err(err) = written {
                tracing::warn!("answering a request for {slug}: {err}");
            }
        };
        if let Err(err) = std::fs::remove_file(dir.join(lease::REQUEST)) {
            tracing::warn!("removing a request for {slug}: {err}");
        }
        self.machines
            .names
            .entry(request.machine.clone())
            .or_insert_with(|| request.name.clone());
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        if request.forced {
            // From before this machine took it back itself, it says nothing now.
            let me = own_id().unwrap_or_default();
            if (request.epoch, request.machine.as_str()) > (feature.epoch, me.as_str()) {
                self.lost_feature(project, slug, &request.machine);
            }
            return;
        }
        let why = (request.epoch != feature.epoch + 1)
            .then(|| "It moved in the meantime: look, then press again".to_owned())
            .or_else(|| self.keeps_feature(&feature));
        if let Some(why) = why {
            return say(&Answer {
                epoch: request.epoch,
                refused: Some(why),
            });
        }
        if let Err(err) = store::save(None, &feature) {
            tracing::warn!("saving {slug} for its new writer: {err}");
            return say(&Answer {
                epoch: request.epoch,
                refused: Some(format!("It couldn't be saved there: {err}")),
            });
        }
        let handed = dir.join(lease::HANDED);
        // It shows by this while that machine can't be reached (`show_left`).
        if let Err(err) = std::fs::copy(dir.join(store::FILE), dir.join(lease::LEFT)) {
            tracing::warn!("keeping {slug} as it leaves: {err}");
        }
        let moved = crate::projects::write_atomic(&dir.join(lease::HANDED_TO), request.machine.as_bytes())
            .and_then(|()| std::fs::rename(dir.join(store::FILE), &handed));
        if let Err(err) = moved {
            tracing::warn!("handing {slug} over: {err}");
            return say(&Answer {
                epoch: request.epoch,
                refused: Some(format!("It couldn't be handed over: {err}")),
            });
        }
        touch(&handed);
        say(&Answer {
            epoch: request.epoch,
            refused: None,
        });
        let name = self.machine_name(&request.machine);
        self.features.handing.insert(
            (project.to_owned(), slug.to_owned()),
            Handing {
                title: feature.title.clone(),
                to: name.clone(),
            },
        );
        self.leave_feature(project, slug, &request.machine, &feature, true);
        self.say_moved(project, slug, &feature.title, &request.machine, true);
    }

    /// Why this gerdr keeps `feature`: something of it at work that only it can
    /// follow, or work of its own on it.
    pub(crate) fn keeps_feature(&self, feature: &Feature) -> Option<String> {
        let (project, slug) = (feature.project.as_str(), feature.slug.as_str());
        let name = this_machines_name();
        // A sandbox goes on where it is, whoever writes the feature: here too,
        // once it is up (`lease::KEPT`).
        let starting = (feature.sandbox.as_ref()).is_some_and(|run| {
            run.machine.is_none() && !run.stopped && run.failed.is_none() && run.finished.is_none()
        });
        if starting || self.busy_with(project, slug, Job::Sandbox).is_some() {
            return Some(format!(
                "Its sandbox is being started or stopped on {name}: wait for it"
            ));
        }
        if self.busy_with(project, slug, Job::Setup).is_some() {
            return Some(format!("Its setup runs on {name}: wait for it"));
        }
        if self.busy_with(project, slug, Job::Ship).is_some() {
            return Some(format!("It is being shipped on {name}: wait for it"));
        }
        let own_pane_works = view::panes_of(feature)
            .into_iter()
            .filter(|pane| split_pane(pane).is_none())
            .any(|pane| {
                let key = crate::agents::AgentKey::Herdr(pane.to_owned());
                matches!(
                    self.agent_key_state(&key).0,
                    Some(AgentStatus::Working | AgentStatus::Blocked)
                )
            });
        if own_pane_works {
            return Some(format!(
                "An agent works in gerdr's own tabs on {name}: wait for it, or stop it there"
            ));
        }
        if checks_running(feature) && feature.verify.as_ref().is_some_and(|v| v.machine.is_none()) {
            return Some(format!(
                "Its checks run in gerdr's own tabs on {name}: wait for them"
            ));
        }
        None
    }

    /// `feature` is no longer this gerdr's to write: it goes from what's walked.
    /// Its sandbox running here goes on for machine `to` with `keep_sandbox`, as
    /// that one's record has it then (`lease::taken`).
    fn leave_feature(
        &mut self,
        project: &str,
        slug: &str,
        to: &str,
        feature: &Feature,
        keep_sandbox: bool,
    ) {
        if let Some(all) = self.features.all.get_mut(project) {
            all.retain(|f| f.slug != slug);
        }
        // With that machine connected it shows from there in a moment. Until then
        // it shows as it was, or its workspace would go with it: its files are
        // still here, and only what that machine tells replaces it. So does one
        // gone to a machine saved here and away: it tells once it is back.
        if self.work_of(to).is_some() || self.machine_saved(to) {
            let mut shown = feature.clone();
            shown.writer = Some(to.to_owned());
            let list = self.features.viewed.entry(project.to_owned()).or_default();
            list.retain(|f| f.slug != slug);
            list.push(shown);
            list.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.slug.cmp(&b.slug)));
        }
        let key = (project.to_owned(), slug.to_owned());
        // Its checks go on where they are, and another waits for them now. What
        // this one's thread still finds no longer finds a feature to change.
        self.features.build.verifying.remove(&key);
        self.features.build.stops.remove(&key);
        self.unbusy(project, slug, Job::Verify);
        // Its stopped sandbox's data stays with nobody's record if it isn't taken
        // away now, and one still running stops with it, unless it goes on.
        let goes_on = |run: &SandboxRun| {
            keep_sandbox && !run.stopped && run.failed.is_none() && run.alive()
        };
        if let Some(run) = feature.sandbox.as_ref().filter(|run| run.machine.is_none())
            && goes_on(run)
        {
            self.keep_sandbox(feature, to);
        } else if let Some(run) = feature.sandbox.clone().filter(|run| run.machine.is_none()) {
            for pane in &run.panes {
                self.close_herdr_pane(pane);
            }
            let dir = crate::feature_sandbox::sandbox_dir(project, slug);
            let spawned = std::thread::Builder::new()
                .name("sandbox of a feature gone".into())
                .spawn(move || {
                    let here = sandbox_run::Host::Here;
                    if let Err(err) = sandbox_run::stop(&run, &dir, true, &here) {
                        tracing::warn!("removing a sandbox whose feature moved: {err}");
                    }
                });
            if let Err(err) = spawned {
                tracing::warn!("removing a sandbox whose feature moved: {err}");
            }
        }
        self.seen_folders_changed();
        self.prune();
        self.sync_feature_panel();
        self.sync_feature_page();
    }

    /// Say that feature `title` went to machine `to`, from this one with
    /// `from_here`: with no link there, nothing is left to show it by, but for
    /// the record as it left here (`leave_feature`).
    pub(crate) fn say_moved(
        &mut self,
        project: &str,
        slug: &str,
        title: &str,
        to: &str,
        from_here: bool,
    ) {
        let name = self.machine_name(to);
        let kept = match self.features.kept.contains_key(&(project.to_owned(), slug.to_owned())) {
            true => ". Its sandbox keeps running here",
            false => "",
        };
        lease::mark_told(&store::project_dir(None, project).join(slug));
        if self.work_of(to).is_some() {
            return self.show_notice(format!(
                "\u{201c}{title}\u{201d} is written on {name} now{kept}"
            ));
        }
        if from_here && self.machine_saved(to) {
            return self.show_notice(format!(
                "\u{201c}{title}\u{201d} is written on {name} now: until {name} is connected, it shows as it left here{kept}"
            ));
        }
        let running = format!(
            "\u{201c}{title}\u{201d} is written on {name} now: connect it here to see it. Its folder here stays{kept}"
        );
        let closed = format!(
            "\u{201c}{title}\u{201d} is written on {name} now: connect it here to see it{kept}"
        );
        self.close_feature_workspace(project, slug, running, closed);
    }

    /// The features of `project` that left this machine, and that no machine
    /// told of since, shown as they left (`lease::LEFT`), written by the machine
    /// they went to, if it is saved here: that one replaces them once it tells.
    /// A move that happened while gerdr was closed is told once.
    pub(crate) fn show_left(&mut self, project: &str) {
        let Ok(entries) = std::fs::read_dir(store::project_dir(None, project)) else {
            return;
        };
        let mut dirs: Vec<PathBuf> = (entries.filter_map(Result::ok))
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        dirs.sort();
        let mut changed = false;
        for dir in dirs {
            let Some(slug) = dir.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
                continue;
            };
            let Some((raw, to)) = lease::left_here(&dir) else {
                continue;
            };
            if Self::is_this_machine(&to) {
                continue;
            }
            // Its copy from there came before: that is newer.
            if let Some(shown) = self.feature(project, &slug) {
                if shown.writer.as_deref() == Some(to.as_str()) {
                    lease::forget_left(&dir);
                }
                continue;
            }
            let mut feature: Feature = match serde_json::from_slice(&raw) {
                Ok(feature) => feature,
                Err(err) => {
                    tracing::warn!("reading {}: {err}", dir.join(lease::LEFT).display());
                    continue;
                }
            };
            store::attach_sides(&dir, &mut feature);
            feature.migrate(feature::now());
            let title = feature.title.clone();
            if self.machine_saved(&to) {
                feature.writer = Some(to.clone());
                let list = self.features.viewed.entry(project.to_owned()).or_default();
                list.push(feature);
                list.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.slug.cmp(&b.slug)));
                changed = true;
            }
            if !lease::told(&dir) {
                self.say_moved(project, &slug, &title, &to, true);
            }
        }
        if changed {
            self.seen_folders_changed();
            self.prune();
            self.sync_feature_panel();
            self.sync_feature_page();
        }
    }

    /// Machine `id` told of `project`: what of it was kept here as it left for
    /// that machine shows from there now, or is over there.
    pub(crate) fn forget_left_of(&self, id: &str, project: &str) {
        let Ok(entries) = std::fs::read_dir(store::project_dir(None, project)) else {
            return;
        };
        for dir in entries.filter_map(Result::ok).map(|entry| entry.path()) {
            if lease::left_here(&dir).is_some_and(|(_, to)| to == id) {
                lease::forget_left(&dir);
            }
        }
    }

    /// Put back what was handed over and not taken in time, or was given back.
    fn restore_handed(&mut self) {
        let keys: Vec<(String, String)> = self.features.handing.keys().cloned().collect();
        for key in keys {
            let (project, slug) = (&key.0, &key.1);
            let dir = store::project_dir(None, project).join(slug);
            let handed = dir.join(lease::HANDED);
            let back = dir.join(store::FILE);
            if back.exists() {
                self.record_back(project, slug, &key);
                continue;
            }
            let Ok(modified) = std::fs::metadata(&handed).and_then(|m| m.modified()) else {
                // The new writer has it all.
                self.features.handing.remove(&key);
                continue;
            };
            if modified.elapsed().is_ok_and(|age| age > lease::RESTORE_AFTER) {
                if let Err(err) = restore_record(&dir) {
                    tracing::warn!("putting {slug} back: {err}");
                    self.features.handing.remove(&key);
                    continue;
                }
                self.record_back(project, slug, &key);
            }
        }
    }

    /// The record of a feature handed over is back: it never moved.
    fn record_back(&mut self, project: &str, slug: &str, key: &(String, String)) {
        let Some(handing) = self.features.handing.remove(key) else {
            return;
        };
        let read = store::read_at(&store::project_dir(None, project).join(slug));
        let mut feature = match read {
            Ok(feature) => feature,
            Err(why) => return tracing::warn!("reading {slug} back: {why}"),
        };
        feature.migrate(feature::now());
        // Its sandbox is this gerdr's own again, tabs and all.
        let started_here = (feature.sandbox.as_ref())
            .filter(|run| run.machine.is_none())
            .map(|run| run.started);
        let lan_again = self.end_kept(project, slug, started_here);
        // Shown from there meanwhile: it isn't written there.
        if let Some(list) = self.features.viewed.get_mut(project) {
            list.retain(|f| f.slug != slug);
        }
        let all = self.features.all.entry(project.to_owned()).or_default();
        all.retain(|f| f.slug != slug);
        all.push(feature);
        all.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.slug.cmp(&b.slug)));
        self.ai_work_seed(project, slug);
        self.seen_folders_changed();
        self.sync_feature_panel();
        self.sync_feature_page();
        self.adopt_checks(project);
        if lan_again {
            self.open_sandbox_lan(project, slug);
        }
        self.show_warning(format!(
            "\u{201c}{}\u{201d} stays here: {} didn't finish taking it over",
            handing.title, handing.to
        ));
    }

    /// Another machine's record of feature `slug` won: this one's goes aside.
    ///
    /// The record goes aside (`lease::ASIDE`), and what is kept beside it, its
    /// history and review rounds (`lease::SIDES`), with it: the winner's,
    /// coming here later, is never mixed with this one's.
    pub(crate) fn lost_feature(&mut self, project: &str, slug: &str, to: &str) {
        let Some(feature) = (self.features.all.get(project).into_iter().flatten())
            .find(|f| f.slug == slug)
            .cloned()
        else {
            return;
        };
        let dir = store::project_dir(None, project).join(slug);
        let at = feature::now();
        let aside = dir.join(lease::aside_name(store::FILE, at));
        let name = self.machine_name(to);
        let moved = crate::projects::write_atomic(&dir.join(lease::HANDED_TO), to.as_bytes())
            .and_then(|()| std::fs::rename(dir.join(store::FILE), &aside));
        set_sides_aside(&dir, at);
        // Its file gone, that machine got to it first: only memory is left to clear.
        if let Err(err) = moved.or_else(|err| match err.kind() {
            std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(err),
        }) {
            tracing::warn!("setting {slug} aside: {err}");
            return self.show_error(format!(
                "\u{201c}{}\u{201d} is written on {name} now, but this machine's record couldn't be set aside: {err}",
                feature.title
            ));
        }
        crate::problems::report(
            "lease",
            format!(
                "\u{201c}{}\u{201d} was taken over by {name} while this machine couldn't be reached: what was written here is kept in {}, and its commits stay on its branch here",
                feature.title,
                aside.display()
            ),
        );
        self.features.handing.remove(&(project.to_owned(), slug.to_owned()));
        // What won may not know of its sandbox: it stops here, as ever.
        self.leave_feature(project, slug, to, &feature, false);
        let running = feature::view::panes_of(&feature).into_iter().any(|pane| {
            split_pane(pane).is_none() && {
                let key = crate::agents::AgentKey::Herdr(pane.to_owned());
                matches!(
                    self.agent_key_state(&key).0,
                    Some(AgentStatus::Working | AgentStatus::Blocked)
                )
            }
        });
        let then = match running {
            true => ": its agents here go on, and nothing follows them",
            false => "",
        };
        self.show_warning(format!(
            "\u{201c}{}\u{201d} is written on {name} now{then}",
            feature.title
        ));
    }

    /// Feature `feature`'s running sandbox goes on running here for machine `to`,
    /// which writes it now: this gerdr's tabs run its commands, and it tells of
    /// them (`publish_live`) until the sandbox is gone (`kept_tick`).
    fn keep_sandbox(&mut self, feature: &Feature, to: &str) {
        let (project, slug) = (feature.project.clone(), feature.slug.clone());
        let Some(mut kept) = lease::kept_of(feature) else {
            return;
        };
        let key = (project.clone(), slug.clone());
        // Only the writer opens one to the network: what reached it goes, and
        // comes back with the feature (`end_kept`).
        kept.lan = self.features.build.sandbox_lan.remove(&key).is_some();
        let dir = own_sandbox_dir(&project, &slug);
        if let Err(err) = lease::write_kept(&dir, &kept) {
            tracing::warn!("{project}/{slug}: noting that its sandbox goes on here: {err}");
            let name = self.machine_name(to);
            self.show_error(format!(
                "The sandbox of \u{201c}{}\u{201d} keeps running here, but gerdr couldn't note that: {err}. Its terminals won't show on {name} after a restart here",
                feature.title
            ));
        }
        if kept.lan {
            let name = self.machine_name(to);
            self.show_notice(format!(
                "The sandbox of \u{201c}{}\u{201d} is closed to the network while {name} writes it",
                feature.title
            ));
        }
        // Told of from now on, noted or not.
        self.features.kept.insert(key, kept);
    }

    /// Feature `slug`'s sandbox no longer runs here for another writer: its
    /// feature is back, and has the sandbox that started at `started_here` as its
    /// own. Tabs of any other are closed. Whether the sandbox comes back open
    /// to the network, as it was when the feature left: for the caller to open,
    /// once the feature is here.
    fn end_kept(&mut self, project: &str, slug: &str, started_here: Option<u64>) -> bool {
        let Some(kept) = self.features.kept.remove(&(project.to_owned(), slug.to_owned())) else {
            return false;
        };
        let same = started_here == Some(kept.started);
        if !same {
            for pane in &kept.panes {
                self.close_herdr_pane(pane);
            }
        }
        lease::forget_kept(&own_sandbox_dir(project, slug));
        same && kept.lan
    }

    /// Close the tabs of the sandboxes kept here once they are gone: stopped,
    /// or started again, by the gerdr that writes their features.
    fn kept_tick(&mut self) {
        let gone: Vec<(String, String)> = (self.features.kept.iter())
            .filter(|(_, kept)| !kept.holder.as_ref().is_some_and(|h| h.alive()))
            .map(|(key, _)| key.clone())
            .collect();
        for (project, slug) in gone {
            tracing::info!("{project}/{slug}: its sandbox kept here is gone");
            self.end_kept(&project, &slug, None);
        }
    }

    /// Machine `id` still has a record of feature `slug` that lost to this
    /// machine's: tell it to go, now and again, for it may not have heard.
    pub(crate) fn claim_feature(&mut self, id: &str, project: &str, slug: &str) {
        let key = (id.to_owned(), project.to_owned(), slug.to_owned());
        let due = (self.features.claims.get(&key)).is_none_or(|at| at.elapsed() >= CLAIM_EVERY);
        if !due {
            return;
        }
        let (Some(work), Some(me), Some(feature)) =
            (self.work_of(id), own_id_made(), self.feature(project, slug))
        else {
            return;
        };
        let request = Request {
            machine: me.clone(),
            name: this_machines_name(),
            epoch: feature.epoch,
            forced: true,
        };
        let reach = work.reach();
        let data = work.info.site.data.clone();
        let dir = format!("{data}/gerdr/features/{project}/{slug}");
        let lock_data = format!("{data}/gerdr");
        let project = project.to_owned();
        self.features.claims.insert(key, Instant::now());
        let spawned = std::thread::Builder::new()
            .name("claim a feature".into())
            .spawn(move || {
                let quiet = AtomicBool::new(false);
                let told = (|| {
                    let open = reach.script(&lease::open_script(&lock_data, &project), &quiet)?;
                    let script = match open.trim() {
                        "HELD" => {
                            let json = serde_json::to_string(&request)
                                .map_err(|e| Trouble::Work(e.to_string()))?;
                            lease::ask_script(&dir, &json)
                        }
                        _ => lease::aside_script(&dir, &me, feature::now()),
                    };
                    reach.script(&script, &quiet)
                })();
                if let Err(trouble) = told {
                    tracing::warn!("telling a machine its record lost: {}", trouble.text());
                }
            });
        if let Err(err) = spawned {
            tracing::warn!("telling a machine its record lost: {err}");
        }
    }
}

/// Something at work that keeps a feature from moving.
pub(crate) struct Blocker {
    pub(crate) why: String,
    /// It is what the machine last said, long ago: it may be over.
    stale: bool,
}

/// A feature whose record waits to be taken from here.
#[derive(Debug)]
pub struct Handing {
    pub title: String,
    /// What this machine calls the one taking it.
    pub to: String,
}

/// Put a handed record back in its place, where it never moved.
/// The folder of a sandbox's files in a feature's folder.
const SANDBOX: &str = "sandbox";

/// The folder of feature `slug`'s sandbox in this machine's own data, whoever
/// writes the feature: never the copy kept of another machine's.
pub(crate) fn own_sandbox_dir(project: &str, slug: &str) -> PathBuf {
    store::project_dir(None, project).join(slug).join(SANDBOX)
}

/// The sandboxes of `project` kept running here for features another machine
/// writes now (`lease::KEPT`).
pub(crate) fn load_kept(project: &str) -> Vec<lease::Kept> {
    let dir = store::project_dir(None, project);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(err) => {
            tracing::warn!("reading {}: {err}", dir.display());
            return Vec::new();
        }
    };
    (entries.filter_map(Result::ok))
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        // Back here, it is this gerdr's own: `end_kept` forgets it.
        .filter(|path| !path.join(store::FILE).exists())
        .filter_map(|path| lease::read_kept(&path.join(SANDBOX)))
        .filter(|kept| kept.project == project)
        .collect()
}

/// Say why what was asked of a sandbox isn't done, for the one who asked.
fn write_refusal(dir: &Path, why: String) {
    let said = serde_json::to_vec(&lease::SandboxRefusal { why })
        .map_err(std::io::Error::other)
        .and_then(|text| std::fs::write(dir.join(lease::SANDBOX_ANSWER), text));
    if let Err(err) = said {
        tracing::warn!("answering what was asked of a sandbox: {err}");
    }
}

fn restore_record(dir: &Path) -> std::io::Result<()> {
    std::fs::rename(dir.join(lease::HANDED), dir.join(store::FILE))?;
    for file in [lease::HANDED_TO, lease::ANSWER, lease::LEFT, lease::TOLD] {
        match std::fs::remove_file(dir.join(file)) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err),
            _ => {}
        }
    }
    Ok(())
}

/// A record handed over that nobody took while gerdr was closed goes back before
/// the project's features are read: `HANDED` old enough, and none in its place.
pub fn restore_left_records(project: &str) {
    restore_left_in(&store::project_dir(None, project));
}

/// `restore_left_records` over the features folder `features`.
fn restore_left_in(features: &Path) {
    let Ok(entries) = std::fs::read_dir(features) else {
        return;
    };
    for dir in entries.filter_map(Result::ok).map(|e| e.path()) {
        let handed = dir.join(lease::HANDED);
        let old = (std::fs::metadata(&handed).and_then(|m| m.modified()))
            .is_ok_and(|at| at.elapsed().is_ok_and(|age| age > lease::RESTORE_AFTER));
        if old
            && !dir.join(store::FILE).exists()
            && let Err(err) = restore_record(&dir)
        {
            tracing::warn!("putting {} back: {err}", dir.display());
        }
    }
}

/// Mark file `path` as written now: the time a record waits counts from here.
fn touch(path: &Path) {
    let done = (std::fs::File::options().write(true).open(path))
        .and_then(|file| file.set_modified(SystemTime::now()));
    if let Err(err) = done {
        tracing::warn!("touching {}: {err}", path.display());
    }
}

/// What other machines hear this one called: there is no name it has for itself.
pub fn this_machines_name() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "another machine".to_owned())
}

/// This machine's id, made now if no other machine has connected to it yet: a
/// record names it from here on, and so do the AI work's files.
pub(crate) fn own_id_made() -> Option<String> {
    if let Some(id) = own_id() {
        return Some(id);
    }
    let new = format!(
        "{:x}{:x}",
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
        std::process::id()
    );
    let file = crate::projects::data_dir().join("machine-id");
    if let Err(err) = crate::projects::write_atomic(&file, format!("{new}\n").as_bytes()) {
        tracing::warn!("saving this machine's id: {err}");
        return None;
    }
    own_id()
}

/// Move what is kept beside a record that lost in folder `dir` aside with
/// it, named for `at` (`lease::aside_name`). None there is nothing to do.
fn set_sides_aside(dir: &Path, at: u64) {
    for side in lease::SIDES {
        let to = dir.join(lease::aside_name(side, at));
        match std::fs::rename(dir.join(side), &to) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                tracing::warn!("setting {} aside: {err}", to.display());
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_handed_record_nobody_took_comes_back_once_old() {
        let t = std::env::temp_dir().join(format!("gerdr-lease-restore-{}", std::process::id()));
        let at = |slug: &str, age: Duration| {
            let dir = t.join(slug);
            std::fs::create_dir_all(&dir).unwrap();
            for file in [lease::HANDED, lease::HANDED_TO, lease::LEFT] {
                std::fs::write(dir.join(file), slug).unwrap();
            }
            let handed = std::fs::File::options().write(true).open(dir.join(lease::HANDED));
            handed.unwrap().set_modified(SystemTime::now() - age).unwrap();
            dir
        };
        let old = lease::RESTORE_AFTER + Duration::from_secs(5);
        let gone = at("gone", old);
        let fresh = at("fresh", Duration::ZERO);
        // One with a record in its place keeps that record.
        let back = at("back", old);
        std::fs::write(back.join(store::FILE), "mine").unwrap();
        restore_left_in(&t);
        assert_eq!(std::fs::read_to_string(gone.join(store::FILE)).unwrap(), "gone");
        for file in [lease::HANDED, lease::HANDED_TO, lease::LEFT] {
            assert!(!gone.join(file).exists(), "{file}");
        }
        assert!(fresh.join(lease::HANDED).exists());
        assert!(!fresh.join(store::FILE).exists());
        assert_eq!(std::fs::read_to_string(back.join(store::FILE)).unwrap(), "mine");
        assert!(back.join(lease::HANDED).exists());
        std::fs::remove_dir_all(&t).unwrap();
    }

    #[test]
    fn taking_from_whats_kept_here_keeps_its_history() {
        let t = std::env::temp_dir().join(format!("gerdr-lease-take-{}", std::process::id()));
        let root = t.join("kept");
        let here = Side {
            home: t.to_string_lossy().into_owned(),
            data: t.join("data").to_string_lossy().into_owned(),
        };
        let there = Side {
            home: "/home/ana".into(),
            data: "/home/ana/.local/share/gerdr".into(),
        };
        let mut f = crate::feature::tests::feature("a", 5);
        f.root = "/home/ana/app".into();
        f.dir = "/home/ana/app/.features/a".into();
        f.log(1, "created");
        f.log(2, "Brief ticked");
        let copy = store::project_dir(Some(&root), "p").join("a");
        store::save_at(&copy, &f).unwrap();
        let raw = std::fs::read(copy.join(store::FILE)).unwrap();
        let shown = view::side_here(&here.home, &root);
        let before =
            crate::machine_views::read_copy(&raw, &copy, &there, &shown, &BTreeMap::new(), "m1")
                .unwrap();
        let job = TakeJob {
            project: "p".into(),
            slug: "a".into(),
            title: f.title.clone(),
            me: "me".into(),
            my_name: "here".into(),
            from: "m1".into(),
            from_name: "ana".into(),
            reach: None,
            there,
            here,
            folders: BTreeMap::new(),
            root,
            before,
            forced: false,
            handed: false,
        };
        // What this machine had of it before, newer by its time.
        let mut stale = f.clone();
        stale.history = Vec::new();
        stale.log(9, "only here");
        store::save_at(&job.dir_here(), &stale).unwrap();
        let taken = job.taken_here(&AtomicBool::new(false)).unwrap();
        assert!(taken.same);
        // "Taken over from ana" after what it had.
        assert_eq!(taken.feature.history[..2], f.history);
        assert_eq!(taken.feature.history.len(), 3);
        let mine = store::read_at(&job.dir_here()).unwrap();
        assert_eq!(mine.history, taken.feature.history);
        std::fs::remove_dir_all(&t).unwrap();
    }

    #[test]
    fn what_is_kept_beside_a_record_that_lost_goes_aside_with_it() {
        let t = std::env::temp_dir().join(format!("gerdr-lease-aside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        std::fs::create_dir_all(&t).unwrap();
        std::fs::write(t.join(store::HISTORY), "{}\n").unwrap();
        std::fs::write(t.join(store::ROUNDS), "[]").unwrap();
        set_sides_aside(&t, 5);
        for side in lease::SIDES {
            assert!(!t.join(side).exists(), "{side}");
            assert!(t.join(lease::aside_name(side, 5)).exists(), "{side}");
        }
        // Nothing left to move is nothing to say.
        set_sides_aside(&t, 6);
        std::fs::remove_dir_all(&t).unwrap();
    }
}

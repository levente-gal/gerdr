//! A press on a feature another machine writes moves it here (see `feature::lease`).
//!
//! The press is held (`Taking`) while a thread of its own brings the feature's
//! files and commits, gets the writer to let go and makes the record this machine's.
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
        sandbox_run, store, view,
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

    /// `record` as this machine's.
    fn taken(&self, record: &Feature) -> Result<Feature, TakeEnd> {
        let paths = Paths::new(&self.there, &self.here, &self.folders, Some(record));
        lease::taken(record, &paths, &self.from, &self.from_name, &self.me, feature::now())
            .map_err(TakeEnd::Failed)
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
        for file in [lease::HANDED_TO, lease::REQUEST, lease::ANSWER] {
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
            let record = self.parse(&raw)?;
            let feature = self.taken(&record)?;
            reach.pull_data(&local, &dir, cancel)?;
            reach.view_code(&feature, &self.places(&feature), &mut known, cancel)?;
            stopped()?;
            self.settle(&feature, cancel)?;
            Ok(Taken {
                same: self.same(&record),
                feature,
            })
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
        finished
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
        let record = self.parse(&raw)?;
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
    fn start_take(
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
        let Taken { feature, same } = *taken;
        // What it was seen as goes, what it names is asked of those machines now.
        if let Some(list) = self.features.viewed.get_mut(project) {
            if let Some(old) = list.iter().find(|f| f.slug == slug) {
                let names: Vec<String> =
                    view::panes_of(old).into_iter().map(str::to_owned).collect();
                for pane in names {
                    self.features.seen.remove(&pane);
                }
            }
            list.retain(|f| f.slug != slug);
        }
        self.features.code_here.remove(&key);
        self.features.clashes.remove(&(taking.from.clone(), slug.to_owned()));
        let all = self.features.all.entry(project.to_owned()).or_default();
        all.retain(|f| f.slug != slug);
        all.push(feature);
        all.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.slug.cmp(&b.slug)));
        self.seen_folders_changed();
        self.prune();
        self.sync_feature_panel();
        self.sync_feature_page();
        self.probe_feature(project, slug);
        self.find_repo_sources();
        self.adopt_checks(project);
        self.show_success(format!("\u{201c}{title}\u{201d} is written here now"));
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

    /// Why `feature`, written on another machine, can't move now: something at
    /// work that nothing here could follow.
    fn take_blocker(&self, feature: &Feature, me: &str) -> Option<Blocker> {
        let writer = feature.writer.as_deref()?;
        let name = self.machine_name(writer);
        if feature.sandbox.as_ref().is_some_and(|run| run.may_run_there()) {
            return Some(Blocker {
                why: format!("Its sandbox runs on {name}: stop it there first"),
                stale: false,
            });
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
        self.restore_handed();
        self.notice_moved(&project);
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
            self.leave_feature(project, &feature.slug, &to, &feature);
            self.say_moved(project, &feature.slug, &feature.title, &to);
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
        self.leave_feature(project, slug, &request.machine, &feature);
        self.say_moved(project, slug, &feature.title, &request.machine);
    }

    /// Why this gerdr keeps `feature`: something of it at work that only it can
    /// follow, or work of its own on it.
    fn keeps_feature(&self, feature: &Feature) -> Option<String> {
        let (project, slug) = (feature.project.as_str(), feature.slug.as_str());
        let name = this_machines_name();
        if feature.sandbox.as_ref().is_some_and(|run| !run.stopped && run.alive())
            || self.busy_with(project, slug, Job::Sandbox).is_some()
        {
            return Some(format!("Its sandbox runs on {name}: stop it there first"));
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
    fn leave_feature(&mut self, project: &str, slug: &str, to: &str, feature: &Feature) {
        if let Some(all) = self.features.all.get_mut(project) {
            all.retain(|f| f.slug != slug);
        }
        // With that machine connected it shows from there in a moment. Until then
        // it shows as it was, or its workspace would go with it: its files are
        // still here, and only what that machine tells replaces it.
        if self.work_of(to).is_some() {
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
        // away now, and one still running stops with it.
        if let Some(run) = feature.sandbox.clone() {
            for pane in &run.panes {
                self.close_herdr_pane(pane);
            }
            let dir = crate::feature_sandbox::sandbox_dir(project, slug);
            let spawned = std::thread::Builder::new()
                .name("sandbox of a feature gone".into())
                .spawn(move || {
                    if let Err(err) = sandbox_run::stop(&run, &dir, true) {
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

    /// Say that feature `title` went to machine `to`: with no link there, nothing
    /// is left to show it by.
    pub(crate) fn say_moved(&mut self, project: &str, slug: &str, title: &str, to: &str) {
        let name = self.machine_name(to);
        if self.work_of(to).is_some() {
            return self.show_notice(format!("\u{201c}{title}\u{201d} is written on {name} now"));
        }
        let running = format!(
            "\u{201c}{title}\u{201d} is written on {name} now: connect it here to see it. Its folder here stays"
        );
        let closed = format!(
            "\u{201c}{title}\u{201d} is written on {name} now: connect it here to see it"
        );
        self.close_feature_workspace(project, slug, running, closed);
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
        let read = std::fs::read(store::project_dir(None, project).join(slug).join(store::FILE))
            .map_err(|err| err.to_string())
            .and_then(|raw| serde_json::from_slice::<Feature>(&raw).map_err(|e| e.to_string()));
        let mut feature = match read {
            Ok(feature) => feature,
            Err(why) => return tracing::warn!("reading {slug} back: {why}"),
        };
        feature.migrate(feature::now());
        // Shown from there meanwhile: it isn't written there.
        if let Some(list) = self.features.viewed.get_mut(project) {
            list.retain(|f| f.slug != slug);
        }
        let all = self.features.all.entry(project.to_owned()).or_default();
        all.retain(|f| f.slug != slug);
        all.push(feature);
        all.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.slug.cmp(&b.slug)));
        self.seen_folders_changed();
        self.sync_feature_panel();
        self.sync_feature_page();
        self.adopt_checks(project);
        self.show_warning(format!(
            "\u{201c}{}\u{201d} stays here: {} didn't finish taking it over",
            handing.title, handing.to
        ));
    }

    /// Another machine's record of feature `slug` won: this one's goes aside.
    pub(crate) fn lost_feature(&mut self, project: &str, slug: &str, to: &str) {
        let Some(feature) = (self.features.all.get(project).into_iter().flatten())
            .find(|f| f.slug == slug)
            .cloned()
        else {
            return;
        };
        let dir = store::project_dir(None, project).join(slug);
        let aside = dir.join(format!("{}{}", lease::ASIDE, feature::now()));
        let name = self.machine_name(to);
        let moved = crate::projects::write_atomic(&dir.join(lease::HANDED_TO), to.as_bytes())
            .and_then(|()| std::fs::rename(dir.join(store::FILE), &aside));
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
        self.leave_feature(project, slug, to, &feature);
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
struct Blocker {
    why: String,
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
fn restore_record(dir: &Path) -> std::io::Result<()> {
    std::fs::rename(dir.join(lease::HANDED), dir.join(store::FILE))?;
    for file in [lease::HANDED_TO, lease::ANSWER] {
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
    let Ok(entries) = std::fs::read_dir(store::project_dir(None, project)) else {
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
/// record names it from here on.
fn own_id_made() -> Option<String> {
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

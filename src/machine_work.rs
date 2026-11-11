//! What runs on other machines: stage agents here; Verify's checks and setups
//! in `machine_checks`, over the same links.
//!
//! A run placed on a machine has that machine's id in its record, and its pane
//! is `<machine id>/<pane>`: herdr's ids repeat across servers, and a machine's
//! name is only what this gerdr calls it. A link that is up has two threads
//! besides its own. One lists the panes of the worker session there, so an
//! agent's dot and whether it is gone come from there; the other does the work
//! in the order it was asked for (`Job`) and brings the feature's files and
//! commits back (`Pull`).
//!
//! What can't be told must not read as gone: with the link down or herdr not
//! answering a pane is taken to be there (`Work::panes` is `None`), and only a
//! list that came back without it says otherwise. The agent's page says the
//! machine can't be reached, and the watch of the run waits.
//!
//! A pane there opens here only when asked (`show_machine_pane`), as a new tab
//! attached over SSH. Closing that tab ends the view, not the agent.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use serde_json::json;
use smithay::reexports::calloop::channel;

use crate::{
    Gerdr,
    agent_tell::Told,
    agents,
    confirm::Then,
    desktop::shell_quote,
    feature::{
        self, Feature, Stage, Status,
        agents::program,
        patch::Reading,
        remote::{self, Paths, Place, Reach, Side, Trouble},
        sandbox_run::{self, Elsewhere, SandboxRun},
        store,
    },
    feature_agents::WatchKey,
    herdr::{AgentStatus, Herdr},
    machine_load::{LEAST_BUSY, Load},
    machines::{LinkInfo, LinkState},
    prepare,
};

/// How often a machine is asked how busy it is and how its sandboxes stand.
const LOOK_EVERY: Duration = Duration::from_secs(3);
/// How often the panes there are listed.
const LIST_EVERY: Duration = Duration::from_secs(2);
/// How often a feature's files and commits come back, its agent at work or
/// about to be judged.
const PULL_EVERY: Duration = Duration::from_secs(3);
/// How long the jobs' thread waits for a job before it looks what is due.
const JOB_WAIT: Duration = Duration::from_millis(400);
/// How long a new pane's shell gets to start.
const SHELL_START: Duration = Duration::from_secs(20);
/// Until when something other than a shell in a new pane is taken for its
/// startup files: then the line goes anyway, as nothing else can be there.
const SHELL_GRACE: Duration = Duration::from_secs(5);
/// How long a line sent there counts as not yet taken.
const LINE_PENDING: Duration = Duration::from_secs(20);
/// How long an agent that came to rest still counts as at work, for what it
/// did to come back. Past that, what can't come back is said by itself.
const SETTLE_MOST: Duration = Duration::from_secs(60);
/// How many times in a row bringing a feature's work back may fail for what
/// looks like a lost connection before it is said: a lost link ends sooner.
const PULL_TRIES: u32 = 5;
/// How wide a look at a terminal there asks for: wider than any, as what's wider
/// than the terminal here is cut.
const OBSERVE_COLS: u32 = 400;
/// How tall, when that machine doesn't say.
const OBSERVE_ROWS: u32 = 50;
/// What says a run's place is this machine, over the project's default.
pub const HERE: &str = "here";

/// The machine and its own id of a pane on another machine.
pub fn split_pane(pane: &str) -> Option<(&str, &str)> {
    pane.split_once('/')
}

/// A pane there, as this gerdr knows it.
pub fn qualified(machine: &str, pane: &str) -> String {
    format!("{machine}/{pane}")
}

/// A pane of the worker session there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub terminal: String,
    /// herdr's word on the agent in it, if it sees one.
    pub status: Option<AgentStatus>,
}

/// How the start of an agent there counts once its pane runs its command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartKind {
    /// Its command tells it its prompt.
    Sent,
    /// It waits for the user to send its prompt.
    Held,
    /// It goes back to its chat.
    Resumed,
}

/// Start an agent there.
#[derive(Debug)]
pub struct StartJob {
    pub project: String,
    pub slug: String,
    pub stage: Stage,
    pub n: u32,
    pub kind: StartKind,
    /// The CLI's program, which must be there.
    pub program: String,
    pub feature: Feature,
    pub paths: Paths,
    /// The feature's data folder here and there.
    pub data: PathBuf,
    pub data_there: String,
    /// Its folder there, and what to run in it.
    pub cwd: String,
    pub command: String,
}

/// What a link's jobs thread is asked for.
#[derive(Debug)]
enum Job {
    Start(Box<StartJob>),
    /// Type `line` to the agent in `pane`, once what's new of its files is
    /// there.
    Tell {
        pane: String,
        line: String,
        data: PathBuf,
        data_there: String,
        /// The line as it reads here, to keep should it fail.
        told: Box<Told>,
    },
    Close {
        pane: String,
    },
    /// Take a removed feature off the machine.
    Forget {
        script: String,
    },
}

/// A feature whose files and commits come back.
#[derive(Clone, Debug)]
struct Pull {
    project: String,
    slug: String,
    data: PathBuf,
    data_there: String,
    places: Vec<Place>,
}

/// What a link's threads share.
#[derive(Debug, Default)]
struct Shared {
    pulls: Mutex<Vec<Pull>>,
    /// Bring everything back as soon as can be: something waits for it.
    pull_now: AtomicBool,
    /// Agents' states, for the Enter pressed again on a line left in a box.
    statuses: Mutex<HashMap<String, AgentStatus>>,
    /// The sandboxes to ask about there, by project and feature.
    sandboxes: Mutex<Vec<(String, String, SandboxRun)>>,
}

/// What a link's threads tell.
#[derive(Debug)]
pub enum WorkEvent {
    /// The panes there; `None`: they couldn't be listed.
    Panes(Option<HashMap<String, Listed>>),
    /// How busy the machine is, and how the sandboxes asked of it stand.
    Looked {
        load: Option<Load>,
        sandboxes: Vec<(String, String, Elsewhere)>,
    },
    Started {
        project: String,
        slug: String,
        stage: Stage,
        n: u32,
        kind: StartKind,
        /// Its pane there and that pane's terminal, or why not.
        result: Result<(String, String), String>,
    },
    /// A line couldn't be typed to the agent in this pane.
    NotTold {
        pane: String,
        why: String,
        /// The agent quit: it is gone, not out of reach.
        quit: bool,
        /// The line, as it reads here.
        told: Box<Told>,
    },
    /// The agent in this pane never took a line.
    LineIgnored(String),
    /// A removed feature couldn't be taken off the machine: why.
    NotForgotten(String),
    /// What the machine has of the project open here.
    Viewed(Box<crate::machine_views::ViewNews>),
    /// How taking a feature over from that machine goes.
    Take(crate::machine_lease::TakeEvent),
    /// How a feature's code here stands.
    ViewCode {
        project: String,
        slug: String,
        state: crate::machine_views::CodeHere,
    },
    /// The machine's features can't be brought here: why.
    ViewTrouble(String),
    /// What it last said of that holds no more.
    ViewTroubleOver(String),
    Pulled {
        project: String,
        slug: String,
        /// When it started: all that happened before is here now.
        began: Instant,
        /// Repositories with changes not committed there, by `Repo::key`.
        dirty: Vec<String>,
        /// What the user has to sort out.
        trouble: Option<String>,
    },
}

/// A machine's work, while its link is up: dropped, its threads end.
#[derive(Debug)]
pub struct Work {
    pub id: String,
    pub target: String,
    pub info: LinkInfo,
    /// The panes there, as last listed; `None`: not known.
    pub panes: Option<HashMap<String, Listed>>,
    jobs: mpsc::Sender<Job>,
    stop: Arc<AtomicBool>,
    shared: Arc<Shared>,
    /// When the last pull of each feature that worked started.
    pulled: HashMap<(String, String), Instant>,
    /// What each feature's last pull found in the way.
    dirty: HashMap<(String, String), Vec<String>>,
    /// The panes whose agents came to rest, and when: until what they did
    /// came back, they aren't done here (see `settling`).
    rested: HashMap<String, Instant>,
    /// What is wanted of its features (see `machine_views`).
    views: Arc<Mutex<crate::machine_views::ViewAsk>>,
    /// How busy it last said it is.
    pub load: Option<Load>,
}

impl Drop for Work {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// How a link's threads say things.
pub(crate) type WorkTx = channel::Sender<(String, WorkEvent)>;

impl Work {
    /// Start the threads of a link that is up.
    fn start(id: String, target: String, info: LinkInfo, tx: WorkTx) -> std::io::Result<Self> {
        let (jobs, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Shared::default());
        let reach = Reach {
            target: target.clone(),
            control: info.control.clone(),
        };
        let view_tx = tx.clone();
        let lister = {
            let (socket, stop, tx, id) =
                (info.socket.clone(), stop.clone(), tx.clone(), id.clone());
            let shared = shared.clone();
            move || list_panes(socket, &stop, &shared, |event| send(&tx, &id, event))
        };
        std::thread::Builder::new()
            .name("machine panes".into())
            .spawn(lister)?;
        let looker = {
            let reach = reach.clone();
            let (stop, tx, id) = (stop.clone(), tx.clone(), id.clone());
            let shared = shared.clone();
            move || look_there(&reach, &stop, &shared, |event| send(&tx, &id, event))
        };
        std::thread::Builder::new()
            .name("machine looks".into())
            .spawn(looker)?;
        let worker = {
            let (socket, stop, id) = (info.socket.clone(), stop.clone(), id.clone());
            let shared = shared.clone();
            move || {
                do_work(reach, socket, rx, &stop, &shared, |event| {
                    send(&tx, &id, event)
                })
            }
        };
        std::thread::Builder::new()
            .name("machine work".into())
            .spawn(worker)?;
        let views = Arc::new(Mutex::new(crate::machine_views::ViewAsk::default()));
        let place = crate::machine_views::ViewPlace {
            id: id.clone(),
            reach: Reach {
                target: target.clone(),
                control: info.control.clone(),
            },
            there: Side {
                home: info.site.home.clone(),
                data: format!("{}/gerdr", info.site.data),
            },
            home: crate::home_dir().map_or_else(String::new, |h| h.to_string_lossy().into_owned()),
            root: feature::view::root(&crate::projects::data_dir(), &id),
        };
        let viewer = {
            let (ask, stop, id) = (views.clone(), stop.clone(), id.clone());
            move || {
                let tell = move |event| send(&view_tx, &id, event);
                crate::machine_views::watch_views(place, ask, stop, tell)
            }
        };
        std::thread::Builder::new()
            .name("machine views".into())
            .spawn(viewer)?;
        Ok(Self {
            views,
            id,
            target,
            info,
            panes: None,
            jobs,
            stop,
            shared,
            pulled: HashMap::new(),
            dirty: HashMap::new(),
            rested: HashMap::new(),
            load: None,
        })
    }

    /// Have the sandboxes in `asked` asked about from now on.
    pub(crate) fn ask_sandboxes(&self, asked: Vec<(String, String, SandboxRun)>) {
        if let Ok(mut mine) = self.shared.sandboxes.lock() {
            *mine = asked;
        }
    }

    /// A new list of the panes there: note those whose agents came to rest.
    fn listed(&mut self, panes: Option<HashMap<String, Listed>>, now: Instant) {
        let busy =
            |l: &Listed| matches!(l.status, Some(AgentStatus::Working | AgentStatus::Blocked));
        if let (Some(old), Some(new)) = (&self.panes, &panes) {
            for (pane, listed) in new {
                if busy(listed) {
                    self.rested.remove(pane);
                } else if old.get(pane).is_some_and(busy) {
                    self.rested.insert(pane.clone(), now);
                }
            }
            self.rested.retain(|pane, _| new.contains_key(pane));
        }
        self.panes = panes;
    }

    /// Whether the agent in `pane`, of the feature `key`, came to rest and
    /// what it did isn't here yet: anything that reads it as done would read
    /// its commits and files before they arrive. It is asked for at once.
    fn settling(&self, pane: &str, key: &(String, String), now: Instant) -> bool {
        let Some(rested) = self.rested.get(pane) else {
            return false;
        };
        let here = (self.pulled.get(key)).is_some_and(|began| began >= rested);
        let settling = !here && now.duration_since(*rested) < SETTLE_MOST;
        if settling {
            self.shared.pull_now.store(true, Ordering::Relaxed);
        }
        settling
    }

    /// How to run commands there.
    pub(crate) fn reach(&self) -> Reach {
        Reach {
            target: self.target.clone(),
            control: self.info.control.clone(),
        }
    }

    fn side(&self) -> Side {
        Side {
            home: self.info.site.home.clone(),
            data: format!("{}/gerdr", self.info.site.data),
        }
    }

    /// Say what is wanted of this machine's features.
    pub(crate) fn ask_views(&self, ask: crate::machine_views::ViewAsk) {
        if let Ok(mut mine) = self.views.lock() {
            *mine = ask;
        }
    }

    /// The line showing here what `terminal` in `session` there shows, taking it
    /// from nothing: that machine's gerdr shows it itself. herdr sends whole
    /// frames in base64, a line at a time, cut to the size asked for; a frame
    /// wider than this terminal is cut too, rather than wrapped and then joined
    /// up when the terminal changes size, and it waits for a new tab to get its
    /// size, as the whole frame comes only first. It is read-only: keys go nowhere.
    pub(crate) fn observe_line(
        &self,
        terminal: &str,
        session: &str,
        rows: Option<u32>,
        name: &str,
    ) -> String {
        let site = &self.info.site;
        let herdr = site.herdr.as_deref().unwrap_or("herdr");
        let there = format!(
            "env HERDR_SOCKET_PATH={} {} terminal session observe {} --cols {OBSERVE_COLS} --rows {}",
            shell_quote(&site.socket_of(session)),
            shell_quote(herdr),
            shell_quote(terminal),
            rows.unwrap_or(OBSERVE_ROWS),
        );
        let said =
            format!("This view ended. It only looked: what runs there keeps running on {name}.");
        format!(
            r#"printf '\033[?7l'; sleep 1; ssh -n -S {} -o BatchMode=yes -o ControlMaster=no {} {} | sed -u -n 's/.*"bytes":"\([^"]*\)".*/\1/p' | while IFS= read -r l; do printf %s "$l" | base64 -d; done; printf '\033[?7h\033[?25h\033[0m\n'; echo {}"#,
            shell_quote(&self.info.control.to_string_lossy()),
            shell_quote(&self.target),
            shell_quote(&there),
            shell_quote(&said),
        )
    }

    /// The line attaching a terminal here to `terminal` there, taking it from
    /// any other client. It says what ending means: the agent goes on; and, when
    /// another machine took the view, which.
    pub(crate) fn attach_line(&self, terminal: &str, name: &str) -> String {
        let site = &self.info.site;
        let herdr = site.herdr.as_deref().unwrap_or("herdr");
        let attach = format!(
            "env HERDR_SOCKET_PATH={} {} terminal attach {} --takeover",
            shell_quote(&site.socket()),
            shell_quote(herdr),
            shell_quote(terminal),
        );
        let views = format!("{}/gerdr/{VIEWS}", site.data);
        let there = viewed_by(
            &views,
            terminal,
            &crate::machine_lease::this_machines_name(),
            &attach,
        );
        let said = format!("This view ended. What runs there keeps running on {name}.");
        format!(
            "ssh -t -S {} -o BatchMode=yes -o ControlMaster=no {} {}; echo {}",
            shell_quote(&self.info.control.to_string_lossy()),
            shell_quote(&self.target),
            shell_quote(&there),
            shell_quote(&said),
        )
    }
}

/// The folder, in gerdr's folder of a machine, saying which machine last opened a
/// view of each terminal there.
const VIEWS: &str = "views";

/// `attach`, a view of `terminal` taking it from any other, run by shell where the
/// terminal is: it first says there that machine `me` has it, in folder `views`,
/// and once it ends, says which machine took it, if one did; if none did, what
/// it said goes.
fn viewed_by(views: &str, terminal: &str, me: &str, attach: &str) -> String {
    let file = shell_quote(&format!("{views}/{}", terminal.replace('/', "_")));
    let me = shell_quote(me);
    format!(
        r#"mkdir -p {views} 2>/dev/null; printf %s {me} > {file} 2>/dev/null; {attach}; s=$?; w=$(cat {file} 2>/dev/null); if [ "$w" = {me} ]; then rm -f {file}; elif [ -n "$w" ]; then printf '\nOpened on %s.\n' "$w"; fi; exit $s"#,
        views = shell_quote(views),
    )
}

/// The line attaching a terminal here to `terminal` of this machine's own worker
/// session: work another gerdr put here.
pub(crate) fn attach_here(terminal: &str) -> String {
    let socket = crate::machines::worker_socket_here();
    let attach = format!(
        "env HERDR_SOCKET_PATH={} herdr terminal attach {} --takeover",
        shell_quote(&socket.to_string_lossy()),
        shell_quote(terminal),
    );
    let views = crate::projects::data_dir().join(VIEWS);
    let me = crate::machine_lease::this_machines_name();
    format!(
        "sh -c {}; echo {}",
        shell_quote(&viewed_by(&views.to_string_lossy(), terminal, &me, &attach)),
        shell_quote("This view ended. What runs here keeps running."),
    )
}

fn send(tx: &WorkTx, id: &str, event: WorkEvent) {
    // The other end goes only with gerdr.
    let _ = tx.send((id.to_owned(), event));
}

/// The panes of the session there, with what herdr says of their agents.
fn panes_there(herdr: &mut Herdr) -> Result<HashMap<String, Listed>, String> {
    let mut panes = HashMap::new();
    for workspace in herdr.workspace_list()? {
        for pane in herdr.pane_list(&workspace.workspace_id)? {
            panes.insert(
                pane.pane_id,
                Listed {
                    terminal: pane.terminal_id,
                    status: None,
                },
            );
        }
    }
    for agent in herdr.agent_list()? {
        if agent.agent.is_some()
            && let Some(listed) = panes.get_mut(&agent.pane_id)
        {
            listed.status = Some(agent.agent_status);
        }
    }
    Ok(panes)
}

/// Keep listing the panes there, saying what changed.
fn list_panes(socket: PathBuf, stop: &AtomicBool, shared: &Shared, tell: impl Fn(WorkEvent)) {
    let mut herdr = Herdr::on_socket(socket);
    let mut last: Option<Option<HashMap<String, Listed>>> = None;
    while !stop.load(Ordering::Relaxed) {
        let listed = match panes_there(&mut herdr) {
            Ok(panes) => Some(panes),
            Err(err) => {
                tracing::debug!("listing a machine's panes: {err}");
                None
            }
        };
        if let (Some(panes), Ok(mut statuses)) = (&listed, shared.statuses.lock()) {
            *statuses = (panes.iter())
                .filter_map(|(pane, l)| Some((pane.clone(), l.status?)))
                .collect();
        }
        if last.as_ref() != Some(&listed) {
            last = Some(listed.clone());
            tell(WorkEvent::Panes(listed));
        }
        std::thread::sleep(LIST_EVERY);
    }
}

/// Do what is asked and bring back what is due, until told to stop.
/// Ask the machine how busy it is, and how the sandboxes `shared` names stand:
/// one script, in a thread of its own, as the jobs can take minutes.
fn look_there(reach: &Reach, stop: &AtomicBool, shared: &Shared, tell: impl Fn(WorkEvent)) {
    while !stop.load(Ordering::Relaxed) {
        let asked = shared
            .sandboxes
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default();
        let names: Vec<(String, &SandboxRun)> = (asked.iter())
            .map(|(project, slug, run)| (format!("{project}/{slug}"), run))
            .collect();
        let script = format!(
            "{}{}",
            crate::machine_load::LOAD_SCRIPT,
            sandbox_run::there_script(names.iter().map(|(name, run)| (name.as_str(), *run))),
        );
        // Lost, the link says so itself, and what was known goes with it.
        if let Ok(said) = reach.script(&script, stop) {
            let sandboxes = (names.iter().zip(&asked))
                .map(|((name, run), (project, slug, _))| {
                    (
                        project.clone(),
                        slug.clone(),
                        sandbox_run::read_there(&said, name, run),
                    )
                })
                .collect();
            tell(WorkEvent::Looked {
                load: crate::machine_load::parse_load(&said),
                sandboxes,
            });
        }
        let began = Instant::now();
        while began.elapsed() < LOOK_EVERY && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

fn do_work(
    reach: Reach,
    socket: PathBuf,
    jobs: mpsc::Receiver<Job>,
    stop: &AtomicBool,
    shared: &Shared,
    tell: impl Fn(WorkEvent),
) {
    let mut herdr = Herdr::on_socket(socket);
    let mut last: HashMap<(String, String), Instant> = HashMap::new();
    let mut fails: HashMap<(String, String), u32> = HashMap::new();
    while !stop.load(Ordering::Relaxed) {
        match jobs.recv_timeout(JOB_WAIT) {
            Ok(Job::Start(job)) => {
                let result = start_there(&reach, &mut herdr, &job, stop);
                tell(WorkEvent::Started {
                    project: job.project,
                    slug: job.slug,
                    stage: job.stage,
                    n: job.n,
                    kind: job.kind,
                    result: result.map_err(|t| t.text().to_owned()),
                });
            }
            Ok(Job::Tell {
                pane,
                line,
                data,
                data_there,
                told,
            }) => {
                let typed = reach
                    .push_data(&data, &data_there, false, stop)
                    .map_err(|t| (t.text().to_owned(), false))
                    .and_then(|()| type_line(&mut herdr, &pane, &line));
                if let Err((why, quit)) = typed {
                    tell(WorkEvent::NotTold {
                        pane,
                        why,
                        quit,
                        told,
                    });
                }
            }
            Ok(Job::Close { pane }) => {
                if let Err(err) = herdr.pane_close(&pane) {
                    tracing::warn!("closing pane {pane} on a machine: {err}");
                }
            }
            Ok(Job::Forget { script }) => {
                if let Err(trouble) = reach.script(&script, stop) {
                    tracing::warn!("removing a feature there: {}", trouble.text());
                    tell(WorkEvent::NotForgotten(trouble.text().to_owned()));
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
        let pulls = shared.pulls.lock().map(|p| p.clone()).unwrap_or_default();
        let now = shared.pull_now.swap(false, Ordering::Relaxed);
        last.retain(|key, _| {
            pulls
                .iter()
                .any(|p| (&p.project, &p.slug) == (&key.0, &key.1))
        });
        for pull in pulls {
            let key = (pull.project.clone(), pull.slug.clone());
            let due = last.get(&key).is_none_or(|at| at.elapsed() >= PULL_EVERY);
            if now || due {
                last.insert(key.clone(), Instant::now());
                pull_back(&reach, &pull, fails.entry(key).or_default(), stop, &tell);
            }
        }
        let statuses = shared
            .statuses
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default();
        let at_work = |pane: &str| {
            matches!(
                statuses.get(pane),
                Some(AgentStatus::Working | AgentStatus::Blocked)
            )
        };
        for pane in herdr.check_agent_lines(at_work) {
            tell(WorkEvent::LineIgnored(pane));
        }
    }
}

/// Bring what `pull` is about back here: the commits first, so a file saying
/// they are made doesn't get here before them.
fn pull_back(
    reach: &Reach,
    pull: &Pull,
    fails: &mut u32,
    stop: &AtomicBool,
    tell: impl Fn(WorkEvent),
) {
    let began = Instant::now();
    let result = reach
        .script(&remote::status_script(&pull.places), stop)
        .map(|text| remote::parse_status(&text))
        .and_then(|said| {
            reach.fetch_code(&pull.places, &said, stop)?;
            reach.pull_data(&pull.data, &pull.data_there, stop)?;
            Ok(said)
        });
    let (dirty, trouble) = match result {
        Ok(said) => (
            (said.into_iter())
                .filter(|s| s.dirty)
                .map(|s| s.key)
                .collect(),
            None,
        ),
        // The link says so itself, if that is what it is.
        Err(Trouble::Link(why)) if *fails + 1 < PULL_TRIES => {
            *fails += 1;
            return tracing::debug!("bringing work back: {why}");
        }
        Err(Trouble::Link(why) | Trouble::Work(why)) => {
            // Its files are worth having, whatever is wrong with the code.
            if let Err(err) = reach.pull_data(&pull.data, &pull.data_there, stop) {
                tracing::debug!("bringing files back: {}", err.text());
            }
            (Vec::new(), Some(why))
        }
    };
    if trouble.is_none() {
        *fails = 0;
    }
    tell(WorkEvent::Pulled {
        project: pull.project.clone(),
        slug: pull.slug.clone(),
        began,
        dirty,
        trouble,
    });
}

/// Type `line` to the agent in `pane`, if one still runs there. Err: why, and
/// whether its agent quit.
fn type_line(herdr: &mut Herdr, pane: &str, line: &str) -> Result<(), (String, bool)> {
    match herdr.process_info(pane) {
        Err(err) => Err((err.clone(), Herdr::says_no_pane(&err))),
        Ok(info) => {
            if prepare::step(&Ok(info)) != prepare::Step::Busy {
                return Err((crate::feature_build::AGENT_QUIT.to_owned(), true));
            }
            herdr
                .send_agent_line(pane, line)
                .map_err(|err| (err, false))
        }
    }
}

/// Set up everything `job` needs there, and start its command in a new tab of
/// the project's workspace. Its pane there.
fn start_there(
    reach: &Reach,
    herdr: &mut Herdr,
    job: &StartJob,
    stop: &AtomicBool,
) -> Result<(String, String), Trouble> {
    if let Some(missing) = reach.missing(&job.program, stop)? {
        return Err(Trouble::Work(format!("{missing} isn't installed there")));
    }
    reach.push_code(&job.feature, &job.paths, &job.data_there, stop)?;
    // A new run starts on what is here. An agent going back to its chat has
    // written there what didn't make it here yet.
    let exact = job.kind != StartKind::Resumed;
    reach.push_data(&job.data, &job.data_there, exact, stop)?;
    reach.push_graphs(&job.feature, &job.paths, stop);
    let (pane, terminal, info) = open_there(herdr, &job.project, &job.cwd, stop)?;
    let line = prepare::line_there(&info, &job.cwd, &job.command);
    herdr.send_shell_line(&pane, &line).map_err(Trouble::Link)?;
    Ok((pane, terminal))
}

/// A new tab in `cwd`, in `project`'s workspace of the worker session there,
/// once its shell can be told something: its pane, terminal and what runs in
/// it.
pub(crate) fn open_there(
    herdr: &mut Herdr,
    project: &str,
    cwd: &str,
    stop: &AtomicBool,
) -> Result<(String, String, crate::herdr::ProcessInfo), Trouble> {
    let link = |err: String| Trouble::Link(err);
    let dir = Path::new(cwd);
    let workspaces = herdr.workspace_list().map_err(link)?;
    let found = (workspaces.iter()).find(|w| w.label == project);
    let pane = match found {
        Some(w) => herdr.tab_create(&w.workspace_id, dir, &json!({})),
        None => herdr.workspace_create(dir, project, true, &json!({})),
    }
    .map_err(link)?;
    let (pane, terminal) = (pane.pane_id, pane.terminal_id);
    let started = Instant::now();
    let info = loop {
        let info = herdr.process_info(&pane).map_err(link)?;
        let step = prepare::step(&Ok(info.clone()));
        let late = started.elapsed() > SHELL_GRACE;
        match step {
            prepare::Step::Send => break info,
            prepare::Step::Busy if late => break info,
            prepare::Step::Drop => {
                return Err(Trouble::Work("its pane closed as it started".into()));
            }
            _ if started.elapsed() > SHELL_START => {
                return Err(Trouble::Work("its pane's shell didn't start".into()));
            }
            _ => std::thread::sleep(Duration::from_millis(200)),
        }
        if stop.load(Ordering::Relaxed) {
            return Err(Trouble::Link("stopped".into()));
        }
    };
    Ok((pane, terminal, info))
}

/// Whether what `stage` runs can go on another machine: its agent, for Verify
/// its checks, for Manual test its sandbox.
pub fn takes_a_place(stage: Stage) -> bool {
    matches!(stage, Stage::Verify | Stage::ManualTest) || feature::agents::needs(stage).is_some()
}

/// Where a stage's next agent runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Placed {
    Here,
    /// On the machine with this id, which is up.
    There(String),
    /// On this machine, by the name the user knows it by, which isn't up.
    Away(String),
}

impl Gerdr {
    /// Where the threads of links say things; set up the first time.
    pub(crate) fn work_sender(&mut self) -> Option<WorkTx> {
        if self.machines.work_tx.is_none() {
            let (tx, rx) = channel::channel::<(String, WorkEvent)>();
            let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
                if let channel::Event::Msg((id, event)) = event {
                    state.work_event(&id, event);
                }
            });
            match inserted {
                Ok(_) => self.machines.work_tx = Some(tx),
                Err(err) => {
                    tracing::warn!("machines: {err}");
                    return None;
                }
            }
        }
        self.machines.work_tx.clone()
    }

    /// Machine `name`'s link is up: its work can start, and go on with what
    /// ran there.
    pub(crate) fn start_work(&mut self, name: &str, target: String, info: LinkInfo) {
        let Some(tx) = self.work_sender() else {
            return self.show_error(format!("gerdr can't run work on {name}"));
        };
        let id = info.site.id.clone();
        self.machines.names.insert(id.clone(), name.to_owned());
        let place = feature::view::Place {
            name: name.to_owned(),
            home: info.site.home.clone(),
            data: format!("{}/gerdr", info.site.data),
        };
        let root = feature::view::root(&crate::projects::data_dir(), &id);
        if let Err(err) = feature::view::write_place(&root, &place) {
            tracing::warn!("machine {name}: noting where things are there: {err}");
        }
        let place_id = id.clone();
        match Work::start(id, target, info, tx) {
            Ok(work) => {
                if let Some(link) = self.machines.links.get_mut(name) {
                    link.work = Some(work);
                }
                // Checks that went on there while nobody could wait for them.
                if let Some(project) = self.project.clone() {
                    self.adopt_checks(&project);
                }
                self.clean_left_there(&place_id);
                self.look_for_sandbox_leftovers_there(&place_id);
            }
            Err(err) => {
                tracing::warn!("machine {name}: starting its work: {err}");
                self.show_error(format!("gerdr can't run work on {name}: {err}"));
            }
        }
    }

    pub(crate) fn work_of(&self, id: &str) -> Option<&Work> {
        (self.machines.links.values())
            .filter_map(|link| link.work.as_ref())
            .find(|work| work.id == id)
    }

    /// How to reach machine `id`, and its folders, for a thread of its own
    /// that copies AI work from there: none while its link is down.
    pub(crate) fn archive_reach(&self, machine_id: &str) -> Option<(Reach, Side)> {
        let work = self.work_of(machine_id)?;
        Some((work.reach(), work.side()))
    }

    fn work_of_mut(&mut self, id: &str) -> Option<&mut Work> {
        (self.machines.links.values_mut())
            .filter_map(|link| link.work.as_mut())
            .find(|work| work.id == id)
    }

    /// Whether machine `id` is one saved in the settings, connected or not.
    pub(crate) fn machine_saved(&self, id: &str) -> bool {
        (self.machines.names.get(id)).is_some_and(|name| self.machines.saved.contains_key(name))
    }

    /// What the user calls machine `id`.
    /// This one is "this machine", whatever others call it.
    pub(crate) fn machine_name(&self, id: &str) -> String {
        if Self::is_this_machine(id) {
            return "this machine".to_owned();
        }
        match self.machines.names.get(id) {
            Some(name) => name.clone(),
            None => "another machine".to_owned(),
        }
    }

    /// Whether herdr pane `pane` is still there. On another machine, unless
    /// told otherwise by a list of its panes: what can't be reached isn't gone.
    pub(crate) fn pane_there(&self, pane: &str) -> bool {
        if let Some((_, alive)) = self.seen_pane_state(pane) {
            return alive;
        }
        let Some((id, bare)) = split_pane(pane) else {
            return self.herdr_pane_place(pane).is_some();
        };
        (self.work_of(id).and_then(|w| w.panes.as_ref())).is_none_or(|p| p.contains_key(bare))
    }

    /// What's known of pane `pane` on another machine, unless it is here.
    /// Not reachable, its agent isn't known to do anything.
    pub(crate) fn machine_pane_state(&self, pane: &str) -> Option<(Option<AgentStatus>, bool)> {
        if let Some(state) = self.seen_pane_state(pane) {
            return Some(state);
        }
        let (id, bare) = split_pane(pane)?;
        let Some(panes) = self.work_of(id).and_then(|w| w.panes.as_ref()) else {
            return Some((Some(AgentStatus::Unknown), true));
        };
        let key = self
            .owner_of_pane(pane)
            .map(|f| (f.project.clone(), f.slug.clone()));
        let settling = (self.work_of(id).zip(key.as_ref()))
            .is_some_and(|(work, key)| work.settling(bare, key, Instant::now()));
        Some(match panes.get(bare) {
            Some(_) if settling => (Some(AgentStatus::Working), true),
            Some(listed) => (listed.status, true),
            None => (None, false),
        })
    }

    /// Whether the machine of `pane` can't say how it stands now: nothing
    /// that depends on it is decided meanwhile.
    pub(crate) fn pane_out_of_reach(&self, pane: &str) -> bool {
        if let Some(out) = self.seen_out_of_reach(pane) {
            return out;
        }
        split_pane(pane)
            .is_some_and(|(id, _)| (self.work_of(id).and_then(|w| w.panes.as_ref())).is_none())
    }

    /// The socket to ask about `pane` on, and what it is called there.
    pub(crate) fn pane_socket(&self, pane: &str) -> Option<(PathBuf, String)> {
        match split_pane(pane) {
            None => {
                let socket = self.herdr.as_ref()?.socket().to_path_buf();
                Some((socket, pane.to_owned()))
            }
            Some((id, bare)) => {
                // In its writer's own session: no socket to that one comes here.
                if (self.features.seen.get(pane)).is_some_and(|seen| seen.session.is_some()) {
                    return None;
                }
                let work = self.work_of(id)?;
                Some((work.info.socket.clone(), bare.to_owned()))
            }
        }
    }

    /// Whether a line typed to `pane` on a machine may not be taken yet.
    pub(crate) fn machine_line_pending(&self, pane: &str) -> bool {
        (self.machines.lines.get(pane)).is_some_and(|at| at.elapsed() < LINE_PENDING)
    }

    /// This machine's folders and the ones of machine `id`, for `feature`.
    pub(crate) fn machine_paths(&self, id: &str, feature: Option<&Feature>) -> Option<Paths> {
        let work = self.work_of(id)?;
        let name = self.machine_name(id);
        let here = Side {
            home: crate::home_dir()?.to_string_lossy().into_owned(),
            data: crate::projects::data_dir().to_string_lossy().into_owned(),
        };
        let mut folders = std::collections::BTreeMap::new();
        let projects = (feature.iter())
            .flat_map(|f| {
                std::iter::once(f.project.as_str())
                    .chain(f.links.iter().map(|l| l.project.as_str()))
            })
            .chain(self.project.as_deref());
        for project in projects {
            if let Some(project) = self.projects.project(project)
                && let Some(mine) = project.workflow.machine_folders.get(&name)
            {
                folders.extend(mine.iter().map(|(f, t)| (f.clone(), t.clone())));
            }
        }
        Some(Paths::new(&here, &work.side(), &folders, feature))
    }

    /// The feature whose stage agent or check runs in `pane`.
    fn owner_of_pane(&self, pane: &str) -> Option<&Feature> {
        let viewed = self.features.viewed.values().flatten();
        (self.features.all.values().flatten().chain(viewed))
            .find(|f| crate::feature::view::panes_of(f).contains(&pane))
    }

    /// Type the line `told` to the agent in `pane` on another machine, its
    /// paths as they read there. It goes in the machine's turn: a failure
    /// comes later, as a warning.
    pub(crate) fn tell_machine_pane(&mut self, pane: &str, told: &Told) -> Result<(), String> {
        let (id, bare) = split_pane(pane).ok_or_else(|| String::from("no such pane"))?;
        let name = self.machine_name(id);
        let Some(work) = self.work_of(id) else {
            return Err(format!("{name} can't be reached"));
        };
        let Some(feature) = self.owner_of_pane(pane) else {
            return Err("its feature is gone".into());
        };
        let Some(paths) = self.machine_paths(id, Some(feature)) else {
            return Err(format!("{name} can't be reached"));
        };
        let data = store::dir(None, &feature.project, &feature.slug);
        let job = Job::Tell {
            pane: bare.to_owned(),
            line: paths.text(&told.text),
            data_there: paths.there(&data),
            data,
            told: Box::new(told.clone()),
        };
        work.jobs
            .send(job)
            .map_err(|_| format!("{name} can't be reached"))?;
        self.machines.lines.insert(pane.to_owned(), Instant::now());
        Ok(())
    }

    /// Close `pane` on another machine, and the view of it here if any. A
    /// machine out of reach keeps it until it is asked again: its run forgets
    /// it either way, so it is said.
    pub(crate) fn close_machine_pane(&mut self, pane: &str) {
        let Some((id, bare)) = split_pane(pane) else {
            return;
        };
        let sent = (self.work_of(id)).is_some_and(|work| {
            work.jobs
                .send(Job::Close {
                    pane: bare.to_owned(),
                })
                .is_ok()
        });
        if !sent {
            let name = self.machine_name(id);
            tracing::warn!("{name} is out of reach: pane {bare} stays there");
            self.show_warning(format!(
                "{name} can't be reached: what ran there keeps running"
            ));
        }
        if let Some(view) = self.machines.views.remove(pane) {
            self.close_herdr_pane(&view);
        }
    }

    /// Close every agent and check `feature` has on other machines: it is
    /// over.
    pub(crate) fn close_machine_panes_of(&mut self, feature: &Feature) {
        let agents = feature.runs.values().filter_map(|r| r.pane.clone());
        let checks = (feature.verify.iter())
            .flat_map(|v| &v.checks)
            .filter_map(|c| c.pane.clone());
        // A kept sandbox's tabs are its old writer's to close.
        let sandbox = (feature.sandbox.iter())
            .filter(|run| !run.kept)
            .flat_map(|run| run.panes.clone());
        let mut panes: Vec<String> = (agents.chain(checks).chain(sandbox))
            .filter(|p| split_pane(p).is_some())
            .collect();
        panes.sort();
        panes.dedup();
        for pane in panes {
            self.close_machine_pane(&pane);
        }
    }

    /// `feature` is removed here: take what it left on other machines off
    /// them, its worktrees and files. A machine out of reach keeps it, and it
    /// is said.
    pub(crate) fn forget_on_machines(&mut self, feature: &Feature) {
        let agents = feature.runs.values().filter_map(|r| r.machine.clone());
        let checks = feature.verify.iter().filter_map(|v| v.machine.clone());
        let setups = (feature.setup.iter().chain(&feature.setups_elsewhere))
            .filter_map(|s| s.machine.clone());
        let mut ids: Vec<String> = agents.chain(checks).chain(setups).collect();
        ids.sort();
        ids.dedup();
        for id in ids {
            let job = self
                .forget_script(&id, feature)
                .map(|script| Job::Forget { script });
            let sent =
                (job.zip(self.work_of(&id))).is_some_and(|(job, w)| w.jobs.send(job).is_ok());
            // What's kept for a sandbox there takes this along once that's back.
            let waits = crate::machine_sandbox::left_file(&id, &feature.project, &feature.slug);
            if !sent && !waits.exists() {
                let name = self.machine_name(&id);
                tracing::warn!("{name} is out of reach: {} stays there", feature.slug);
                self.show_warning(format!(
                    "{name} can't be reached: this feature's copy stays there"
                ));
            }
        }
    }

    /// The script taking what `feature` left off machine `id`.
    pub(crate) fn forget_script(&self, id: &str, feature: &Feature) -> Option<String> {
        let paths = self.machine_paths(id, Some(feature))?;
        let data = store::dir(None, &feature.project, &feature.slug);
        Some(remote::forget_script(
            &remote::places(feature, &paths),
            &remote::doc_dirs(feature, &paths),
            &paths.there(&data),
        ))
    }

    /// Show `pane` on another machine: its view's tab, or a new one attached
    /// to it, taking it from any other view. Whether it is shown; if not, the
    /// user was told why.
    pub(crate) fn show_machine_pane(&mut self, pane: &str) -> bool {
        let Some((id, bare)) = split_pane(pane) else {
            return false;
        };
        let name = self.machine_name(id);
        if let Some(view) = self.machines.views.get(pane).cloned() {
            let open = self.herdr_pane_place(&view).is_some();
            if open && self.pane_bar_content(&view).status.runs() {
                return self.show_herdr_pane(&view);
            }
            // Its connection dropped, or another took the terminal: a new
            // view takes the old one's place.
            self.machines.views.remove(pane);
            if open && self.work_of(id).is_some() {
                self.close_herdr_pane(&view);
            }
        }
        let command = match self.seen_pane_line(pane) {
            Some(Ok(line)) => line,
            Some(Err(why)) => {
                self.show_warning(why);
                return false;
            }
            None => {
                let Some(work) = self.work_of(id) else {
                    self.show_warning(format!(
                        "{name} can't be reached: what runs there keeps running"
                    ));
                    return false;
                };
                let Some(panes) = &work.panes else {
                    self.show_warning(format!(
                        "{name} isn't answering yet: what runs there keeps running"
                    ));
                    return false;
                };
                let Some(listed) = panes.get(bare) else {
                    self.show_warning(format!("That terminal is gone from {name}"));
                    return false;
                };
                work.attach_line(&listed.terminal, &name)
            }
        };
        let owner = self
            .owner_of_pane(pane)
            .map(|f| (f.project.clone(), f.slug.clone(), f.dir.clone()));
        let opened = match owner {
            Some((project, slug, dir)) => {
                self.open_feature_tab(&project, &slug, &dir, Some(command))
            }
            None => {
                let w = self.active_workspace;
                let dir = self.workspaces.get(w).map(|ws| ws.dir.clone());
                dir.and_then(|dir| self.new_tab_pane(w, &dir, Some(command)))
            }
        };
        let Some(view) = opened else {
            tracing::warn!("no tab to show pane {pane}");
            self.show_error(format!("Couldn't open a tab to show what runs on {name}"));
            return false;
        };
        self.machines.views.insert(pane.to_owned(), view);
        true
    }

    /// Whether local pane `pane` only shows something on another machine:
    /// closing it ends the view, and nothing runs that would be lost.
    pub(crate) fn is_machine_view(&self, pane: &str) -> bool {
        self.machines.views.values().any(|view| view == pane)
    }

    /// Start `job` on machine `id`. The run waits meanwhile: `machine_started`
    /// says what came of it.
    pub(crate) fn start_on_machine(&mut self, id: &str, job: StartJob) {
        let name = self.machine_name(id);
        let key: WatchKey = (job.project.clone(), job.slug.clone(), job.stage);
        let (project, slug, stage) = (job.project.clone(), job.slug.clone(), job.stage);
        let sent = (self.work_of(id))
            .is_some_and(|work| work.jobs.send(Job::Start(Box::new(job))).is_ok());
        if sent {
            self.machines.starting.insert(key, id.to_owned());
            self.show_notice(format!(
                "Starting the {} agent on {name}\u{2026}",
                stage.label()
            ));
            return;
        }
        tracing::warn!("machine {name} took no job: {project}/{slug}");
        self.show_warning(format!(
            "{name} can't be reached: try again once it is back"
        ));
    }

    /// Whether `stage`'s agent of feature `slug` is on its way up there.
    pub(crate) fn starting_on_machine(&self, project: &str, slug: &str, stage: Stage) -> bool {
        (self.machines.starting).contains_key(&(project.to_owned(), slug.to_owned(), stage))
    }

    /// The agents on their way up on a machine whose link has gone won't get
    /// there: their runs wait for Start agent.
    pub(crate) fn starts_lost(&mut self) {
        let lost: Vec<(WatchKey, String)> = (self.machines.starting.iter())
            .filter(|(_, id)| self.work_of(id).is_none())
            .map(|(key, id)| (key.clone(), id.clone()))
            .collect();
        for (key, id) in lost {
            self.machines.starting.remove(&key);
            let name = self.machine_name(&id);
            let stage = key.2.label();
            tracing::warn!("{}/{}: lost {name} while starting {stage}", key.0, key.1);
            self.show_warning(format!(
                "{stage}'s agent didn't start: {name} went away. Start agent tries again"
            ));
        }
    }

    /// What the threads of machine `id`'s link say.
    fn work_event(&mut self, id: &str, event: WorkEvent) {
        match event {
            WorkEvent::Panes(panes) => {
                if let Some(work) = self.work_of_mut(id) {
                    work.listed(panes, Instant::now());
                }
            }
            WorkEvent::Looked { load, sandboxes } => {
                if let Some(work) = self.work_of_mut(id) {
                    work.load = load;
                }
                self.machine_seen(id);
                self.sandboxes_looked(id, sandboxes);
            }
            WorkEvent::Started {
                project,
                slug,
                stage,
                n,
                kind,
                result,
            } => self.machine_started(id, (&project, &slug, stage), n, kind, result),
            WorkEvent::NotTold {
                pane,
                why,
                quit,
                told,
            } => {
                let name = self.machine_name(id);
                tracing::warn!("telling pane {pane} on {name}: {why}");
                self.told_failed(&told, crate::ai_work::Via::Machine, &why);
                let then = if quit {
                    ""
                } else {
                    ": try again once it is back"
                };
                self.show_warning(format!("The agent on {name} wasn't told: {why}{then}"));
            }
            WorkEvent::Viewed(news) => {
                let project = news.project.clone();
                self.machine_viewed(id, *news);
                self.forget_left_of(id, &project);
            }
            WorkEvent::Take(event) => self.take_event(event),
            WorkEvent::ViewCode {
                project,
                slug,
                state,
            } => {
                let here = state == crate::machine_views::CodeHere::Here;
                if let crate::machine_views::CodeHere::Failed(why) = &state {
                    tracing::warn!("the code of {project}/{slug} didn't come: {why}");
                }
                (self.features.code_here).insert((project.clone(), slug.clone()), state);
                if here {
                    self.probe_feature(&project, &slug);
                }
            }
            WorkEvent::ViewTrouble(why) => {
                let name = self.machine_name(id);
                tracing::warn!("machine {name}: {why}");
                let text = crate::machine_views::view_trouble_text(&name, &why);
                self.report_problem("machine features", text);
            }
            WorkEvent::ViewTroubleOver(why) => {
                let name = self.machine_name(id);
                let text = crate::machine_views::view_trouble_text(&name, &why);
                crate::problems::forget("machine features", &text);
            }
            WorkEvent::NotForgotten(why) => {
                let name = self.machine_name(id);
                self.show_warning(format!("A removed feature's copy stayed on {name}: {why}"));
            }
            WorkEvent::LineIgnored(pane) => {
                let name = self.machine_name(id);
                tracing::warn!("pane {pane} on {name} never took the line typed to it");
                self.show_warning(format!(
                    "An agent on {name} didn't take the line typed to it: Show agent, then press Enter"
                ));
            }
            WorkEvent::Pulled {
                project,
                slug,
                began,
                dirty,
                trouble,
            } => {
                let key = (project, slug);
                let told = self.machines.troubles.get(&key) == trouble.as_ref();
                match &trouble {
                    Some(text) => {
                        tracing::warn!("{}/{}: {text}", key.0, key.1);
                        (self.machines.troubles).insert(key.clone(), text.clone());
                    }
                    None => {
                        self.machines.troubles.remove(&key);
                    }
                }
                self.report_work_troubles();
                if let Some(work) = self.work_of_mut(id) {
                    work.dirty.insert(key.clone(), dirty);
                    // Nothing can be decided on code that couldn't come back.
                    if trouble.is_none() {
                        work.pulled.insert(key, began);
                    }
                }
                if !told && let Some(text) = trouble {
                    let name = self.machine_name(id);
                    self.show_warning(format!("Work on {name} can't come back: {text}"));
                }
            }
        }
    }

    /// What keeps work from coming back goes on the "!" badge.
    fn report_work_troubles(&self) {
        let mut texts: Vec<String> = (self.machines.troubles.iter())
            .map(|((_, slug), text)| format!("Feature {slug}: {text}"))
            .collect();
        texts.sort();
        crate::problems::set("machine work", texts);
    }

    /// Stop `stage`'s agent on another machine of feature `slug`, asking
    /// first while it works: Show agent's tab only ends the view of it.
    pub(crate) fn ask_stop_on_machine(&mut self, project: &str, slug: &str, stage: Stage) {
        let Some(run) = self.feature(project, slug).and_then(|f| f.runs.get(&stage)) else {
            return;
        };
        let Some(id) = run.machine.clone().filter(|_| run.pane.is_some()) else {
            return self.show_warning(format!("{} has no agent on another machine", stage.label()));
        };
        if !self.run_at_work(run) {
            return self.stop_on_machine(project, slug, stage);
        }
        let name = self.machine_name(&id);
        self.ask(
            format!("Stop {}?", run.agent),
            format!("It is working on {name}: stopping it there ends what it is doing"),
            "Stop agent",
            Then::StopOnMachine(project.to_owned(), slug.to_owned(), stage),
        );
    }

    /// Stop `stage`'s agent on another machine. Out of reach, it runs on, and
    /// its run still knows it.
    pub(crate) fn stop_on_machine(&mut self, project: &str, slug: &str, stage: Stage) {
        let run = self.feature(project, slug).and_then(|f| f.runs.get(&stage));
        let Some((id, pane)) = run.and_then(|r| Some((r.machine.clone()?, r.pane.clone()?))) else {
            return;
        };
        if self.work_of(&id).is_none() {
            let name = self.machine_name(&id);
            return self.show_warning(format!(
                "{name} can't be reached: its agent keeps running there"
            ));
        }
        self.close_machine_pane(&pane);
        self.forget_pane(project, slug, stage);
    }

    /// Whether a fresh start of `stage`'s agent would end an agent at work,
    /// here or on another machine (nothing here shows that one, or could).
    /// Then it asks, and it starts if told to (`Then::FreshOnMachine`).
    pub(crate) fn asks_before_fresh(&mut self, project: &str, slug: &str, stage: Stage) -> bool {
        let Some(run) = self.feature(project, slug).and_then(|f| f.runs.get(&stage)) else {
            return false;
        };
        let live = run.pane.as_deref().is_some_and(|p| self.pane_there(p));
        let Some(id) = run
            .machine
            .as_deref()
            .filter(|_| live && self.run_at_work(run))
        else {
            // One at work here asks too: Restart would stop it mid-work.
            if live && self.run_at_work(run) {
                let agent = run.agent.clone();
                self.ask(
                    format!("Restart {}'s agent?", stage.label()),
                    format!("{agent} is working: it stops, and a fresh one starts on the prompt"),
                    "Restart",
                    Then::FreshOnMachine(project.to_owned(), slug.to_owned(), stage),
                );
                return true;
            }
            return false;
        };
        let name = self.machine_name(id);
        self.ask(
            format!("Start {} afresh?", run.agent),
            format!("The one on {name} is working: the new one takes its place and it stops"),
            "Start fresh",
            Then::FreshOnMachine(project.to_owned(), slug.to_owned(), stage),
        );
        true
    }

    /// What came of starting a run's agent on machine `id`. One no longer the
    /// stage's, started meanwhile, has no place to be: its pane goes.
    fn machine_started(
        &mut self,
        id: &str,
        (project, slug, stage): (&str, &str, Stage),
        n: u32,
        kind: StartKind,
        result: Result<(String, String), String>,
    ) {
        let key = (project.to_owned(), slug.to_owned(), stage);
        // Said already if its link went first: it is only that coming back.
        let awaited = self.machines.starting.remove(&key).is_some();
        let name = self.machine_name(id);
        let run = self.feature(project, slug).and_then(|f| f.runs.get(&stage));
        let wanted = run.is_some_and(|r| r.n == n && r.machine.as_deref() == Some(id));
        let agent = run.map_or_else(String::new, |r| r.agent.clone());
        let cwd = (self.feature(project, slug))
            .and_then(|f| Some(self.machine_paths(id, Some(f))?.there(&f.dir)))
            .unwrap_or_default();
        let (pane, terminal) = match result {
            Ok((pane, _)) if !wanted => {
                let pane = qualified(id, &pane);
                tracing::info!("{project}/{slug}: pane {pane} started for a run that is over");
                return self.close_machine_pane(&pane);
            }
            Err(why) if !wanted || !awaited => {
                return tracing::info!("{project}/{slug}: a run that is over didn't start: {why}");
            }
            Err(why) => {
                tracing::warn!("{project}/{slug}: starting on {name}: {why}");
                self.show_warning(format!(
                    "Couldn't start {agent} on {name}: {why}; Start agent tries again"
                ));
                if let Some(f) = self.feature_mut(project, slug) {
                    f.log(
                        feature::now(),
                        format!("{} agent couldn't start on {name}: {why}", stage.label()),
                    );
                }
                self.save_feature(project, slug);
                return;
            }
            Ok(found) => found,
        };
        // There by now, whatever the last list of its panes said.
        if let Some(panes) = self.work_of_mut(id).and_then(|w| w.panes.as_mut()) {
            let status = None;
            panes.insert(pane.clone(), Listed { terminal, status });
        }
        let pane = qualified(id, &pane);
        let now = feature::now();
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        let Some(r) = f.runs.get_mut(&stage) else {
            return;
        };
        r.pane = Some(pane.clone());
        r.nudged = false;
        if kind == StartKind::Sent {
            r.held = false;
            // Its work starts now: what it writes is new from here.
            r.started = now;
        }
        f.log(
            now,
            format!("{} agent runs on {name}, in {cwd}", stage.label()),
        );
        self.save_feature(project, slug);
        self.features.agents.remove(&key);
        if stage == Stage::Build {
            self.fixing_moved(project, slug, agents::AgentKey::Herdr(pane.clone()));
        }
        match kind {
            StartKind::Sent => {
                self.show_success(format!("{agent} is on {} on {name}", stage.label()));
                // Started from its page and still there: the user waits to
                // answer it. Gone elsewhere, the screen stays theirs.
                let at_feature = (self.features.panel.as_ref())
                    .is_some_and(|p| p.project == project && p.slug == slug);
                if crate::feature_agents::asks_first(stage) && at_feature {
                    self.show_machine_pane(&pane);
                }
            }
            StartKind::Resumed => {
                self.show_success(format!("Resumed {agent} on {name}"));
            }
            StartKind::Held => {
                let text = format!(
                    "{agent} waits on {name} for its {} prompt: send it from its page",
                    stage.label()
                );
                self.notify_feature(project, slug, Some(pane), &text);
                self.show_success(text);
            }
        }
    }

    /// Say to each machine what is due to come back: what its agents in the
    /// shown project may be writing. A run done and at rest writes nothing
    /// more until it is told to, and a file taken away here must not come
    /// back from there.
    pub(crate) fn machine_work_tick(&mut self) {
        self.reach_tick();
        self.publish_live();
        // Asked over another machine's link to this one: this one may have none.
        self.lease_tick();
        // Before anything that needs a link: losing the last closes what used it.
        self.sandbox_places_tick();
        self.sandbox_views_tick();
        self.sandbox_lan_tick();
        if self.machines.links.values().all(|link| link.work.is_none()) {
            return;
        }
        self.views_tick();
        self.same_repos_tick();
        let mut pulls: HashMap<String, Vec<Pull>> = HashMap::new();
        let listed = self.project.iter().filter_map(|p| self.features.all.get(p));
        for feature in listed.flatten().filter(|f| f.status == Status::Open) {
            let mut machines: Vec<&str> = (feature.runs.values())
                .filter(|run| run.ready.is_none() || self.run_at_work(run))
                .filter(|run| run.pane.is_some())
                .filter_map(|run| run.machine.as_deref())
                .collect();
            machines.sort_unstable();
            machines.dedup();
            for id in machines {
                let Some(paths) = self.machine_paths(id, Some(feature)) else {
                    continue;
                };
                let data = store::dir(None, &feature.project, &feature.slug);
                pulls.entry(id.to_owned()).or_default().push(Pull {
                    project: feature.project.clone(),
                    slug: feature.slug.clone(),
                    data_there: paths.there(&data),
                    data,
                    places: remote::places(feature, &paths),
                });
            }
        }
        // What no longer comes back has nothing in its way any more.
        let coming = |key: &(String, String)| {
            pulls
                .values()
                .flatten()
                .any(|p| (&p.project, &p.slug) == (&key.0, &key.1))
        };
        let before = self.machines.troubles.len();
        self.machines.troubles.retain(|key, _| coming(key));
        if self.machines.troubles.len() != before {
            self.report_work_troubles();
        }
        for work in (self.machines.links.values()).filter_map(|link| link.work.as_ref()) {
            if let Ok(mut mine) = work.shared.pulls.lock() {
                *mine = pulls.remove(&work.id).unwrap_or_default();
            }
        }
    }

    fn run_at_work(&self, run: &feature::agents::StageRun) -> bool {
        matches!(
            self.stage_run_state(run).0,
            Some(AgentStatus::Working | AgentStatus::Blocked)
        )
    }

    /// Whether what machine `id`'s agent of feature `slug` did before `since`
    /// is here. Not yet, it is brought back as soon as can be.
    pub(crate) fn machine_pulled_since(
        machines: &crate::machines::Machines,
        id: &str,
        (project, slug): (&str, &str),
        since: Instant,
    ) -> bool {
        let work = (machines.links.values())
            .filter_map(|link| link.work.as_ref())
            .find(|work| work.id == id);
        let Some(work) = work else {
            return false;
        };
        let key = (project.to_owned(), slug.to_owned());
        let here = work.pulled.get(&key).is_some_and(|began| *began >= since);
        if !here {
            work.shared.pull_now.store(true, Ordering::Relaxed);
        }
        here
    }

    /// `reading` of feature `slug`, with the changes left uncommitted on
    /// machines: they aren't in the code here, and they count as if they were.
    pub(crate) fn machine_dirt(&self, project: &str, slug: &str) -> Vec<String> {
        let key = (project.to_owned(), slug.to_owned());
        let dirt = (self.machines.links.values())
            .filter_map(|link| link.work.as_ref())
            .filter_map(|work| work.dirty.get(&key));
        let mut keys: Vec<String> = dirt.flatten().cloned().collect();
        keys.sort();
        keys.dedup();
        keys
    }

    /// Where `stage`'s next agent of `feature` is to run, by its pick or else
    /// the project's file.
    pub(crate) fn placed(&self, feature: &Feature, stage: Stage) -> Placed {
        if self.goes_to_least_busy(feature, stage) {
            return self.least_busy(feature);
        }
        if let Some(id) = feature.stage_machines.get(&stage) {
            // A feature another machine writes says "here" for that machine,
            // and may name this one by its id.
            let id = match (id.as_str(), feature.writer.as_deref()) {
                (HERE, Some(writer)) => writer,
                (HERE, None) => return Placed::Here,
                (id, _) => id,
            };
            if Self::is_this_machine(id) {
                return Placed::Here;
            }
            return match self.work_of(id) {
                Some(_) => Placed::There(id.to_owned()),
                None => Placed::Away(self.machine_name(id)),
            };
        }
        let project = self.projects.project(&feature.project);
        let Some(name) = project.and_then(|p| p.workflow.machines.get(&stage)) else {
            return Placed::Here;
        };
        let up = (self.machines.links.get(name)).and_then(|link| match &link.state {
            LinkState::Up(info) => Some(info.site.id.clone()),
            _ => None,
        });
        match up {
            Some(id) if self.work_of(&id).is_some() => Placed::There(id),
            _ => Placed::Away(name.clone()),
        }
    }

    /// What the button placing `stage`'s next agent says: none with no
    /// machine to place it on.
    pub(crate) fn place_label(&self, feature: &Feature, stage: Stage) -> Option<String> {
        let running = stage == Stage::Verify && crate::feature_checks::checks_running(feature);
        // A sandbox's page offers it as long as there is no sandbox. It can be
        // tried again once its stage is done, and has what it's on to say so.
        let over = match stage {
            Stage::ManualTest => feature.sandbox.is_some(),
            _ => feature.is_done(stage),
        };
        if feature.status != Status::Open
            || !takes_a_place(stage)
            || over
            || running
            || !self.takes_a_machine(feature, stage)
        {
            return None;
        }
        let picked = feature.stage_machines.contains_key(&stage);
        if self.machines.saved.is_empty() && !picked {
            return None;
        }
        if self.goes_to_least_busy(feature, stage) {
            return Some("Runs on the least busy".to_owned());
        }
        Some(match self.placed(feature, stage) {
            Placed::Here => "Runs here".to_owned(),
            Placed::There(id) => format!("Runs on {}", self.machine_name(&id)),
            Placed::Away(name) => format!("Runs on {name}: away"),
        })
    }

    /// Run `stage`'s next agent on the next machine that is up, then on whichever
    /// is least busy, then here again (Ctrl+Shift+W, the button on its agent
    /// card). One that runs goes on where it is.
    pub(crate) fn cycle_stage_machine(&mut self, project: &str, slug: &str, stage: Stage) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        if stage == Stage::Verify && crate::feature_checks::checks_running(feature) {
            return self.show_warning(
                "Its checks are running: pick their machine once they ended".to_owned(),
            );
        }
        if stage == Stage::ManualTest
            && let Some(run) = &feature.sandbox
        {
            let on = run.machine.as_deref().map_or("here".to_owned(), |id| {
                format!("on {}", self.machine_name(id))
            });
            return self.show_warning(format!(
                "Its sandbox is {on}, and its data with it: remove it to start one elsewhere"
            ));
        }
        if self.place_label(feature, stage).is_none() {
            let why = match self.takes_a_machine(feature, stage) {
                true => "Connect a machine first, from the launcher: Connect a machine\u{2026}",
                false => "T3 Code runs on this machine only",
            };
            return self.show_warning(why.to_owned());
        }
        let mut ids: Vec<(String, String)> = (self.machines.links.values())
            .filter_map(|link| link.work.as_ref())
            .map(|work| (self.machine_name(&work.id), work.id.clone()))
            .collect();
        ids.sort();
        let mut order = vec![HERE.to_owned()];
        order.extend(ids.into_iter().map(|(_, id)| id));
        let now = match self.placed(feature, stage) {
            _ if self.goes_to_least_busy(feature, stage) => LEAST_BUSY.to_owned(),
            Placed::Here => HERE.to_owned(),
            Placed::There(id) => id,
            Placed::Away(_) => String::new(),
        };
        if order.len() == 1 && now == HERE {
            return self.show_warning("No machine is connected right now".to_owned());
        }
        // With a machine to choose from, or to get away from that choice.
        if order.len() > 1 || now == LEAST_BUSY {
            order.push(LEAST_BUSY.to_owned());
        }
        let at = order.iter().position(|id| *id == now);
        let next = at.map_or(0, |i| (i + 1) % order.len());
        let next = order[next].clone();
        let name = self.machine_name(&next);
        let stays = (feature.runs.get(&stage))
            .filter(|r| r.live())
            .is_some_and(|r| r.machine.as_deref().unwrap_or(HERE) != next);
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        f.stage_machines.insert(stage, next.clone());
        self.save_feature(project, slug);
        if let Some(f) = self.feature(project, slug)
            && self.default_place(f, stage) == next
        {
            let f = self.feature_mut(project, slug);
            f.map(|f| f.stage_machines.remove(&stage));
            self.save_feature(project, slug);
        }
        if stays {
            let place = match next.as_str() {
                HERE => "here".to_owned(),
                LEAST_BUSY => "on the least busy".to_owned(),
                _ => format!("on {name}"),
            };
            self.show_warning(format!(
                "The agent already open stays where it is: the next one runs {place}"
            ));
        }
    }

    /// Where the project's file has `stage` run: `HERE`, a machine's id if it
    /// is up, or nothing known.
    fn default_place(&self, feature: &Feature, stage: Stage) -> String {
        let project = self.projects.project(&feature.project);
        let Some(name) = project.and_then(|p| p.workflow.machines.get(&stage)) else {
            return HERE.to_owned();
        };
        if name == LEAST_BUSY {
            return LEAST_BUSY.to_owned();
        }
        (self.machines.links.get(name))
            .and_then(|link| link.work.as_ref())
            .map_or_else(String::new, |work| work.id.clone())
    }

    /// What `feature`'s card and page say of where its current agent runs:
    /// the words, and whether they say all there is to say of it.
    pub(crate) fn machine_line(&self, feature: &Feature, stage: Stage) -> Option<(String, bool)> {
        if stage == Stage::Verify {
            let run = feature.verify.as_ref()?;
            let id = run.machine.as_deref()?;
            if !crate::feature_checks::checks_running(feature) {
                return None;
            }
            let name = self.machine_name(id);
            // Seen from elsewhere, what its writer says is all there is to ask.
            let reached = match feature.writer.as_deref() {
                Some(writer) => self.work_of(writer).is_some(),
                None => self.work_of(id).is_some_and(|w| w.panes.is_some()),
            };
            return Some(match reached {
                true => (format!("on {name}"), false),
                false => (format!("{name} can't be reached"), false),
            });
        }
        let run = feature.runs.get(&stage)?;
        let id = run.machine.as_deref()?;
        // Seen from elsewhere, what its writer says is all there is to ask.
        let name = self.machine_name(feature.writer.as_deref().unwrap_or(id));
        if self.starting_on_machine(&feature.project, &feature.slug, stage) {
            return Some((format!("starting on {name}\u{2026}"), true));
        }
        let pane = run.pane.as_deref()?;
        if self.pane_out_of_reach(pane) {
            return Some((format!("{name} can't be reached"), true));
        }
        // On the machine writing it: the feature's own line says that already.
        if feature.writer.as_deref() == Some(id) {
            return None;
        }
        Some((format!("on {}", self.machine_name(id)), false))
    }

    /// The data folder there and the feature's own, to go with a `StartJob`.
    pub(crate) fn machine_start_job(
        &self,
        id: &str,
        feature: &Feature,
        (stage, n, kind): (Stage, u32, StartKind),
        base: &str,
        command: &str,
    ) -> Option<StartJob> {
        let paths = self.machine_paths(id, Some(feature))?;
        let data = store::dir(None, &feature.project, &feature.slug);
        Some(StartJob {
            project: feature.project.clone(),
            slug: feature.slug.clone(),
            stage,
            n,
            kind,
            program: program(base).to_owned(),
            cwd: paths.there(&feature.dir),
            command: paths.text(command),
            data_there: paths.there(&data),
            data,
            paths,
            feature: feature.clone(),
        })
    }
}

/// `reading`, with the repositories in `dirty` as changed: those a feature
/// touches are its uncommitted changes, the others are strays.
pub(crate) fn with_dirt(feature: &Feature, reading: &mut Reading, dirty: &[String]) {
    for key in dirty {
        let touched = (feature.repos.iter()).any(|r| r.key() == *key && r.touched);
        let list = match touched {
            true => &mut reading.dirty,
            false => &mut reading.stray,
        };
        if !list.contains(key) {
            list.push(key.clone());
        }
    }
    reading.dirty.sort();
    reading.stray.sort();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::Repo;

    #[test]
    fn a_pane_there_names_its_machine() {
        assert_eq!(qualified("abc", "w1:p1"), "abc/w1:p1");
        assert_eq!(split_pane("abc/w1:p1"), Some(("abc", "w1:p1")));
        assert_eq!(split_pane("w1:p1"), None);
    }

    #[test]
    fn changes_left_there_count_as_the_ones_here_do() {
        let mut f = crate::feature::tests::feature("x", 0);
        let repo = |name: &str, touched| Repo {
            name: name.into(),
            main: "/r".into(),
            base: "main".into(),
            base_commit: "c".into(),
            worktree: "/w".into(),
            touched,
            branch: None,
            project: None,
        };
        f.repos = vec![repo("api", true), repo("web", false)];
        let mut reading = Reading {
            dirty: vec!["api".into()],
            ..Reading::default()
        };
        with_dirt(&f, &mut reading, &["api".to_owned(), "web".to_owned()]);
        assert_eq!(reading.dirty, ["api"]);
        assert_eq!(reading.stray, ["web"]);
    }

    fn listed(status: AgentStatus) -> HashMap<String, Listed> {
        let pane = Listed {
            terminal: "t".into(),
            status: Some(status),
        };
        [("p".to_owned(), pane)].into()
    }

    #[test]
    fn an_agent_at_rest_is_done_only_once_its_work_is_here() {
        let mut work = work();
        let key = ("p".to_owned(), "s".to_owned());
        let start = Instant::now();
        work.listed(Some(listed(AgentStatus::Working)), start);
        assert!(!work.settling("p", &key, start));
        let rest = start + Duration::from_secs(4);
        work.listed(Some(listed(AgentStatus::Done)), rest);
        assert!(work.settling("p", &key, rest));
        assert!(work.shared.pull_now.load(Ordering::Relaxed));
        // A pull that began before it rested may have missed its last commit.
        work.pulled.insert(key.clone(), start);
        assert!(work.settling("p", &key, rest));
        work.pulled.insert(key.clone(), rest);
        assert!(!work.settling("p", &key, rest));
        // Not coming back, it doesn't look at work for ever.
        work.pulled.clear();
        assert!(!work.settling("p", &key, rest + SETTLE_MOST));
        // At work again, it isn't at rest.
        work.listed(Some(listed(AgentStatus::Working)), rest);
        assert!(!work.settling("p", &key, rest));
    }

    #[test]
    fn a_list_that_failed_says_nothing_of_who_rests() {
        let mut work = work();
        let key = ("p".to_owned(), "s".to_owned());
        let now = Instant::now();
        work.listed(Some(listed(AgentStatus::Working)), now);
        work.listed(None, now);
        work.listed(Some(listed(AgentStatus::Done)), now);
        assert!(!work.settling("p", &key, now));
    }

    fn work() -> Work {
        let (jobs, _rx) = mpsc::channel();
        Work {
            views: Arc::default(),
            load: None,
            id: "m".into(),
            target: "ana@box".into(),
            info: LinkInfo {
                site: crate::machines::Site {
                    config: "/home/ana/.config".into(),
                    herdr: Some("/home/ana/.local/bin/herdr".into()),
                    ..Default::default()
                },
                socket: "/run/s.sock".into(),
                control: "/run/s.ctl".into(),
            },
            panes: None,
            jobs,
            stop: Arc::default(),
            shared: Arc::default(),
            pulled: HashMap::new(),
            dirty: HashMap::new(),
            rested: HashMap::new(),
        }
    }

    #[test]
    fn a_look_at_a_writers_terminal_takes_nothing_from_it() {
        let work = work();
        let line = work.observe_line("t1", "gerdr-app", Some(78), "box");
        assert!(line.contains("terminal session observe"), "{line}");
        assert!(!line.contains("--takeover"), "{line}");
        assert!(line.contains("sessions/gerdr-app/herdr.sock"), "{line}");
        assert!(line.contains("--rows 78"), "{line}");
        assert!(line.contains("ssh -n -S "), "{line}");
        assert!(line.contains("It only looked"), "{line}");
        // It reads as a shell line, and the terminal is put back after.
        let check = std::process::Command::new("sh")
            .args(["-n", "-c", &line])
            .output()
            .unwrap();
        assert!(check.status.success(), "{check:?}");
        assert!(line.contains(r"\033[?7l"), "{line}");
        assert!(line.contains(r"\033[?7h"), "{line}");
    }

    #[test]
    fn a_view_says_ending_it_stops_nothing() {
        let work = work();
        let line = work.attach_line("t1", "box");
        assert!(line.starts_with("ssh -t -S "), "{line}");
        assert!(line.contains("ana@box"), "{line}");
        assert!(line.contains("terminal attach"), "{line}");
        assert!(line.contains("--takeover"), "{line}");
        assert!(line.contains("gerdr-worker/herdr.sock"), "{line}");
        assert!(line.contains("keeps running on box"), "{line}");
    }

    #[test]
    fn a_view_taken_by_another_machine_says_which() {
        let views = std::env::temp_dir().join(format!("gerdr-views-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&views);
        let views = views.to_string_lossy().into_owned();
        let run = |attach: &str| {
            let script = viewed_by(&views, "w1/t1", "my box", attach);
            let out = std::process::Command::new("sh")
                .args(["-c", &script])
                .output()
                .unwrap();
            (
                out.status.code(),
                String::from_utf8_lossy(&out.stdout).into_owned(),
            )
        };
        // Ended here, by closing: nothing to say, nor left there.
        assert_eq!(run("true"), (Some(0), String::new()));
        assert!(!std::path::Path::new(&format!("{views}/w1_t1")).exists());
        // Another machine opened it meanwhile, which ends this one.
        let taken = format!("printf %s desk > '{views}/w1_t1'; false");
        assert_eq!(run(&taken), (Some(1), "\nOpened on desk.\n".to_owned()));
        let _ = std::fs::remove_dir_all(&views);
    }
}

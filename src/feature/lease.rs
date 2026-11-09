//! Which gerdr writes a feature, and how that moves.
//!
//! Each feature has one writer (see `view`). A press on another machine's feature
//! moves it here: the writer stops writing and leaves its record for the new one,
//! whose `Feature::epoch` is one higher. All of it goes through files in the
//! feature's folder on the old writer, as no gerdr listens on the network:
//!
//! - `REQUEST` asks a gerdr open there to let go, and `ANSWER` is what it says.
//!   With none open (`LOCK` is free) there is nobody to ask.
//! - `HANDED` is the record, out of the way of that machine's own reading, until
//!   the new writer has all of it and removes the file. If it never does, the
//!   record goes back (`RESTORE_AFTER`): a feature is never left with no writer.
//! - `HANDED_TO` stays, saying where it went: a third gerdr looking there tells a
//!   feature that moved from one that is over.
//!
//! With the writer out of reach a feature can be taken from the copy kept here. It
//! may have gone on there, so once both can be seen the higher epoch wins
//! (`rival`), and the loser's record is kept under `ASIDE`.
//!
//! What runs in the old writer's own session stays in its tabs: nothing else can
//! reach those panes (`taken` forgets them). What runs on a worker goes on, and
//! the new writer follows it.
//!
//! A sandbox running on the old writer goes on there too, when the feature moves
//! over a link: the new writer follows it as one on a worker. Its terminals stay
//! in the old writer's tabs, which that gerdr goes on telling of (`KEPT`), so the
//! new writer shows them as a viewer shows a writer's own.

use std::{path::Path, time::Duration};

use serde::{Deserialize, Serialize};

use super::{
    Feature,
    remote::Paths,
    setup::SetupRun,
    store, view,
};
use crate::{desktop::shell_quote, machine_work::HERE};

/// In a feature's folder there: the request to let go.
pub const REQUEST: &str = ".take.json";
/// In a feature's folder there: what a gerdr open there says to it.
pub const ANSWER: &str = ".take-answer.json";
/// In a feature's folder there: the record, while it moves.
pub const HANDED: &str = "feature.json.handed";
/// In a feature's folder there: the machine it moved to.
pub const HANDED_TO: &str = ".handed-to";
/// What starts the name of a record that lost, kept beside the others. What
/// was kept beside it (`SIDES`) goes aside with it, named by `aside_name`:
/// the winner's, coming later, has the names to itself.
pub const ASIDE: &str = "feature.json.aside-";
/// The files kept beside a record: its history and its review rounds.
pub const SIDES: [&str; 2] = [store::HISTORY, store::ROUNDS];

/// The name file `file` of a record that lost at `at` is kept under.
pub fn aside_name(file: &str, at: u64) -> String {
    format!("{file}.aside-{at}")
}
/// In a feature's folder there: what another machine asks of its sandbox.
pub const SANDBOX_ASK: &str = ".sandbox-ask.json";
/// In a feature's folder there: why a gerdr open there didn't do what was asked.
pub const SANDBOX_ANSWER: &str = ".sandbox-answer.json";
/// In a feature's folder there: what another machine did to its sandbox with no
/// gerdr open there, for its record to catch up with when one opens.
pub const SANDBOX_TOLD: &str = ".sandbox-told.json";
/// In a sandbox's folder, on the machine it runs on: it went on running there
/// when its feature moved to another writer, and this gerdr's own tabs run its
/// commands.
pub const KEPT: &str = "kept.json";

/// A sandbox that went on running here when its feature moved (see `KEPT`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kept {
    pub project: String,
    pub slug: String,
    /// When it started: one started since is another.
    pub started: u64,
    /// What keeps its network: once that is gone, so is the sandbox.
    pub holder: Option<super::sandbox_run::Proc>,
    /// The panes of this gerdr's own session running its commands.
    pub panes: Vec<String>,
    /// It was open to the network here when its feature left, which closed
    /// that: it opens again when the feature comes back with it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub lan: bool,
}

/// What `KEPT` says of `feature`'s sandbox, written by the machine it runs on,
/// if that one runs: what goes on there when the feature moves (`taken`).
pub fn kept_of(feature: &Feature) -> Option<Kept> {
    let run = feature.sandbox.as_ref()?;
    (run.machine.is_none() && !run.stopped && run.failed.is_none()).then(|| Kept {
        project: feature.project.clone(),
        slug: feature.slug.clone(),
        started: run.started,
        holder: run.holder.clone(),
        panes: run.panes.clone(),
        lan: false,
    })
}

/// The script saying `KEPT` (`json`) in sandbox folder `dir` there, for a gerdr
/// that isn't open there now and finds it at its next start.
pub fn kept_script(dir: &str, json: &str) -> String {
    format!(
        "mkdir -p {dir} && printf '%s' {json} > {dir}/{KEPT}.tmp && mv {dir}/{KEPT}.tmp {dir}/{KEPT}\n",
        dir = shell_quote(dir),
        json = shell_quote(json),
    )
}

/// What `KEPT` in sandbox folder `dir` says, if it is there and reads.
pub fn read_kept(dir: &Path) -> Option<Kept> {
    let raw = match std::fs::read(dir.join(KEPT)) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            tracing::warn!("reading {}: {err}", dir.join(KEPT).display());
            return None;
        }
    };
    serde_json::from_slice(&raw)
        .inspect_err(|err| tracing::warn!("reading {}: {err}", dir.join(KEPT).display()))
        .ok()
}

/// Say in sandbox folder `dir` that `kept` goes on running here.
pub fn write_kept(dir: &Path, kept: &Kept) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(kept).map_err(std::io::Error::other)?;
    crate::projects::write_atomic(&dir.join(KEPT), &bytes)
}

/// The sandbox in folder `dir` no longer runs here for another writer.
pub fn forget_kept(dir: &Path) {
    if let Err(err) = std::fs::remove_file(dir.join(KEPT))
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("removing {}: {err}", dir.join(KEPT).display());
    }
}

/// In a feature's folder on the machine it was taken over from: the machine
/// writing it now asks that one's gerdr to take it back.
pub const HAND_BACK: &str = ".hand-back.json";
/// Beside `HAND_BACK`: what the gerdr there says to it.
pub const HAND_BACK_ANSWER: &str = ".hand-back-answer.json";

/// A machine writing a feature asks the one it took it from to take it back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandBack {
    /// The machine that writes it now, by id.
    pub machine: String,
    /// What that machine calls itself.
    pub name: String,
    /// The record's epoch as it asks: one moved since is another matter.
    pub epoch: u64,
}

/// What a gerdr says to a `HandBack`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandBackAnswer {
    /// Why it doesn't take it; none: it takes it now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
}

/// The script asking the gerdr there to take a feature back: `json`, a
/// `HandBack`. An answer to an earlier one goes first.
pub fn hand_back_script(dir: &str, json: &str) -> String {
    format!(
        "cd {} || exit 3\nrm -f {HAND_BACK_ANSWER}\nprintf '%s' {} > {HAND_BACK}.tmp && mv {HAND_BACK}.tmp {HAND_BACK}\n",
        shell_quote(dir),
        shell_quote(json)
    )
}

/// The script saying `WAITS` while a hand back isn't taken up there yet.
pub fn hand_back_waits_script(dir: &str) -> String {
    format!(
        "cd {} || exit 3\nif [ -e {HAND_BACK} ]; then echo WAITS; fi\n",
        shell_quote(dir)
    )
}

/// The script taking a hand back back, nobody having taken it up.
pub fn hand_back_unask_script(dir: &str) -> String {
    format!("cd {} || exit 0\nrm -f {HAND_BACK}\nexit 0\n", shell_quote(dir))
}

/// How long a gerdr open on the writer gets to answer.
pub const ASK_FOR: Duration = Duration::from_secs(12);
/// How long a record may wait for its new writer before it goes back.
pub const RESTORE_AFTER: Duration = Duration::from_secs(60);
/// How old a request can be before nobody waits for its answer any more.
pub const REQUEST_STANDS: Duration = Duration::from_secs(30);

/// The file a gerdr holds a shared lock on while it shows `project`, under its
/// features folder: found from another machine by the data folder alone, where
/// the runtime folder isn't known.
pub fn lock_dir(data: &Path) -> std::path::PathBuf {
    data.join("features")
}

/// A request to let go of a feature.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// The machine that takes it, by id.
    pub machine: String,
    /// What that machine calls itself, for one that doesn't know it.
    pub name: String,
    /// The epoch its record will have.
    pub epoch: u64,
    /// Taken already: the one asked lost, and nothing it says changes that.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub forced: bool,
}

/// Another machine asks a gerdr open on the writer to do this to a sandbox.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxAsk {
    pub what: crate::feature_sandbox::StopFor,
    /// What that machine calls itself.
    pub name: String,
    /// When the sandbox it is about started: a later one is another.
    #[serde(default)]
    pub started: u64,
}

/// A gerdr open on the writer won't do what was asked of a sandbox.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxRefusal {
    pub why: String,
}

/// What a gerdr open on the writer says to a `Request`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub epoch: u64,
    /// Why it keeps the feature; none: it let go.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
}

/// How a record here stands against one of another machine with the same slug.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rival {
    /// Two features that happen to share a name.
    Clash,
    /// The same one, and this machine's is the older: it goes.
    TheyWin,
    /// The same one, and the other is the older.
    IWin,
}

/// How `mine` (written by machine `me`) stands against `theirs` (by `them`). Both
/// having moved the same number of times, one of them still has to win, the same
/// one everywhere.
pub fn rival(mine: &Feature, me: &str, theirs: &Feature, them: &str) -> Rival {
    if mine.created != theirs.created || (mine.epoch == 0 && theirs.epoch == 0) {
        return Rival::Clash;
    }
    if (theirs.epoch, them) > (mine.epoch, me) {
        Rival::TheyWin
    } else {
        Rival::IWin
    }
}

/// `record`, as written by machine `from` until now, as machine `me` writes it:
/// its paths are this machine's, and its epoch one higher.
///
/// What ran in `from`'s own session, or on this machine's worker session, can't
/// be followed from here: a pane is known only on the worker sessions of machines
/// linked to. Its run stays, and a chat goes back on the machine it ran on.
///
/// A sandbox running on `from` goes on there with `keep_sandbox`, as one on a
/// worker: `from` tells of its tabs. Without, it stays behind.
pub fn taken(
    record: &Feature,
    paths: &Paths,
    from: &str,
    from_name: &str,
    me: &str,
    keep_sandbox: bool,
    now: u64,
) -> Result<Feature, String> {
    let mut value = serde_json::to_value(record).map_err(|err| err.to_string())?;
    view::translate(&mut value, paths);
    let mut out: Feature = serde_json::from_value(value).map_err(|err| err.to_string())?;
    let elsewhere = |pane: &str| {
        crate::machine_work::split_pane(pane).is_some_and(|(id, _)| id != me)
    };
    let cut = |pane: &mut Option<String>| {
        if pane.as_deref().is_some_and(|p| !elsewhere(p)) {
            *pane = None;
        }
    };
    // Its place, for what ran here until now: the writer's, unless that's me.
    let moved = |machine: &mut Option<String>| {
        *machine = match machine.take() {
            None => Some(from.to_owned()),
            Some(id) if id == me => None,
            other => other,
        };
    };
    for run in out.runs.values_mut() {
        cut(&mut run.pane);
        run.t3 = None;
        // Nothing to go back to: it stays out of the way.
        if run.machine.is_some() || !run.base.is_empty() {
            moved(&mut run.machine);
        }
    }
    if let Some(verify) = &mut out.verify {
        moved(&mut verify.machine);
        for check in &mut verify.checks {
            cut(&mut check.pane);
        }
    }
    let setup = |run: &mut SetupRun| {
        moved(&mut run.machine);
        cut(&mut run.pane);
    };
    out.setup.iter_mut().for_each(setup);
    out.setups_elsewhere.iter_mut().for_each(setup);
    // The latest of a place counts. Two can name one now: this machine's, kept
    // from when another wrote it, and `from`'s.
    let latest = out.setup.as_ref().map(|s| s.machine.clone());
    out.setups_elsewhere
        .retain(|s| latest.as_ref() != Some(&s.machine));
    // A sandbox on its old writer stays there, and goes on being followed only
    // when asked. One on a worker goes on running: its record comes along, and
    // so does what it left there.
    let mut log = format!("Taken over from {from_name}");
    out.sandbox = out.sandbox.take().and_then(|mut run| {
        run.elsewhere = None;
        let Some(on) = run.machine.take() else {
            if !keep_sandbox || run.stopped || run.failed.is_some() {
                return None;
            }
            for pane in run.panes.iter_mut().filter(|p| !p.contains('/')) {
                *pane = crate::machine_work::qualified(from, pane);
            }
            run.machine = Some(from.to_owned());
            run.kept = true;
            log = format!("Taken over from {from_name}, its sandbox still running there");
            return Some(run);
        };
        if on == me {
            // On this machine's worker session: not one of this gerdr's panes.
            // One kept here has its tabs back from what was kept of them.
            run.panes.clear();
            run.kept = false;
        } else {
            run.machine = Some(on);
        }
        Some(run)
    });
    // Picks name machines as `from` saw them: its "here" is `from` now.
    for machine in out.stage_machines.values_mut() {
        if machine == me {
            *machine = HERE.to_owned();
        } else if machine == HERE {
            *machine = from.to_owned();
        }
    }
    out.writer = None;
    out.epoch = record.epoch + 1;
    out.came_from = Some(from.to_owned());
    out.log(now, log);
    Ok(out)
}

/// The script reading a feature's record there; nothing with none.
pub fn record_script(dir: &str, file: &str) -> String {
    format!("cat {}/{file} 2>/dev/null\nexit 0\n", shell_quote(dir))
}

/// The script saying whether a gerdr shows `project` there: `HELD`, `FREE`, or
/// `UNKNOWN` with no way to tell.
pub fn open_script(data: &str, project: &str) -> String {
    let lock = format!("{data}/features/{project}.lock");
    format!(
        r#"f={}
[ -e "$f" ] || {{ echo FREE; exit 0; }}
command -v flock >/dev/null 2>&1 || {{ echo UNKNOWN; exit 0; }}
if flock -n -x "$f" true; then echo FREE; else echo HELD; fi
"#,
        shell_quote(&lock)
    )
}

/// The script asking a gerdr open there to let go: `request`, as JSON. An answer
/// to an earlier one goes first.
pub fn ask_script(dir: &str, request: &str) -> String {
    format!(
        "cd {} || exit 3\nrm -f {ANSWER}\nprintf '%s' {} > {REQUEST}.tmp && mv {REQUEST}.tmp {REQUEST}\n",
        shell_quote(dir),
        shell_quote(request)
    )
}

/// The script asking a gerdr open there to do `ask`, as JSON, to a sandbox. An
/// answer to an earlier one goes first.
pub fn sandbox_ask_script(dir: &str, ask: &str) -> String {
    format!(
        "cd {} || exit 3\nrm -f {SANDBOX_ANSWER}\nprintf '%s' {} > {SANDBOX_ASK}.tmp && mv {SANDBOX_ASK}.tmp {SANDBOX_ASK}\n",
        shell_quote(dir),
        shell_quote(ask)
    )
}

/// The script leaving `ask`, as JSON, for a gerdr that opens there later.
pub fn sandbox_told_script(dir: &str, ask: &str) -> String {
    format!(
        "cd {} || exit 3\nprintf '%s' {} > {SANDBOX_TOLD}.tmp && mv {SANDBOX_TOLD}.tmp {SANDBOX_TOLD}\n",
        shell_quote(dir),
        shell_quote(ask)
    )
}

/// The script saying `WAITS` while what was asked of a sandbox isn't taken yet.
pub fn sandbox_waits_script(dir: &str) -> String {
    format!(
        "cd {} || exit 3\nif [ -e {SANDBOX_ASK} ]; then echo WAITS; fi\n",
        shell_quote(dir)
    )
}

/// The script taking back what was asked of a sandbox.
pub fn sandbox_unask_script(dir: &str) -> String {
    format!("cd {} || exit 3\nrm -f {SANDBOX_ASK}\n", shell_quote(dir))
}

/// The script taking the record out of the way there, with no gerdr to ask. Says
/// `GONE` when it had none.
pub fn yield_script(dir: &str, to: &str) -> String {
    format!(
        r#"cd {} || exit 3
[ -e {file} ] || {{ echo GONE; exit 0; }}
printf '%s' {} > {HANDED_TO}
mv {file} {HANDED} && touch {HANDED} && echo YIELDED
"#,
        shell_quote(dir),
        shell_quote(to),
        file = store::FILE,
    )
}

/// The script putting the record back there, as this machine won't take it after
/// all. What's there by now stays.
pub fn undo_script(dir: &str) -> String {
    format!(
        r#"cd {} || exit 0
if [ -e {HANDED} ] && [ ! -e {file} ]; then mv {HANDED} {file}; rm -f {HANDED_TO}; fi
rm -f {REQUEST} {ANSWER}
exit 0
"#,
        shell_quote(dir),
        file = store::FILE,
    )
}

/// The script clearing up there once the new writer has it all.
pub fn done_script(dir: &str) -> String {
    format!(
        "cd {} || exit 0\nrm -f {HANDED} {REQUEST} {ANSWER}\nexit 0\n",
        shell_quote(dir)
    )
}

/// The script setting a record aside that lost, with no gerdr open there.
pub fn aside_script(dir: &str, to: &str, at: u64) -> String {
    format!(
        r#"cd {} || exit 0
[ -e {file} ] && mv {file} {ASIDE}{at}
{sides}rm -f {HANDED}
printf '%s' {} > {HANDED_TO}
exit 0
"#,
        shell_quote(dir),
        shell_quote(to),
        file = store::FILE,
        sides = SIDES
            .iter()
            .map(|side| format!("[ -e {side} ] && mv {side} {}\n", aside_name(side, at)))
            .collect::<String>(),
    )
}

/// The script putting a worktree here on its branch, at what came (`head`, a ref).
/// A branch of that name that went another way is kept under another name, and one
/// ahead of what came is left as it is: it holds what this machine did for the
/// feature as its worker. Changes in the way end it, saying `INWAY <folder>`.
pub fn branch_script(main: &Path, worktree: &Path, branch: &str, head: &str, at: u64) -> String {
    let main = shell_quote(&main.to_string_lossy());
    let worktree = shell_quote(&worktree.to_string_lossy());
    let branch = shell_quote(branch);
    let head = shell_quote(head);
    format!(
        r#"m={main}; w={worktree}; b={branch}; r={head}
t=$(git -C "$m" rev-parse -q --verify "$r") || exit 7
if c=$(git -C "$m" rev-parse -q --verify "refs/heads/$b"); then
  if git -C "$m" merge-base --is-ancestor "$t" "$c"; then
    git -C "$w" checkout -q "$b" || {{ echo "INWAY $w"; exit 6; }}
    exit 0
  fi
  if ! git -C "$m" merge-base --is-ancestor "$c" "$t"; then
    git -C "$m" branch -m "$b" "$b-aside-{at}" || exit 7
  fi
fi
git -C "$w" checkout -q -B "$b" "$t" || {{ echo "INWAY $w"; exit 6; }}
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::{
        Stage,
        agents::{StageRun, T3Run},
        remote::Side,
        sandbox_run::SandboxRun,
        setup::SetupRun,
        tests::{Temp, feature},
        verify::{CheckRun, VerifyRun},
    };
    use std::collections::BTreeMap;

    fn paths() -> Paths {
        Paths::new(
            &Side {
                home: "/home/ana".into(),
                data: "/home/ana/.local/share/gerdr".into(),
            },
            &Side {
                home: "/home/lg".into(),
                data: "/home/lg/.local/share/gerdr".into(),
            },
            &BTreeMap::new(),
            None,
        )
    }

    fn record() -> Feature {
        let mut f = feature("a", 5);
        f.root = "/home/ana/app".into();
        f.dir = "/home/ana/app/.features/a".into();
        f.epoch = 2;
        f
    }

    fn run(pane: Option<&str>, machine: Option<&str>) -> StageRun {
        StageRun {
            pane: pane.map(str::to_owned),
            machine: machine.map(str::to_owned),
            base: "claude".into(),
            prompt: "/home/ana/.local/share/gerdr/features/p/a/prompt.md".into(),
            ..StageRun::default()
        }
    }

    fn take(f: &Feature) -> Feature {
        taken(f, &paths(), "ana", "box", "me", false, 100).unwrap()
    }

    fn take_keeping(f: &Feature) -> Feature {
        taken(f, &paths(), "ana", "box", "me", true, 100).unwrap()
    }

    fn sandbox(panes: &[&str]) -> SandboxRun {
        SandboxRun {
            started: 7,
            finished: Some(8),
            panes: panes.iter().map(|p| (*p).to_owned()).collect(),
            ..SandboxRun::default()
        }
    }

    #[test]
    fn a_taken_feature_knows_where_it_came_from() {
        let f = record();
        assert_eq!(f.came_from, None);
        assert_eq!(take(&f).came_from.as_deref(), Some("ana"));
    }

    #[test]
    fn a_sandbox_running_on_its_old_writer_goes_on_there_when_kept() {
        let mut f = record();
        f.sandbox = Some(sandbox(&["w1:p4"]));
        let out = take_keeping(&f);
        let run = out.sandbox.as_ref().unwrap();
        assert_eq!(run.machine.as_deref(), Some("ana"));
        assert_eq!(run.panes, ["ana/w1:p4"]);
        assert!(run.kept);
        assert_eq!(run.elsewhere, None);
        let said = &out.history.last().unwrap().what;
        assert_eq!(said, "Taken over from box, its sandbox still running there");
    }

    #[test]
    fn a_sandbox_on_its_old_writer_stays_behind_unless_kept_and_running() {
        let mut f = record();
        f.sandbox = Some(sandbox(&["w1:p4"]));
        assert_eq!(take(&f).sandbox, None);
        let mut stopped = sandbox(&[]);
        stopped.stopped = true;
        f.sandbox = Some(stopped);
        assert_eq!(take_keeping(&f).sandbox, None);
        let mut failed = sandbox(&[]);
        failed.failed = Some("no".into());
        f.sandbox = Some(failed);
        assert_eq!(take_keeping(&f).sandbox, None);
    }

    #[test]
    fn a_sandbox_on_a_worker_goes_on_being_followed_either_way() {
        let mut f = record();
        let mut there = sandbox(&["w9/w1:p4"]);
        there.machine = Some("w9".into());
        f.sandbox = Some(there);
        for out in [take(&f), take_keeping(&f)] {
            let run = out.sandbox.as_ref().unwrap();
            assert_eq!(run.machine.as_deref(), Some("w9"));
            assert_eq!(run.panes, ["w9/w1:p4"]);
            assert!(!run.kept);
        }
    }

    #[test]
    fn a_sandbox_kept_on_this_machine_comes_back_as_its_own() {
        let mut f = record();
        let mut here = sandbox(&["me/w1:p4"]);
        here.machine = Some("me".into());
        here.kept = true;
        f.sandbox = Some(here);
        let out = take_keeping(&f);
        let run = out.sandbox.as_ref().unwrap();
        assert_eq!(run.machine, None);
        assert!(!run.kept);
        // Its tabs come from what was kept of them here.
        assert!(run.panes.is_empty());
    }

    #[test]
    fn only_a_sandbox_running_on_the_writer_is_kept() {
        let mut f = record();
        assert_eq!(kept_of(&f), None);
        f.sandbox = Some(sandbox(&["w1:p4"]));
        let kept = kept_of(&f).unwrap();
        assert_eq!((kept.started, kept.panes.as_slice()), (7, &["w1:p4".to_owned()][..]));
        f.sandbox.as_mut().unwrap().machine = Some("w9".into());
        assert_eq!(kept_of(&f), None);
        let mut stopped = sandbox(&[]);
        stopped.stopped = true;
        f.sandbox = Some(stopped);
        assert_eq!(kept_of(&f), None);
    }

    #[test]
    fn the_kept_script_leaves_what_reads_back() {
        let t = Temp::new();
        let dir = t.0.join("it's/sandbox");
        let kept = Kept {
            project: "p".into(),
            slug: "a".into(),
            started: 7,
            holder: None,
            panes: vec!["w1:p4".into()],
            lan: false,
        };
        let json = serde_json::to_string(&kept).unwrap();
        let script = kept_script(&dir.to_string_lossy(), &json);
        let ran = std::process::Command::new("sh").arg("-c").arg(&script).status().unwrap();
        assert!(ran.success());
        assert_eq!(read_kept(&dir), Some(kept));
    }

    #[test]
    fn what_is_kept_of_a_sandbox_reads_back() {
        let t = Temp::new();
        let dir = t.0.as_path();
        assert_eq!(read_kept(dir), None);
        let kept = Kept {
            project: "p".into(),
            slug: "a".into(),
            started: 7,
            holder: None,
            panes: vec!["w1:p4".into()],
            lan: true,
        };
        write_kept(dir, &kept).unwrap();
        assert_eq!(read_kept(dir), Some(kept));
        forget_kept(dir);
        assert_eq!(read_kept(dir), None);
        // Already gone: nothing to say.
        forget_kept(dir);
    }

    #[test]
    fn a_taken_record_has_this_machines_paths_and_the_next_epoch() {
        let out = take(&record());
        assert_eq!(out.root, Path::new("/home/lg/app"));
        assert_eq!(out.dir, Path::new("/home/lg/app/.features/a"));
        assert_eq!(out.epoch, 3);
        assert_eq!(out.writer, None);
        let said = &out.history.last().unwrap().what;
        assert_eq!(said, "Taken over from box");
    }

    #[test]
    fn a_taken_record_reads_from_here_not_from_a_copy() {
        let mut f = record();
        f.runs.insert(Stage::Plan, run(None, None));
        let out = take(&f);
        assert_eq!(
            out.runs[&Stage::Plan].prompt,
            Path::new("/home/lg/.local/share/gerdr/features/p/a/prompt.md")
        );
    }

    #[test]
    fn a_pane_only_its_old_writer_can_reach_is_forgotten_and_its_chat_stays_there() {
        let mut f = record();
        f.runs.insert(Stage::Plan, run(Some("w1:p1"), None));
        let out = take(&f);
        let plan = &out.runs[&Stage::Plan];
        assert_eq!(plan.pane, None);
        assert_eq!(plan.machine.as_deref(), Some("ana"));
    }

    #[test]
    fn a_run_on_a_worker_goes_on_being_followed() {
        let mut f = record();
        f.runs
            .insert(Stage::Plan, run(Some("w9/w1:p1"), Some("w9")));
        let out = take(&f);
        let plan = &out.runs[&Stage::Plan];
        assert_eq!(plan.pane.as_deref(), Some("w9/w1:p1"));
        assert_eq!(plan.machine.as_deref(), Some("w9"));
    }

    #[test]
    fn a_run_in_the_old_writers_worker_session_goes_on_as_a_workers() {
        let mut f = record();
        f.runs
            .insert(Stage::Plan, run(Some("ana/w1:p1"), Some("ana")));
        let out = take(&f);
        assert_eq!(out.runs[&Stage::Plan].pane.as_deref(), Some("ana/w1:p1"));
    }

    #[test]
    fn what_this_machine_did_as_a_worker_is_its_own_now() {
        let mut f = record();
        f.runs.insert(Stage::Plan, run(Some("me/w1:p1"), Some("me")));
        f.stage_machines.insert(Stage::Build, "me".into());
        f.stage_machines.insert(Stage::Plan, "w9".into());
        f.stage_machines.insert(Stage::Verify, HERE.into());
        let out = take(&f);
        let plan = &out.runs[&Stage::Plan];
        assert_eq!(plan.pane, None);
        assert_eq!(plan.machine, None);
        assert_eq!(out.stage_machines[&Stage::Build], HERE);
        assert_eq!(out.stage_machines[&Stage::Plan], "w9");
        assert_eq!(out.stage_machines[&Stage::Verify], "ana");
    }

    #[test]
    fn a_run_with_no_chat_doesnt_claim_a_machine() {
        let mut f = record();
        let mut plain = run(None, None);
        plain.base.clear();
        f.runs.insert(Stage::Plan, plain);
        assert_eq!(take(&f).runs[&Stage::Plan].machine, None);
    }

    #[test]
    fn an_agent_in_t3_code_stays_there() {
        let mut f = record();
        let mut there = run(None, None);
        there.t3 = Some(T3Run::Thread {
            id: "t".into(),
            agent: None,
        });
        f.runs.insert(Stage::Plan, there);
        assert!(!take(&f).runs[&Stage::Plan].live());
    }

    #[test]
    fn checks_on_a_worker_stay_there_and_those_of_the_old_writer_are_cut_off() {
        let mut f = record();
        f.verify = Some(VerifyRun {
            machine: Some("w9".into()),
            checks: vec![CheckRun {
                pane: Some("w9/w1:p1".into()),
                ..CheckRun::default()
            }],
            ..VerifyRun::default()
        });
        let out = take(&f);
        let verify = out.verify.as_ref().unwrap();
        assert_eq!(verify.machine.as_deref(), Some("w9"));
        assert_eq!(verify.checks[0].pane.as_deref(), Some("w9/w1:p1"));

        let mut f = record();
        f.verify = Some(VerifyRun {
            checks: vec![CheckRun {
                pane: Some("w1:p1".into()),
                ..CheckRun::default()
            }],
            ..VerifyRun::default()
        });
        let out = take(&f);
        let verify = out.verify.as_ref().unwrap();
        assert_eq!(verify.machine.as_deref(), Some("ana"));
        assert_eq!(verify.checks[0].pane, None);
    }

    #[test]
    fn this_machine_needs_its_own_setup_and_keeps_one_it_had() {
        let mut f = record();
        f.setup = Some(SetupRun {
            started: 5,
            ..SetupRun::default()
        });
        let out = take(&f);
        assert_eq!(out.setup_at(None), None);
        assert_eq!(out.setup_at(Some("ana")).unwrap().started, 5);

        let mut f = record();
        f.setup = Some(SetupRun {
            started: 5,
            ..SetupRun::default()
        });
        f.setups_elsewhere = vec![SetupRun {
            machine: Some("me".into()),
            started: 4,
            ..SetupRun::default()
        }];
        let out = take(&f);
        assert_eq!(out.setup_at(None).unwrap().started, 4);
    }

    #[test]
    fn a_sandbox_stays_where_it_was_started() {
        let mut f = record();
        f.sandbox = Some(Default::default());
        assert!(take(&f).sandbox.is_none());
    }

    #[test]
    fn a_sandbox_on_a_worker_is_taken_along() {
        let mut f = record();
        f.sandbox = Some(crate::feature::sandbox_run::SandboxRun {
            machine: Some("w9".into()),
            panes: vec!["w9/w1:p1".into()],
            ..Default::default()
        });
        let run = take(&f).sandbox.unwrap();
        assert_eq!(run.machine.as_deref(), Some("w9"));
        assert_eq!(run.panes, ["w9/w1:p1"]);
    }

    #[test]
    fn a_sandbox_on_this_machine_is_its_own_now() {
        let mut f = record();
        f.sandbox = Some(crate::feature::sandbox_run::SandboxRun {
            machine: Some("me".into()),
            panes: vec!["me/w1:p1".into()],
            ..Default::default()
        });
        let run = take(&f).sandbox.unwrap();
        assert_eq!(run.machine, None);
        assert!(run.panes.is_empty());
    }

    #[test]
    fn the_higher_epoch_wins_and_a_tie_goes_the_same_way_everywhere() {
        let (mut a, mut b) = (record(), record());
        a.epoch = 3;
        b.epoch = 2;
        assert_eq!(rival(&a, "x", &b, "y"), Rival::IWin);
        assert_eq!(rival(&b, "y", &a, "x"), Rival::TheyWin);
        b.epoch = 3;
        assert_eq!(rival(&a, "x", &b, "y"), Rival::TheyWin);
        assert_eq!(rival(&b, "y", &a, "x"), Rival::IWin);
    }

    #[test]
    fn features_that_never_moved_or_differ_only_share_a_name() {
        let (mut a, mut b) = (record(), record());
        a.epoch = 0;
        b.epoch = 0;
        assert_eq!(rival(&a, "x", &b, "y"), Rival::Clash);
        b.epoch = 9;
        b.created = a.created + 1;
        assert_eq!(rival(&a, "x", &b, "y"), Rival::Clash);
    }

    fn sh(script: &str) -> String {
        let out = std::process::Command::new("sh")
            .args(["-c", script])
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    #[test]
    fn a_record_moves_out_of_the_way_and_back() {
        let t = Temp::new();
        let dir = t.0.to_string_lossy().into_owned();
        std::fs::write(t.0.join(store::FILE), "{}").unwrap();
        assert_eq!(sh(&yield_script(&dir, "me")).trim(), "YIELDED");
        assert!(!t.0.join(store::FILE).exists());
        assert_eq!(sh(&record_script(&dir, HANDED)), "{}");
        assert_eq!(std::fs::read_to_string(t.0.join(HANDED_TO)).unwrap(), "me");
        // Taken again by another with nothing there: it says so.
        assert_eq!(sh(&yield_script(&dir, "other")).trim(), "GONE");
        sh(&undo_script(&dir));
        assert!(t.0.join(store::FILE).exists());
        assert!(!t.0.join(HANDED).exists());
        assert!(!t.0.join(HANDED_TO).exists());
    }

    #[test]
    fn a_new_writer_having_all_of_it_leaves_only_where_it_went() {
        let t = Temp::new();
        let dir = t.0.to_string_lossy().into_owned();
        std::fs::write(t.0.join(store::FILE), "{}").unwrap();
        sh(&yield_script(&dir, "me"));
        sh(&ask_script(&dir, "{\"it's\":1}"));
        assert_eq!(
            std::fs::read_to_string(t.0.join(REQUEST)).unwrap(),
            "{\"it's\":1}"
        );
        sh(&done_script(&dir));
        assert!(!t.0.join(HANDED).exists());
        assert!(!t.0.join(REQUEST).exists());
        assert!(t.0.join(HANDED_TO).exists());
        // Undoing after that finds nothing to put back.
        sh(&undo_script(&dir));
        assert!(!t.0.join(store::FILE).exists());
    }

    #[test]
    fn a_record_that_lost_is_kept_beside_the_others() {
        let t = Temp::new();
        let dir = t.0.to_string_lossy().into_owned();
        std::fs::write(t.0.join(store::FILE), "{}").unwrap();
        std::fs::write(t.0.join(store::HISTORY), "{}\n").unwrap();
        sh(&aside_script(&dir, "me", 7));
        assert!(!t.0.join(store::FILE).exists());
        assert!(t.0.join(format!("{ASIDE}7")).exists());
        assert_eq!(aside_name(store::FILE, 7), format!("{ASIDE}7"));
        // Its history goes with it; there are no rounds to move.
        assert!(!t.0.join(store::HISTORY).exists());
        assert!(t.0.join(aside_name(store::HISTORY, 7)).exists());
        assert!(!t.0.join(aside_name(store::ROUNDS, 7)).exists());
        assert_eq!(std::fs::read_to_string(t.0.join(HANDED_TO)).unwrap(), "me");
    }

    #[test]
    fn what_is_asked_of_a_sandbox_waits_until_it_is_taken_or_taken_back() {
        let t = Temp::new();
        let dir = t.0.to_string_lossy().into_owned();
        assert_eq!(sh(&sandbox_waits_script(&dir)), "");
        let ask = SandboxAsk {
            what: crate::feature_sandbox::StopFor::Stop,
            name: "lap'top".into(),
            started: 7,
        };
        let json = serde_json::to_string(&ask).unwrap();
        assert!(json.contains("\"stop\""), "{json}");
        sh(&sandbox_ask_script(&dir, &json));
        assert_eq!(sh(&sandbox_waits_script(&dir)).trim(), "WAITS");
        let raw = std::fs::read(t.0.join(SANDBOX_ASK)).unwrap();
        assert_eq!(serde_json::from_slice::<SandboxAsk>(&raw).unwrap(), ask);
        sh(&sandbox_unask_script(&dir));
        assert_eq!(sh(&sandbox_waits_script(&dir)), "");
        // An answer left from before says nothing of a new request.
        std::fs::write(t.0.join(SANDBOX_ANSWER), "{}").unwrap();
        sh(&sandbox_ask_script(&dir, &json));
        assert!(!t.0.join(SANDBOX_ANSWER).exists());
        sh(&sandbox_told_script(&dir, &json));
        let raw = std::fs::read(t.0.join(SANDBOX_TOLD)).unwrap();
        assert_eq!(serde_json::from_slice::<SandboxAsk>(&raw).unwrap().started, 7);
    }

    #[test]
    fn a_lock_held_tells_a_gerdr_is_open() {
        let t = Temp::new();
        let data = t.0.to_string_lossy().into_owned();
        // No gerdr ever showed it.
        assert_eq!(sh(&open_script(&data, "p")).trim(), "FREE");
        let lock = crate::project_lock::ProjectLock::take(&lock_dir(&t.0), "p").unwrap();
        assert_eq!(sh(&open_script(&data, "p")).trim(), "HELD");
        drop(lock);
        assert_eq!(sh(&open_script(&data, "p")).trim(), "FREE");
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    /// A repository with two commits, a detached worktree at the second and that
    /// commit in a ref, as a look from another machine leaves it.
    fn placed(t: &Temp) -> (std::path::PathBuf, std::path::PathBuf, String, String) {
        let main = t.0.join("r");
        std::fs::create_dir_all(&main).unwrap();
        git(&main, &["init", "-q", "-b", "main"]);
        git(&main, &["commit", "-q", "--allow-empty", "-m", "one"]);
        let one = git(&main, &["rev-parse", "HEAD"]);
        git(&main, &["commit", "-q", "--allow-empty", "-m", "two"]);
        let two = git(&main, &["rev-parse", "HEAD"]);
        git(&main, &["update-ref", "refs/gerdr/head", &two]);
        let wt = t.0.join("w");
        git(
            &main,
            &["worktree", "add", "-q", "--detach", &wt.to_string_lossy(), &two],
        );
        (main, wt, one, two)
    }

    #[test]
    fn a_worktree_looked_at_goes_on_its_branch() {
        let t = Temp::new();
        let (main, wt, _, two) = placed(&t);
        sh(&branch_script(&main, &wt, "feature/x", "refs/gerdr/head", 9));
        assert_eq!(git(&wt, &["symbolic-ref", "HEAD"]), "refs/heads/feature/x");
        assert_eq!(git(&wt, &["rev-parse", "HEAD"]), two);
    }

    #[test]
    fn a_branch_behind_what_came_catches_up() {
        let t = Temp::new();
        let (main, wt, one, two) = placed(&t);
        git(&main, &["branch", "feature/x", &one]);
        sh(&branch_script(&main, &wt, "feature/x", "refs/gerdr/head", 9));
        assert_eq!(git(&wt, &["rev-parse", "feature/x"]), two);
    }

    #[test]
    fn a_branch_ahead_of_what_came_is_left_as_it_is() {
        let t = Temp::new();
        let (main, wt, one, two) = placed(&t);
        git(&main, &["branch", "feature/x", &two]);
        git(&main, &["update-ref", "refs/gerdr/head", &one]);
        sh(&branch_script(&main, &wt, "feature/x", "refs/gerdr/head", 9));
        assert_eq!(git(&wt, &["symbolic-ref", "HEAD"]), "refs/heads/feature/x");
        assert_eq!(git(&wt, &["rev-parse", "HEAD"]), two);
    }

    #[test]
    fn a_branch_gone_another_way_is_kept_under_another_name() {
        let t = Temp::new();
        let (main, wt, one, two) = placed(&t);
        git(&main, &["checkout", "-q", "--detach", &one]);
        git(&main, &["commit", "-q", "--allow-empty", "-m", "other"]);
        let other = git(&main, &["rev-parse", "HEAD"]);
        git(&main, &["branch", "feature/x", &other]);
        sh(&branch_script(&main, &wt, "feature/x", "refs/gerdr/head", 9));
        assert_eq!(git(&wt, &["rev-parse", "HEAD"]), two);
        assert_eq!(git(&main, &["rev-parse", "feature/x-aside-9"]), other);
    }
}

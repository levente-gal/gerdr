//! How busy machines are, for work placed on whichever is least busy.
//!
//! Each link's thread asks its machine now and then (`machine_work::look_there`).
//! A machine's load average trails what just started on it by a minute, so what
//! gerdr started there lately counts too: three agents started in a row shouldn't
//! all go to the one that looked free.
//!
//! The point of other machines is to keep this one free: it is only chosen when
//! it is the only one, or when all the others are full and it isn't.
//!
//! A machine where the feature is set up keeps it while it has room. What waits
//! for a setup asks where to go once that's over, and by then the setup itself has
//! made that machine look busy: it would go on to the next, and set up there too.

use crate::{Gerdr, feature::{Feature, Stage, now}, machine_work::Placed};

/// What a stage's place says for whichever is least busy: in a feature's pick,
/// and in the project file's `machines`.
pub const LEAST_BUSY: &str = "least busy";
/// The script saying how busy a machine is: what `parse_load` reads.
pub const LOAD_SCRIPT: &str = "echo \"@load $(cut -d' ' -f1 /proc/loadavg) $(getconf _NPROCESSORS_ONLN 2>/dev/null || nproc 2>/dev/null || echo 1)\"\n";
/// How long something started on a machine counts apart from its load average.
const RECENT_SECS: u64 = 60;
/// How much work per core makes a machine full.
const FULL: f32 = 1.0;

/// How busy a machine is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Load {
    /// Its load average over a minute.
    pub average: f32,
    pub cores: u32,
}

impl Load {
    /// This machine's. Not being able to tell leaves it out.
    fn here() -> Option<Self> {
        let text = std::fs::read_to_string("/proc/loadavg").ok()?;
        let average = text.split_whitespace().next()?.parse().ok()?;
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get() as u32);
        Some(Self { average, cores })
    }
}

/// How busy what `LOAD_SCRIPT` said makes a machine.
pub fn parse_load(said: &str) -> Option<Load> {
    let line = said.lines().find_map(|line| line.strip_prefix("@load "))?;
    let mut words = line.split_whitespace();
    let average = words.next()?.parse().ok()?;
    let cores = words.next()?.parse().ok()?;
    Some(Load { average, cores })
}

/// A place work could go.
#[derive(Clone, Debug, PartialEq)]
struct Seat {
    id: String,
    load: Option<Load>,
    /// What gerdr started there lately.
    recent: u32,
    /// The feature being placed is set up there, or being set up.
    set_up: bool,
}

impl Seat {
    /// How much work per core it has, or `None` when that isn't known.
    fn score(&self) -> Option<f32> {
        let load = self.load?;
        Some((load.average + self.recent as f32) / load.cores.max(1) as f32)
    }
}

/// The machine to use out of `machines`, or `None` for this one. One that hasn't
/// said how busy it is comes last among them: it may be the only one up.
fn choose(here: &Seat, machines: &[Seat]) -> Option<String> {
    let rank = |seat: &&Seat| seat.score().unwrap_or(f32::INFINITY);
    let least = |seats: &mut dyn Iterator<Item = &'_ Seat>| -> Option<String> {
        let best = seats.min_by(|a, b| (rank(a).total_cmp(&rank(b))).then_with(|| a.id.cmp(&b.id)))?;
        Some(best.id.clone())
    };
    let mut roomy = (machines.iter()).filter(|seat| seat.set_up && rank(seat) < FULL);
    if let Some(id) = least(&mut roomy) {
        return Some(id);
    }
    let id = least(&mut machines.iter())?;
    let best = machines.iter().find(|seat| seat.id == id)?;
    if let (Some(there), Some(mine)) = (best.score(), here.score())
        && there >= FULL
        && mine < there
    {
        return None;
    }
    Some(best.id.clone())
}

impl Gerdr {
    /// Whether `stage`'s place in `feature` says whichever is least busy.
    pub(crate) fn goes_to_least_busy(&self, feature: &Feature, stage: Stage) -> bool {
        if let Some(picked) = feature.stage_machines.get(&stage) {
            return picked == LEAST_BUSY;
        }
        (self.projects.project(&feature.project))
            .and_then(|p| p.workflow.machines.get(&stage))
            .is_some_and(|name| name == LEAST_BUSY)
    }

    /// What gerdr started lately on machine `id`, `None` for this one.
    fn started_lately(&self, id: Option<&str>) -> u32 {
        let since = now().saturating_sub(RECENT_SECS);
        let mut count = 0;
        for feature in self.features.all.values().flatten() {
            let runs = feature.runs.values();
            count += runs.filter(|r| r.machine.as_deref() == id && r.started >= since).count();
            if let Some(run) = feature.verify.as_ref()
                && run.machine.as_deref() == id
                && run.started >= since
            {
                count += run.checks.len();
            }
            if let Some(run) = feature.sandbox.as_ref()
                && run.machine.as_deref() == id
                && run.started >= since
            {
                count += 1;
            }
        }
        count as u32
    }

    /// The place with the least to do, for `feature`.
    pub(crate) fn least_busy(&self, feature: &Feature) -> Placed {
        let machines: Vec<Seat> = (self.machines.links.values())
            .filter_map(|link| link.work.as_ref())
            .map(|work| Seat {
                id: work.id.clone(),
                load: work.load,
                recent: self.started_lately(Some(&work.id)),
                set_up: feature.setup_at(Some(&work.id)).is_some(),
            })
            .collect();
        let here = Seat {
            id: String::new(),
            load: Load::here(),
            recent: self.started_lately(None),
            set_up: false,
        };
        choose(&here, &machines).map_or(Placed::Here, Placed::There)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seat(id: &str, average: f32, cores: u32, recent: u32) -> Seat {
        Seat {
            id: id.to_owned(),
            load: Some(Load { average, cores }),
            recent,
            set_up: false,
        }
    }

    fn unknown(id: &str) -> Seat {
        Seat {
            id: id.to_owned(),
            load: None,
            recent: 0,
            set_up: false,
        }
    }

    #[test]
    fn what_a_machine_says_of_its_load_is_read() {
        let load = parse_load("@load 1.50 8\n@holder x\n").unwrap();
        assert_eq!(load, Load { average: 1.5, cores: 8 });
        assert_eq!(parse_load("nothing\n"), None);
        assert_eq!(parse_load("@load x 8\n"), None);
    }

    #[test]
    fn the_script_says_what_is_read_here() {
        let out = std::process::Command::new("sh")
            .args(["-c", LOAD_SCRIPT])
            .output()
            .unwrap();
        let said = String::from_utf8(out.stdout).unwrap();
        let load = parse_load(&said).expect(&said);
        assert!(load.cores >= 1);
    }

    #[test]
    fn the_machine_with_the_least_per_core_is_chosen() {
        let here = seat("", 0.0, 8, 0);
        let machines = [seat("small", 2.0, 2, 0), seat("big", 2.0, 16, 0)];
        assert_eq!(choose(&here, &machines).as_deref(), Some("big"));
    }

    #[test]
    fn this_machine_stays_free_while_another_has_room() {
        let here = seat("", 0.0, 8, 0);
        let machines = [seat("w", 3.0, 4, 0)];
        assert_eq!(choose(&here, &machines).as_deref(), Some("w"));
    }

    #[test]
    fn with_all_others_full_this_machine_helps_out() {
        let here = seat("", 1.0, 8, 0);
        let machines = [seat("a", 4.0, 4, 0), seat("b", 9.0, 4, 0)];
        assert_eq!(choose(&here, &machines), None);
    }

    #[test]
    fn a_fuller_machine_here_doesnt_take_it_from_a_full_one() {
        let here = seat("", 16.0, 4, 0);
        let machines = [seat("a", 4.0, 4, 0)];
        assert_eq!(choose(&here, &machines).as_deref(), Some("a"));
    }

    #[test]
    fn what_just_started_counts_before_the_average_shows_it() {
        let here = seat("", 0.0, 8, 0);
        let machines = [seat("a", 0.0, 4, 2), seat("b", 0.5, 4, 0)];
        assert_eq!(choose(&here, &machines).as_deref(), Some("b"));
    }

    #[test]
    fn a_machine_with_the_feature_set_up_keeps_it_while_it_has_room() {
        let here = seat("", 0.0, 8, 0);
        let free = seat("free", 0.0, 8, 0);
        let mut set_up = seat("set", 2.0, 8, 0);
        set_up.set_up = true;
        let machines = [free.clone(), set_up.clone()];
        assert_eq!(choose(&here, &machines).as_deref(), Some("set"));
        // Full, it isn't worth waiting for.
        set_up.load = Some(Load { average: 8.0, cores: 8 });
        let machines = [free, set_up];
        assert_eq!(choose(&here, &machines).as_deref(), Some("free"));
    }

    #[test]
    fn with_no_machine_it_stays_here() {
        assert_eq!(choose(&seat("", 0.0, 8, 0), &[]), None);
    }

    #[test]
    fn a_machine_not_heard_from_comes_after_those_that_said() {
        let here = seat("", 0.0, 8, 0);
        let machines = [unknown("a"), seat("b", 3.0, 4, 0)];
        assert_eq!(choose(&here, &machines).as_deref(), Some("b"));
        assert_eq!(choose(&here, &[unknown("a")]).as_deref(), Some("a"));
    }

    #[test]
    fn equal_machines_are_told_apart_the_same_way_each_time() {
        let here = seat("", 0.0, 8, 0);
        let machines = [seat("b", 1.0, 4, 0), seat("a", 1.0, 4, 0)];
        assert_eq!(choose(&here, &machines).as_deref(), Some("a"));
    }
}

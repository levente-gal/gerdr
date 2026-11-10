//! Handing a feature back to the machine it was taken over from.
//!
//! A feature moves to a machine when something is pressed on it there
//! (`machine_lease`). The way back is the same take, started by the machine it
//! came from: the machine writing it now leaves `lease::HAND_BACK` in that one's
//! folder of the feature, and the gerdr open there takes it, as if pressed, with
//! nothing more to do once it has it. Its sandbox, still running there, is its
//! own again (`lease::KEPT`).
//!
//! Nothing is left behind for later: with no gerdr open there on the project,
//! the hand back is refused at once, and one not taken up in time is taken back.

use std::{
    path::Path,
    sync::atomic::AtomicBool,
    time::Instant,
};

use crate::{
    Gerdr,
    feature::{Status, lease, remote::Reach, store},
    features::{Busy, Done, Job},
    machine_lease::{own_id_made, this_machines_name},
};

/// How often the machine asked is looked at, to see it took the hand back up.
const ASK_EVERY: std::time::Duration = std::time::Duration::from_millis(400);

/// What a thread needs to ask the machine a feature came from to take it back.
struct HandBackJob {
    reach: Reach,
    /// gerdr's data folder there.
    data: String,
    /// The feature's folder there.
    dir: String,
    project: String,
    /// The project, as the user knows it.
    project_name: String,
    /// The machine asked, by name.
    name: String,
    request: lease::HandBack,
}

impl HandBackJob {
    /// Ask, and wait to see that the gerdr there took it up. `Err`: why not.
    fn go(&self) -> Result<(), String> {
        let never = AtomicBool::new(false);
        let script = |s: String| reach_script(&self.reach, &s, &never);
        let open = script(lease::open_script(&self.data, &self.project))?;
        if open.trim() != "HELD" {
            return Err(format!(
                "Open {} in gerdr on {} first, then hand it back",
                self.project_name, self.name
            ));
        }
        let json = serde_json::to_string(&self.request).map_err(|err| err.to_string())?;
        script(lease::hand_back_script(&self.dir, &json))?;
        let until = Instant::now() + lease::ASK_FOR;
        while Instant::now() < until {
            if script(lease::hand_back_waits_script(&self.dir))?.trim().is_empty() {
                let said = script(lease::record_script(&self.dir, lease::HAND_BACK_ANSWER))?;
                return match serde_json::from_str::<lease::HandBackAnswer>(&said) {
                    Ok(lease::HandBackAnswer { refused: Some(why) }) => Err(why),
                    Ok(_) => Ok(()),
                    // Gone with no answer: another took it up, or it went.
                    Err(_) => Err(format!("{} didn't say what came of it", self.name)),
                };
            }
            std::thread::sleep(ASK_EVERY);
        }
        if let Err(why) = script(lease::hand_back_unask_script(&self.dir)) {
            tracing::warn!("taking back a hand back: {why}");
        }
        Err(format!(
            "gerdr on {} didn't answer: the feature stays here",
            self.name
        ))
    }
}

/// Where a hand back of feature `slug` is asked, under gerdr's data folder `data`
/// on the machine asked: the folder of the feature's own record, which it left
/// when it was taken over. Not `store::dir`, which finds the copy of what the
/// machine handing it back writes, and that copy is the other machine's.
fn asked_in(data: &Path, project: &str, slug: &str) -> std::path::PathBuf {
    store::project_dir(Some(data), project).join(slug)
}

/// `script` run there, its trouble as text.
fn reach_script(reach: &Reach, script: &str, cancel: &AtomicBool) -> Result<String, String> {
    reach.script(script, cancel).map_err(|trouble| trouble.text().to_owned())
}

/// Write the answer to a hand back in feature folder `dir`, then let go of the
/// request: the one asking reads the answer once that is gone.
fn answer(dir: &Path, refused: Option<String>) {
    let answer = lease::HandBackAnswer { refused };
    match serde_json::to_vec(&answer) {
        Ok(bytes) => {
            if let Err(err) = crate::projects::write_atomic(&dir.join(lease::HAND_BACK_ANSWER), &bytes) {
                tracing::warn!("answering a hand back in {}: {err}", dir.display());
            }
        }
        Err(err) => tracing::warn!("answering a hand back: {err}"),
    }
    let file = dir.join(lease::HAND_BACK);
    if let Err(err) = std::fs::remove_file(&file)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("removing {}: {err}", file.display());
    }
}

/// Why a hand back from machine `asking` (`request`) of a feature written by
/// `writer` at `epoch` isn't one to take: another machine, or it moved since.
fn stale(request: &lease::HandBack, writer: Option<&str>, epoch: u64) -> Option<String> {
    if writer != Some(request.machine.as_str()) {
        return Some(format!("It isn't written on {} any more", request.name));
    }
    (epoch != request.epoch).then(|| "It changed just before: hand it back again".to_owned())
}

impl Gerdr {
    /// Hand feature `slug` back to the machine it was taken over from, off the
    /// thread: the gerdr there takes it.
    pub(crate) fn hand_back(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let title = feature.title.clone();
        let Some(id) = feature.came_from.clone().filter(|_| feature.writer.is_none()) else {
            return self.show_warning(format!(
                "\u{201c}{title}\u{201d} wasn't taken over from another machine"
            ));
        };
        let name = self.machine_name(&id);
        if feature.status != Status::Open {
            return self.show_warning(format!("\u{201c}{title}\u{201d} is over: it stays here"));
        }
        if self.busy_with(project, slug, Job::HandBack).is_some() {
            return self.show_warning(format!(
                "\u{201c}{title}\u{201d} is being handed back to {name}"
            ));
        }
        if let Some(why) = self.keeps_feature(&feature) {
            return self.show_warning(why);
        }
        let Some(work) = self.work_of(&id) else {
            return self.show_warning(format!(
                "Connect {name} here to hand \u{201c}{title}\u{201d} back"
            ));
        };
        let Some(me) = own_id_made() else {
            return self.show_error("gerdr couldn't save this machine's id".to_owned());
        };
        let data = format!("{}/gerdr", work.info.site.data);
        let project_name = (self.projects.project(project))
            .map_or_else(|| project.to_owned(), |p| p.name.clone());
        let job = HandBackJob {
            reach: work.reach(),
            dir: asked_in(Path::new(&data), project, slug).to_string_lossy().into_owned(),
            data,
            project: project.to_owned(),
            project_name,
            name: name.clone(),
            request: lease::HandBack {
                machine: me,
                name: this_machines_name(),
                epoch: feature.epoch,
            },
        };
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::HandBack,
            repo: None,
            what: format!("Handing \u{201c}{title}\u{201d} back to {name}"),
        };
        self.show_notice(format!("Handing \u{201c}{title}\u{201d} back to {name}\u{2026}"));
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move |_| Done::HandBack {
            result: job.go(),
            project,
            slug,
        });
    }

    /// A hand back of feature `slug` ended: taken up there, it moves there in a
    /// moment, as any take does; refused, the user hears why.
    pub(crate) fn handed_back(&mut self, project: &str, slug: &str, result: Result<(), String>) {
        self.unbusy(project, slug, Job::HandBack);
        if let Err(why) = result {
            tracing::warn!("{project}/{slug}: handing it back: {why}");
            self.show_warning(why);
        }
    }

    /// Take back the features of `project` that the machines writing them hand
    /// back, as asked in their folders here.
    pub(crate) fn answer_hand_backs(&mut self, project: &str) {
        let asked: Vec<(String, std::path::PathBuf, lease::HandBack)> = (self.viewed_of(project).iter())
            .filter(|f| f.status == Status::Open)
            .filter_map(|f| {
                let dir = asked_in(&crate::projects::data_dir(), project, &f.slug);
                let file = dir.join(lease::HAND_BACK);
                let age = std::fs::metadata(&file).ok()?.modified().ok()?.elapsed().ok();
                let raw = std::fs::read(&file).ok()?;
                // Nobody waits for that one any more.
                if age.is_some_and(|age| age > lease::REQUEST_STANDS) {
                    if let Err(err) = std::fs::remove_file(&file) {
                        tracing::warn!("removing {}: {err}", file.display());
                    }
                    return None;
                }
                match serde_json::from_slice(&raw) {
                    Ok(request) => Some((f.slug.clone(), dir, request)),
                    Err(err) => {
                        tracing::warn!("reading {}: {err}", file.display());
                        answer(&dir, Some("gerdr there couldn't read it".to_owned()));
                        None
                    }
                }
            })
            .collect();
        for (slug, dir, request) in asked {
            let refused = self.hand_back_refusal(project, &slug, &request);
            let take = refused.is_none();
            answer(&dir, refused);
            if !take {
                continue;
            }
            let key = (project.to_owned(), slug.clone());
            if self.features.taking.contains_key(&key) {
                continue;
            }
            if let Some(title) = self.feature(project, &slug).map(|f| f.title.clone()) {
                self.show_notice(format!(
                    "{} hands \u{201c}{title}\u{201d} back",
                    request.name
                ));
            }
            self.features.taking_back.insert(key.clone());
            self.start_take(project, &slug, None, false);
            // Still there: it didn't start, and said why.
            self.features.taking_back.remove(&key);
        }
    }

    /// Why feature `slug` isn't taken back as `request` asks; none: it is.
    fn hand_back_refusal(
        &self,
        project: &str,
        slug: &str,
        request: &lease::HandBack,
    ) -> Option<String> {
        let feature = self.feature(project, slug)?;
        if let Some(why) = stale(request, feature.writer.as_deref(), feature.epoch) {
            return Some(why);
        }
        let here = this_machines_name();
        if self.work_of(&request.machine).is_none() {
            return Some(format!("{here} isn't connected to {}: connect it there first", request.name));
        }
        if (self.features.taking).contains_key(&(project.to_owned(), slug.to_owned())) {
            return None;
        }
        let Some(me) = own_id_made() else {
            return Some(format!("gerdr on {here} couldn't save its machine's id"));
        };
        self.take_blocker(feature, &me).map(|blocker| blocker.why)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(epoch: u64) -> lease::HandBack {
        lease::HandBack {
            machine: "lap".into(),
            name: "Laptop".into(),
            epoch,
        }
    }

    #[test]
    fn only_a_hand_back_from_the_writer_of_the_record_as_it_is_is_taken() {
        assert_eq!(stale(&request(3), Some("lap"), 3), None);
        assert_eq!(
            stale(&request(3), Some("other"), 3).as_deref(),
            Some("It isn't written on Laptop any more")
        );
        assert!(stale(&request(3), None, 3).is_some());
        assert_eq!(
            stale(&request(2), Some("lap"), 3).as_deref(),
            Some("It changed just before: hand it back again")
        );
    }

    #[test]
    fn a_hand_back_is_looked_for_where_it_is_asked() {
        // As the machine handing it back names the folder there.
        let data = "/home/ana/.local/share/gerdr";
        let there = asked_in(Path::new(data), "web", "login");
        assert_eq!(there, Path::new("/home/ana/.local/share/gerdr/features/web/login"));
        // As the one asked looks for it: its own record's folder, not the copy.
        let here = asked_in(&crate::projects::data_dir(), "web", "login");
        assert_eq!(here, store::project_dir(None, "web").join("login"));
    }

    #[test]
    fn an_answer_is_written_before_the_request_goes() {
        let dir = std::env::temp_dir().join(format!("gerdr-hand-back-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(lease::HAND_BACK), b"{}").unwrap();
        answer(&dir, Some("busy".into()));
        assert!(!dir.join(lease::HAND_BACK).exists());
        let said: lease::HandBackAnswer =
            serde_json::from_slice(&std::fs::read(dir.join(lease::HAND_BACK_ANSWER)).unwrap())
                .unwrap();
        assert_eq!(said.refused.as_deref(), Some("busy"));
        answer(&dir, None);
        let said: lease::HandBackAnswer =
            serde_json::from_slice(&std::fs::read(dir.join(lease::HAND_BACK_ANSWER)).unwrap())
                .unwrap();
        assert_eq!(said.refused, None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

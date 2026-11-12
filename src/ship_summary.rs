//! A shipping feature's summary: an agent writes `summary.md` in its data
//! dir from the brief, the plan and the diff, once Ship is reached or when
//! the user asks again. Ship's recap shows it, its pull requests take it as
//! their body and its squash as its message (`ship::pr_body`,
//! `ship::squash_command`).

use std::path::Path;

use crate::{
    Gerdr,
    feature::{
        self, Feature, SUMMARY_FILE, Stage, Status, SummaryRun,
        agents::{command_line, with_prompt},
        store,
    },
    feature_agents::{vars, write_prompt},
    prompts,
};

/// What tells each repository's diff, for the prompt: "- <worktree>: `git
/// diff main...HEAD`".
fn diffs(feature: &Feature) -> String {
    (feature.touched())
        .map(|repo| {
            format!(
                "- {}: `git diff {}...HEAD` and `git log --oneline {}..HEAD`",
                repo.worktree.display(),
                repo.base,
                repo.base
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether `feature` still wants a summary started on its own: at Ship or
/// done here, with none written and none asked for.
pub fn wants_summary(feature: &Feature, data: &Path) -> bool {
    feature.writer.is_none()
        && feature.shipping.summary.is_none()
        && (feature.status == Status::Open && feature.current() == Some(Stage::Ship))
        && !data.join(SUMMARY_FILE).exists()
}

impl Gerdr {
    /// Ask an agent to write feature `slug`'s summary, in a tab of its
    /// workspace behind what shows. `asked`: the user pressed for it, so
    /// what stops it says so.
    pub(crate) fn write_ship_summary(&mut self, project: &str, slug: &str, asked: bool) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let say = |gerdr: &mut Self, why: String| {
            tracing::warn!("writing the summary of {project}/{slug}: {why}");
            if asked {
                gerdr.show_warning(format!(
                    "Couldn't write the summary of \u{201c}{}\u{201d}: {why}",
                    feature.title
                ));
            }
        };
        let (agent, base) = match self.summary_agent(&feature) {
            Ok(pick) => pick,
            Err(why) => return say(self, why),
        };
        let data = store::dir(None, project, slug);
        let n = feature.shipping.summary.as_ref().map_or(1, |run| run.n + 1);
        let names = self.projects.names(&feature);
        let mut values = vars(&feature, &names, &data, Stage::Ship);
        values.insert(
            "summary.path".into(),
            data.join(SUMMARY_FILE).display().to_string(),
        );
        values.insert("summary.diffs".into(), diffs(&feature));
        let text = match prompts::render_builtin("summary", &values) {
            Ok(text) => text,
            Err(err) => return say(self, format!("its prompt couldn't be written: {err}")),
        };
        let prompt = match write_prompt(&data, &format!("summary-{n}.md"), &text) {
            Ok(prompt) => prompt,
            // It follows "Couldn't write the summary of …: ".
            Err(why) => return say(self, why.replacen("Couldn't", "couldn't", 1)),
        };
        let line = command_line(&base, &prompt, &feature.agent_dirs_for(&base, &data));
        let start = format!("Read {} and follow it.", prompt.display());
        let command = with_prompt(&line, &start);
        let Some(pane) = self.open_feature_tab_behind(project, slug, &feature.dir, command) else {
            // Opening the tab said why.
            return;
        };
        let now = feature::now();
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        feature.shipping.summary = Some(SummaryRun {
            at: now,
            agent: agent.clone(),
            pane: Some(pane),
            n,
        });
        feature.log(now, format!("Summary asked of {agent}"));
        self.save_feature(project, slug);
        if asked {
            self.show_success(format!("{agent} is writing the summary"));
        }
    }

    /// Open feature `slug`'s summary to edit, written or not yet.
    pub(crate) fn edit_ship_summary(&mut self, project: &str, slug: &str) {
        // A pull request's review has its own words over its comments.
        let file = match self.feature(project, slug).and_then(|f| f.pull()) {
            Some(_) => crate::feature::pr_submit::BODY_FILE,
            None => SUMMARY_FILE,
        };
        let path = store::dir(None, project, slug).join(file);
        if !path.exists()
            && let Err(err) = std::fs::write(&path, "")
        {
            tracing::warn!("making {}: {err}", path.display());
            return self.show_error(format!("Couldn't make {}: {err}", path.display()));
        }
        self.open_in_zed(&path, crate::launch::Place::Tab, "the summary");
    }
}

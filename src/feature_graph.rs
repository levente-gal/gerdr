//! Keeping the code graphs of features: built when a feature or link is
//! made and for features that lack one, refreshed as a stage's agent starts
//! (see `feature::graph`). It shows in the tasks panel and holds nothing
//! up; taking a folder down stops it first.

use std::{path::PathBuf, sync::Arc};

use crate::{
    Gerdr,
    feature::{Status, graph, store},
    features::{Busy, Done, Job, tilde},
};

/// The problems list's source for a missing graphify.
const PROBLEM: &str = "graphify";

/// Stop `graphs` and wait until they ended. Blocks: off the UI thread.
pub(crate) fn stop_graphs(graphs: Option<Arc<graph::Run>>) {
    if let Some(run) = graphs {
        run.stop_and_wait();
    }
}

/// What the toast says of the git exclude files `excludes` that changed: a
/// project's many repositories' whole paths would cut each other off.
fn excluded_text(excludes: &[PathBuf]) -> Option<String> {
    let out = graph::OUT_DIR;
    match excludes {
        [] => None,
        [one] => Some(format!("Added /{out}/ to {}", tilde(one))),
        many => Some(format!(
            "Added /{out}/ to the git exclude files of {} repositories",
            many.len()
        )),
    }
}

impl Gerdr {
    /// Build the code graphs of open feature `slug` off the thread, if its
    /// projects keep graphs: only those it lacks if `only_missing`. A build
    /// already running does for this one too.
    pub(crate) fn refresh_feature_graphs(&mut self, project: &str, slug: &str, only_missing: bool) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        if feature.status != Status::Open || self.busy_with(project, slug, Job::Graph).is_some() {
            return;
        }
        let mut folders = graph::folders(feature);
        if only_missing {
            folders.retain(|f| !graph::file(&f.dir).exists());
        }
        if folders.is_empty() {
            return;
        }
        let title = feature.title.clone();
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Graph,
            repo: None,
            what: format!("Building the code graph of \u{201c}{title}\u{201d}"),
        };
        let log = store::dir(None, project, slug).join("graph.log");
        let run = Arc::new(graph::Run::default());
        let key = (project.to_owned(), slug.to_owned());
        self.features.graphs.insert(key, run.clone());
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move |_| {
            let built = graph::build(&folders, &slug, &log, &run);
            Done::Graphed {
                project,
                slug,
                title,
                built,
            }
        });
    }

    /// The code graph build of feature `slug` running now, to stop.
    pub(crate) fn running_feature_graphs(
        &self,
        project: &str,
        slug: &str,
    ) -> Option<Arc<graph::Run>> {
        let key = (project.to_owned(), slug.to_owned());
        self.features.graphs.get(&key).cloned()
    }

    /// A build of feature `slug`'s code graphs ended: what it changed and
    /// what stands in the way is said.
    pub(crate) fn feature_graphs_built(
        &mut self,
        project: &str,
        slug: &str,
        title: &str,
        built: graph::Built,
    ) {
        self.unbusy(project, slug, Job::Graph);
        let key = (project.to_owned(), slug.to_owned());
        self.features.graphs.remove(&key);
        if let Some(text) = excluded_text(&built.excludes) {
            self.show_success(text);
        }
        if built.missing {
            let text = "graphify isn't on PATH, so features get no code graph";
            crate::problems::report(PROBLEM, text);
            // Each stage would say it again.
            if !std::mem::replace(&mut self.features.graphify_told, true) {
                self.show_warning(text.into());
            }
            return;
        }
        crate::problems::clear(PROBLEM);
        if let Some(why) = built.failed {
            tracing::warn!("building the code graph of {project}/{slug}: {why}");
            self.show_warning(format!(
                "Couldn't build the code graph of \u{201c}{title}\u{201d}: {why}"
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_toast_names_one_exclude_file_and_counts_many() {
        assert_eq!(excluded_text(&[]), None);
        let one = excluded_text(&["/r/.git/info/exclude".into()]).unwrap();
        assert_eq!(one, "Added /graphify-out/ to /r/.git/info/exclude");
        let many = excluded_text(&vec!["/r/x".into(); 40]).unwrap();
        assert_eq!(
            many,
            "Added /graphify-out/ to the git exclude files of 40 repositories"
        );
    }
}

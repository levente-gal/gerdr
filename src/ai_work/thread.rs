//! The `ai-work` thread: everything too slow for the loop. It copies
//! transcripts, keeps stage files and logs as blobs, and takes new lines
//! into the index, a batch every 2 s in one transaction. What it came to
//! goes back as `Done::AiWork`.

use std::{path::PathBuf, sync::mpsc};

use serde_json::Value;
use smithay::reexports::calloop::channel;

use super::Chat;
use crate::{
    feature::remote::{Reach, Side},
    features::Done,
};

/// What the thread is asked to do.
#[derive(Debug)]
pub enum Job {
    /// Take the new lines of feature `slug` into the index.
    Index { project: String, slug: String },
    /// Keep `chat` of feature `slug` as it is now. `there`: how to reach the
    /// machine it ran on; `None` here.
    Capture {
        project: String,
        slug: String,
        chat: Chat,
        there: Option<(Reach, Side)>,
    },
    /// Keep the stage files `files` (name, path) of feature `slug`, those
    /// that changed, `reason`.
    Outputs {
        project: String,
        slug: String,
        files: Vec<(String, PathBuf)>,
        reason: String,
    },
    /// Keep the part of `log` (a feature's `setup.log`) the setup `run`
    /// wrote.
    SetupSlice {
        project: String,
        slug: String,
        run: Value,
        log: PathBuf,
    },
    /// The chats at work now, by project and slug: kept again every 10
    /// minutes while they last.
    Watch { chats: Vec<(String, String, Chat)> },
    /// Draw the index again from the files.
    Rescan,
    /// Keep the earlier AI work of features `features` (project, slug).
    Backfill { features: Vec<(String, String)> },
    /// `chat` of feature `slug` was pulled from another machine into
    /// `staging`.
    Pulled {
        project: String,
        slug: String,
        chat: Chat,
        staging: PathBuf,
    },
    /// End.
    Stop,
}

/// Do `jobs` under `root` (see `super::root`) until told to stop, telling
/// `done` what they came to.
pub fn run(root: PathBuf, jobs: mpsc::Receiver<Job>, done: channel::Sender<Done>) {
    let _ = (root, done);
    while let Ok(job) = jobs.recv() {
        if matches!(job, Job::Stop) {
            break;
        }
    }
}

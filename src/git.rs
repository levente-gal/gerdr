//! Git status of the workspace folders, shown in the sidebar: the branch,
//! whether there are uncommitted changes, and how far the branch is ahead of
//! and behind its upstream. `git status` can take a while in a big
//! repository, so it runs off the event loop.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// Where a folder's repository stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitStatus {
    /// The checked-out branch; `None` on a detached HEAD.
    pub branch: Option<String>,
    /// Some file is changed, staged or untracked.
    pub dirty: bool,
    /// Commits the upstream doesn't have yet.
    pub ahead: u32,
    /// Upstream commits not merged here yet.
    pub behind: u32,
}

/// The status of the repository `dir` is in; `None` outside one or without git.
pub fn status(dir: &Path) -> Option<GitStatus> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        // Leave index.lock alone, so a refresh never gets in the way of the
        // user's own git commands.
        .args([
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "--branch",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| parse(&String::from_utf8_lossy(&out.stdout)))
}

/// Read the status of each of `dirs` on a thread of its own, and hand those
/// in a repository to `send`.
pub fn read_all(
    dirs: Vec<PathBuf>,
    send: impl FnOnce(HashMap<PathBuf, GitStatus>) + Send + 'static,
) {
    std::thread::spawn(move || {
        send(
            dirs.into_iter()
                .filter_map(|dir| Some((status(&dir)?, dir)))
                .map(|(status, dir)| (dir, status))
                .collect(),
        )
    });
}

/// The status in `git status --porcelain=v2 --branch` output: "# " header
/// lines about the branch, then a line per changed or untracked file.
fn parse(out: &str) -> GitStatus {
    let mut status = GitStatus::default();
    for line in out.lines() {
        let Some(header) = line.strip_prefix("# ") else {
            status.dirty |= !line.is_empty();
            continue;
        };
        if let Some(head) = header.strip_prefix("branch.head ") {
            status.branch = (head != "(detached)").then(|| head.to_string());
        } else if let Some(counts) = header.strip_prefix("branch.ab ") {
            for count in counts.split_whitespace() {
                if let Some(n) = count.strip_prefix('+') {
                    status.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = count.strip_prefix('-') {
                    status.behind = n.parse().unwrap_or(0);
                }
            }
        }
    }
    status
}

#[cfg(test)]
mod tests {
    use super::{GitStatus, parse};

    #[test]
    fn clean_branch_with_upstream() {
        let out = "# branch.oid 1a2b\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -1\n";
        assert_eq!(
            parse(out),
            GitStatus {
                branch: Some("main".into()),
                dirty: false,
                ahead: 2,
                behind: 1
            }
        );
    }

    #[test]
    fn changes_make_it_dirty() {
        let out = "# branch.oid 1a2b\n# branch.head topic\n1 .M N... 100644 100644 100644 a b src/main.rs\n";
        assert_eq!(
            parse(out),
            GitStatus {
                branch: Some("topic".into()),
                dirty: true,
                ahead: 0,
                behind: 0
            }
        );
        assert!(parse("# branch.head main\n? notes.txt\n").dirty);
    }

    #[test]
    fn detached_head_and_new_repo() {
        assert_eq!(
            parse("# branch.oid 1a2b\n# branch.head (detached)\n").branch,
            None
        );
        assert_eq!(
            parse("# branch.oid (initial)\n# branch.head master\n")
                .branch
                .as_deref(),
            Some("master")
        );
    }
}

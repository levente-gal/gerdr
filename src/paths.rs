//! Paths and titles as the chrome shows them.

use crate::herdr::PaneState;
use crate::home_dir;
use std::path::Path;

/// `path` with the home folder `home` written `~`.
pub fn tilde(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// A terminal title without the spinner or status glyph agents put in front.
pub(crate) fn tidy_title(title: &str) -> String {
    let glyph = |c: char| c.is_whitespace() || matches!(c, '\u{2190}'..='\u{2BFF}' | '·' | '•');
    title.trim_start_matches(glyph).trim_end().to_string()
}

/// The name of the repository pane `p` is in, and its worktree's folder.
pub(crate) fn repo_root(p: &PaneState) -> Option<(&str, &std::path::Path)> {
    p.repo
        .as_deref()
        .zip(p.worktree.as_ref().map(|w| w.path.as_path()))
}

/// `path`, short: from the repository named `repo` when it is inside that
/// repository's worktree, from "~" when it is inside the home folder, with
/// every folder between that start and the last cut to its first letter (fish
/// style): "~/Personal/gerdr" is "~/P/gerdr", "<worktree>/src/bin" is
/// "gerdr/s/bin".
pub(crate) fn short_path(path: &std::path::Path, root: Option<(&str, &std::path::Path)>) -> String {
    let in_repo = root.and_then(|(repo, worktree)| Some((repo, path.strip_prefix(worktree).ok()?)));
    let in_home = || home_dir().and_then(|home| Some(path.strip_prefix(home).ok()?.to_path_buf()));
    let (start, rest) = match in_repo {
        Some((repo, rest)) => (repo.to_string(), rest.to_path_buf()),
        None => match in_home() {
            Some(rest) => ("~".into(), rest),
            None => (
                String::new(),
                path.strip_prefix("/").unwrap_or(path).to_path_buf(),
            ),
        },
    };
    let names: Vec<String> = rest
        .iter()
        .map(|name| name.to_string_lossy().into_owned())
        .collect();
    let Some((last, between)) = names.split_last() else {
        return if start.is_empty() { "/".into() } else { start };
    };
    let initial = |name: &String| {
        let skip = name.chars().take_while(|&c| c == '.').count();
        name.chars().take(skip + 1).collect::<String>()
    };
    let mut parts = vec![start];
    parts.extend(between.iter().map(initial));
    parts.push(last.clone());
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::short_path;
    use std::path::Path;

    #[test]
    fn short_path_cuts_folders_between_start_and_last() {
        assert_eq!(
            short_path(Path::new("/usr/local/share/fonts"), None),
            "/u/l/s/fonts"
        );
        assert_eq!(short_path(Path::new("/etc/.config/x"), None), "/e/.c/x");
        assert_eq!(short_path(Path::new("/"), None), "/");
        assert_eq!(short_path(Path::new("/tmp"), None), "/tmp");
    }

    #[test]
    fn short_path_starts_from_repository() {
        let root = Some(("gerdr", Path::new("/work/gerdr")));
        assert_eq!(short_path(Path::new("/work/gerdr"), root), "gerdr");
        assert_eq!(
            short_path(Path::new("/work/gerdr/src/bin/tool"), root),
            "gerdr/s/b/tool"
        );
        assert_eq!(short_path(Path::new("/elsewhere/dir"), root), "/e/dir");
    }
}

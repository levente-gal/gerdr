//! The slash commands an agent CLI takes, for the chat's `/` menu: its own
//! built-in ones, and those found on disk (command files, skills, plugins),
//! as each CLI reads them.

use std::path::{Path, PathBuf};

use crate::ai_work::{AgentKind, locate::Homes};

/// A command offered: typed as `/<name>`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Command {
    pub name: String,
    /// What it does, in one line; may be empty.
    pub about: String,
    /// What it takes after it (`argument-hint`); may be empty.
    pub hint: String,
}

/// Whether agent `kind` takes side questions: a `/btw` of its own.
#[cfg(test)]
pub fn asks_aside(kind: AgentKind) -> bool {
    crate::integrations::of(kind).caps().side
}

/// The commands agent `kind` takes in folder `cwd`, by name: the project's
/// before the home's, those on disk before the built-ins. Unreadable files
/// are left out.
pub fn list(kind: AgentKind, cwd: &Path, homes: &Homes, config: &Path) -> Vec<Command> {
    let mut found = Vec::new();
    let agent = crate::integrations::of(kind);
    agent.disk_commands(cwd, homes, config, &mut found);
    let builtin = agent.builtins();
    found.extend(builtin.iter().map(|(name, about)| Command {
        name: (*name).to_owned(),
        about: (*about).to_owned(),
        hint: String::new(),
    }));
    let mut seen = std::collections::HashSet::new();
    found.retain(|c| seen.insert(c.name.clone()));
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// The commands agent `kind` takes in folder `cwd`, on this machine.
pub fn here(kind: AgentKind, cwd: &Path) -> Vec<Command> {
    let Some(homes) = Homes::here() else {
        return Vec::new();
    };
    let config = (std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
        .filter(|dir| dir.is_absolute())
        .or_else(|| crate::home_dir().map(|home| home.join(".config")))
        .unwrap_or_default();
    list(kind, cwd, &homes, &config)
}

/// What follows the `/` of a reply that is a command being typed: a `/`
/// and no space yet.
pub fn query(draft: &str) -> Option<&str> {
    let typed = draft.strip_prefix('/')?;
    (!typed.chars().any(char::is_whitespace)).then_some(typed)
}

/// How many of the menu's rows show at once; the picked one always does.
pub const ROWS: usize = 8;

/// The `/` menu over the reply box: the commands its agent takes, those
/// matching what follows the `/`, best first. The `@` menu is one too: its
/// rows are the files under `files` (`chat::files`), by their paths from it.
#[derive(Debug, Default)]
pub struct Slash {
    pub all: Vec<Command>,
    /// The folder of the `@` menu's files; none for the `/` menu.
    pub files: Option<PathBuf>,
    /// The `@` menu's files while they are being listed.
    loading: Option<std::sync::mpsc::Receiver<Vec<Command>>>,
    /// What it was filtered for; none before the first time.
    query: Option<String>,
    /// The commands matching, by index in `all`.
    pub shown: Vec<usize>,
    pub picked: usize,
    /// Esc closed it; typing on opens it again.
    pub hidden: bool,
}

impl Slash {
    pub fn new(all: Vec<Command>) -> Self {
        Self {
            all,
            ..Self::default()
        }
    }

    /// The `@` menu of the files under `root`, listed on a thread
    /// (`files::load`): it shows once they come (`take_loaded`).
    pub fn files(root: PathBuf) -> Self {
        Self {
            loading: Some(super::files::load(root.clone())),
            files: Some(root),
            ..Self::default()
        }
    }

    /// Take the files listed for the `@` menu, once they came, waiting at
    /// most `wait` for them; whether they came now.
    pub fn take_loaded(&mut self, wait: std::time::Duration) -> bool {
        let Some(rx) = &self.loading else {
            return false;
        };
        let got = match rx.recv_timeout(wait) {
            Ok(all) => all,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return false,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Vec::new(),
        };
        self.all = got;
        self.loading = None;
        // Filtered again for what is typed now; Esc's close kept.
        if let Some(query) = self.query.take() {
            let hidden = self.hidden;
            self.sync(&query);
            self.hidden = hidden;
        }
        true
    }

    /// Whether it shows, and takes its keys.
    pub fn open(&self) -> bool {
        !self.hidden && !self.shown.is_empty()
    }

    /// Filter it for `query`, the best match picked; the same query again
    /// changes nothing.
    pub fn sync(&mut self, query: &str) {
        if self.query.as_deref() == Some(query) {
            return;
        }
        let all = &self.all;
        let ranked = crate::fuzzy::rank(query, 0..all.len(), |i| all[*i].name.clone(), |_| 0.0);
        self.shown = ranked.into_iter().map(|r| r.item).collect();
        self.picked = 0;
        self.hidden = false;
        self.query = Some(query.to_owned());
    }

    /// Pick the row `by` rows down (up when negative), within the rows.
    pub fn step(&mut self, by: isize) {
        let last = self.shown.len().saturating_sub(1);
        self.picked = self.picked.saturating_add_signed(by).min(last);
    }

    /// The command picked.
    pub fn picked(&self) -> Option<&Command> {
        self.shown.get(self.picked).map(|i| &self.all[*i])
    }

    /// The rows in sight, from the first of them: at most `ROWS`, the
    /// picked one among them.
    pub fn in_sight(&self) -> (usize, &[usize]) {
        let first = (self.picked + 1).saturating_sub(ROWS);
        (
            first,
            &self.shown[first..(first + ROWS).min(self.shown.len())],
        )
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn side_questions_go_to_the_agents_with_a_btw() {
        use AgentKind::*;
        for kind in [Claude, OpenCode, Cursor, Antigravity] {
            assert!(asks_aside(kind), "{kind:?}");
        }
        for kind in [Codex, Pi, T3, Unknown] {
            assert!(!asks_aside(kind), "{kind:?}");
        }
    }

    pub(crate) fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gerdr-cmds-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    pub(crate) fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    pub(crate) fn names(list: &[Command]) -> Vec<&str> {
        list.iter().map(|c| c.name.as_str()).collect()
    }

    pub(crate) fn find<'a>(list: &'a [Command], name: &str) -> &'a Command {
        list.iter().find(|c| c.name == name).unwrap()
    }

    #[test]
    fn a_command_file_wins_over_a_builtin_of_its_name() {
        let tmp = scratch("builtin");
        let homes = Homes::under(&tmp);
        write(
            &tmp.join("w/.claude/commands/review.md"),
            "---\ndescription: Mine\n---\n",
        );
        let list = list(AgentKind::Claude, &tmp.join("w"), &homes, &tmp);
        assert_eq!(find(&list, "review").about, "Mine");
    }

    #[test]
    fn opencode_cursor_and_codex_read_their_own_places() {
        let tmp = scratch("others");
        let (home, cwd) = (tmp.join("home"), tmp.join("work"));
        let homes = Homes::under(&home);
        let config = home.join(".config");
        write(
            &config.join("opencode/commands/tidy.md"),
            "---\ndescription: Tidy up\n---\n",
        );
        write(&cwd.join(".opencode/command/ci/fix.md"), "");
        write(
            &config.join("opencode/opencode.json"),
            r#"{"command": {"test": {"template": "run", "description": "Run tests"}}}"#,
        );
        let open = list(AgentKind::OpenCode, &cwd, &homes, &config);
        assert_eq!(find(&open, "tidy").about, "Tidy up");
        assert_eq!(find(&open, "test").about, "Run tests");
        assert!(find(&open, "models").about.contains("model"));
        // Only files right in the folder.
        assert!(!names(&open).iter().any(|n| n.contains("fix")));

        write(
            &cwd.join(".cursor/commands/lint.md"),
            "# Lint the code\n\nmore",
        );
        write(
            &home.join(".cursor/skills-cursor/create-rule/SKILL.md"),
            "---\ndescription: Rule\n---\n",
        );
        let cursor = list(AgentKind::Cursor, &cwd, &homes, &config);
        assert_eq!(find(&cursor, "lint").about, "Lint the code");
        assert_eq!(find(&cursor, "create-rule").about, "Rule");

        write(
            &home.join(".codex/prompts/draft.md"),
            "---\ndescription: Draft\n---\n",
        );
        let codex = list(AgentKind::Codex, &cwd, &homes, &config);
        assert_eq!(find(&codex, "prompts:draft").about, "Draft");

        assert!(list(AgentKind::Unknown, &cwd, &homes, &config).is_empty());
    }

    fn cmd(name: &str) -> Command {
        Command {
            name: name.into(),
            about: String::new(),
            hint: String::new(),
        }
    }

    #[test]
    fn pi_and_antigravity_read_their_own_places() {
        let tmp = scratch("pi-agy");
        let (home, cwd) = (tmp.join("home"), tmp.join("work"));
        let homes = Homes::under(&home);
        let agent = home.join(".pi/agent");
        let review = "---\nargument-hint: \"[focus]\"\n---\n\nReview it\n";
        write(&agent.join("prompts/review.md"), review);
        write(
            &cwd.join(".pi/prompts/ship.md"),
            "---\ndescription: Ship\n---\n",
        );
        write(
            &home.join(".agents/skills/pdf/SKILL.md"),
            "---\nname: pdf-tools\n---\n",
        );
        let pi = list(AgentKind::Pi, &cwd, &homes, &tmp);
        let review = find(&pi, "review");
        assert_eq!(
            (review.about.as_str(), review.hint.as_str()),
            ("Review it", "[focus]")
        );
        assert_eq!(find(&pi, "ship").about, "Ship");
        assert!(names(&pi).contains(&"skill:pdf-tools"));
        assert!(names(&pi).contains(&"tree"));

        let cli = home.join(".gemini/antigravity-cli");
        let guide = "---\nname: antigravity-guide\ndescription: Ask\n---\n";
        write(&cli.join("builtin/skills/guide/SKILL.md"), guide);
        write(
            &cli.join("builtin/skills/pane/SKILL.md"),
            "---\ndisable-slash-command: true\n---\n",
        );
        write(
            &home.join(".gemini/config/plugins/dart/skills/test/SKILL.md"),
            "",
        );
        write(
            &cwd.join(".agents/skills/deploy/SKILL.md"),
            "---\ndescription: Deploy\n---\n",
        );
        let agy = list(AgentKind::Antigravity, &cwd, &homes, &tmp);
        assert_eq!(find(&agy, "antigravity-guide").about, "Ask");
        assert_eq!(find(&agy, "deploy").about, "Deploy");
        assert!(names(&agy).contains(&"test"));
        assert!(!names(&agy).contains(&"pane"));
        assert!(names(&agy).contains(&"rewind"));
    }

    #[test]
    fn the_menu_filters_picks_and_keeps_the_picked_row_in_sight() {
        assert_eq!(query("/co"), Some("co"));
        assert_eq!(query("/"), Some(""));
        assert_eq!(query("/compact now"), None);
        assert_eq!(query("hi /co"), None);

        let mut slash = Slash::new(["clear", "compact", "config", "cost"].map(cmd).to_vec());
        slash.sync("");
        assert_eq!(slash.shown.len(), 4);
        slash.sync("cmp");
        assert_eq!(slash.picked().unwrap().name, "compact");
        slash.step(5);
        assert_eq!(slash.picked, 0);
        slash.sync("xyz");
        assert!(!slash.open());

        let many: Vec<Command> = (0..20).map(|i| cmd(&format!("c{i:02}"))).collect();
        let mut slash = Slash::new(many);
        slash.sync("");
        slash.step(10);
        let (first, rows) = slash.in_sight();
        assert_eq!((first, rows.len()), (3, ROWS));
        slash.step(-20);
        assert_eq!(slash.in_sight().0, 0);
        slash.hidden = true;
        slash.sync("");
        assert!(!slash.open(), "the same query keeps it closed");
        slash.sync("c1");
        assert!(slash.open(), "typing on opens it again");
    }
}

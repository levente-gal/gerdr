//! A feature's work on another machine: where its paths are there, and how
//! its code and files get there and back.
//!
//! Other users have other homes, so paths are translated at the boundary
//! (`Paths`): what an agent there reads names that machine's folders, and the
//! records here keep naming this one's. A repository goes to the same place
//! under the home there, unless the project's file says another
//! (`Workflow::machine_folders`). None there yet, gerdr makes it and pushes what
//! the feature needs: no clone, so that machine needs no login to the
//! repository's host, and it has nothing to push to by itself.
//!
//! Git carries code: `refs/gerdr/…` are pushed, then fast-forwarded into the
//! worktree there, which keeps the branch it can be on; what the agent commits
//! comes back the same way. rsync carries the feature's data folder, all but
//! its record, which only this machine writes.
//!
//! Everything but `Paths` runs ssh, git or rsync: off the UI thread (D3).

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::AtomicBool,
    time::Duration,
};

use super::{Feature, hook, store};
use crate::{desktop::shell_quote, machines};

/// How long pushing or fetching code may take: a first push sends all of a
/// repository's history.
const CODE: Duration = Duration::from_secs(30 * 60);
/// How long a copy of the data folder may take.
const DATA: Duration = Duration::from_secs(5 * 60);
/// How long a script there may take.
const SCRIPT: Duration = Duration::from_secs(60);
/// Where a folder outside the home goes, under the home there.
const OUTSIDE_HOME: &str = "gerdr-work";

/// A machine's folders that paths are translated between.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Side {
    pub home: String,
    /// gerdr's data folder (`projects::data_dir`).
    pub data: String,
}

/// How this machine's paths read on another.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Paths {
    /// From and to, the longest first: the most exact one wins.
    pairs: Vec<(String, String)>,
}

/// Whether `c` can go on a file's name: a path ending right before it isn't
/// the path being looked for.
fn in_name(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

impl Paths {
    fn of(pairs: Vec<(String, String)>) -> Self {
        let mut pairs: Vec<(String, String)> = pairs
            .into_iter()
            .map(|(from, to)| {
                let trim = |s: &str| match s.trim_end_matches('/') {
                    "" => "/".to_owned(),
                    s => s.to_owned(),
                };
                (trim(&from), trim(&to))
            })
            .collect();
        pairs.sort_by_key(|(from, _)| std::cmp::Reverse(from.len()));
        pairs.dedup_by(|a, b| a.0 == b.0);
        Self { pairs }
    }

    /// The translation between `here` and `there`: gerdr's data folder, the
    /// folders the project's file names (`folders`, this machine's to that
    /// one's), then the home. What `feature` holds outside the home goes to a
    /// folder of gerdr's under the home there.
    pub fn new(
        here: &Side,
        there: &Side,
        folders: &BTreeMap<String, String>,
        feature: Option<&Feature>,
    ) -> Self {
        let mut pairs = vec![
            (here.home.clone(), there.home.clone()),
            (here.data.clone(), there.data.clone()),
        ];
        pairs.extend(folders.iter().map(|(f, t)| (f.clone(), t.clone())));
        let outside = feature.into_iter().flat_map(|feature| {
            let repos = feature.repos.iter().map(|r| r.main.as_path());
            let links = feature.links.iter().map(|l| l.root.as_path());
            let context = feature.context.iter().map(|c| c.main.as_path());
            std::iter::once(feature.root.as_path())
                .chain(repos)
                .chain(links)
                .chain(context)
        });
        for folder in outside {
            let covered = pairs
                .iter()
                .any(|(from, _)| folder.starts_with(Path::new(from)));
            if !covered && let Some(path) = folder.to_str() {
                let to = format!(
                    "{}/{OUTSIDE_HOME}/{}",
                    there.home.trim_end_matches('/'),
                    path.trim_start_matches('/')
                );
                pairs.push((path.to_owned(), to));
            }
        }
        Self::of(pairs)
    }

    /// `path` as it reads there. One outside all it knows stays as it is.
    pub fn map(&self, path: &Path) -> PathBuf {
        for (from, to) in &self.pairs {
            if let Ok(rest) = path.strip_prefix(from) {
                return match rest.as_os_str().is_empty() {
                    true => PathBuf::from(to),
                    false => Path::new(to).join(rest),
                };
            }
        }
        path.to_path_buf()
    }

    /// `path` there, as text for a script.
    pub fn there(&self, path: &Path) -> String {
        self.map(path).to_string_lossy().into_owned()
    }

    /// The way back: what reads there as it reads here. For what comes from
    /// there to be read here, like a check's output.
    pub fn back(&self) -> Self {
        Self::of(
            (self.pairs.iter())
                .map(|(from, to)| (to.clone(), from.clone()))
                .collect(),
        )
    }

    /// `text` with the paths it names as they read there: a prompt, a command
    /// or a line typed to an agent. Whole paths only, and in one pass, so a
    /// home inside the other home isn't translated twice.
    pub fn text(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        let mut before = None;
        while let Some(c) = rest.chars().next() {
            let starts = !before.is_some_and(|b: char| in_name(b) || b == '/' || b == '.');
            let hit = (self.pairs.iter())
                .filter(|_| starts && c == '/')
                .find(|(from, _)| {
                    rest.strip_prefix(from.as_str()).is_some_and(|after| {
                        let mut next = after.chars();
                        match next.next() {
                            None => true,
                            // The end of a sentence, or a file's extension.
                            Some('.') => !next.next().is_some_and(in_name),
                            Some(c) => !in_name(c),
                        }
                    })
                });
            if let Some((from, to)) = hit {
                out.push_str(to);
                rest = &rest[from.len()..];
                before = from.chars().next_back();
                continue;
            }
            out.push(c);
            rest = &rest[c.len_utf8()..];
            before = Some(c);
        }
        out
    }
}

/// A repository of a feature, here and there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    /// `Repo::key`.
    pub key: String,
    pub local_main: PathBuf,
    /// None for a context repository: it has no worktree, only its main.
    pub local_worktree: Option<PathBuf>,
    pub main: String,
    pub worktree: Option<String>,
    /// `feature/<slug>`, for a repository the feature changes.
    pub branch: Option<String>,
    pub base: String,
    /// The commit it goes from; for a context repository what it is at here.
    pub commit: String,
    /// Where it pushes this commit to there.
    pub commit_ref: String,
}

/// Where `feature`'s repositories are there.
pub fn places(feature: &Feature, paths: &Paths) -> Vec<Place> {
    let slug = &feature.slug;
    let repos = feature.repos.iter().map(|r| Place {
        key: r.key(),
        local_main: r.main.clone(),
        local_worktree: Some(r.worktree.clone()),
        main: paths.there(&r.main),
        worktree: Some(paths.there(&r.worktree)),
        branch: r.branch.clone().filter(|_| r.touched),
        base: r.base.clone(),
        commit: r.base_commit.clone(),
        commit_ref: format!("refs/gerdr/base/{slug}"),
    });
    let context = feature.context.iter().map(|c| Place {
        key: c.key(),
        local_main: c.main.clone(),
        local_worktree: None,
        main: paths.there(&c.main),
        worktree: None,
        branch: None,
        base: String::new(),
        commit: "HEAD".to_owned(),
        commit_ref: format!("refs/gerdr/context/{slug}"),
    });
    repos.chain(context).collect()
}

/// The feature folders there the docs link goes in: its own, each link's.
pub fn doc_dirs(feature: &Feature, paths: &Paths) -> Vec<String> {
    std::iter::once(feature.dir.as_path())
        .chain(feature.links.iter().map(|l| l.dir.as_path()))
        .map(|dir| paths.there(dir))
        .collect()
}

/// The script making the repositories there, or finding them: `places`, each
/// with its root commit here. Nothing is overwritten: a folder that holds
/// something else ends it, saying `OCCUPIED <folder>`.
pub fn ensure_script(places: &[(Place, String)]) -> String {
    let mut script = String::from(
        r#"ensure() {
  m=$1; root=$2
  if [ ! -e "$m" ]; then
    mkdir -p "$(dirname "$m")" && git init -q "$m" || exit 4
  elif [ ! -e "$m/.git" ]; then
    if [ -n "$(ls -A "$m" | grep -vxF .features)" ]; then echo "OCCUPIED $m"; exit 3; fi
    git init -q "$m" || exit 4
  fi
  roots=$(git -C "$m" rev-list --max-parents=0 --all 2>/dev/null)
  if [ -n "$roots" ] && ! printf '%s\n' "$roots" | grep -qxF "$root"; then
    echo "OCCUPIED $m"; exit 3
  fi
}
"#,
    );
    for (place, root) in places {
        script.push_str(&format!(
            "ensure {} {}\n",
            shell_quote(&place.main),
            shell_quote(root)
        ));
    }
    script
}

/// The ref holding what a feature's branch `branch` left, shown here and written
/// on another machine.
pub fn view_base_ref(branch: &str) -> String {
    format!("refs/gerdr/view-base/{branch}")
}

/// The ref holding what the worktree of repository `key` of feature `slug` is at
/// on the machine that writes it.
pub fn view_head_ref(slug: &str, key: &str) -> String {
    format!("refs/gerdr/view-head/{slug}/{}", key.replace(':', "-"))
}

/// The script saying where a feature's repositories are on the machine that
/// writes it: a line per repository with its key, the commit of its worktree and
/// that of what it started from, the name first as `layout::base_ref` goes.
pub fn heads_script(places: &[Place]) -> String {
    let mut script = String::from(
        r#"st() {
  h=""
  [ -d "$3" ] && h=$(git -C "$3" rev-parse -q --verify HEAD)
  b=$(git -C "$2" rev-parse -q --verify "$4^{commit}" ||
      git -C "$2" rev-parse -q --verify "origin/$4^{commit}" || echo "$5")
  printf '%s	%s	%s
' "$1" "$h" "$b"
}
"#,
    );
    for place in places {
        if let Some(worktree) = &place.worktree {
            script.push_str(&format!(
                "st {} {} {} {} {}
",
                shell_quote(&place.key),
                shell_quote(&place.main),
                shell_quote(worktree),
                shell_quote(&place.base),
                shell_quote(&place.commit),
            ));
        }
    }
    script
}

/// What `heads_script` said: by key, the worktree's commit (empty with none
/// there) and the one it started from.
pub fn parse_heads(text: &str) -> BTreeMap<String, (String, String)> {
    (text.lines())
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let (key, head, base) = (parts.next()?, parts.next()?, parts.next()?);
            Some((key.to_owned(), (head.to_owned(), base.to_owned())))
        })
        .collect()
}

/// The script placing a worktree here at commit ref `name`, for a look at a
/// feature written on another machine: it is on no branch, as nothing is to be
/// committed in it. Changes made in it are the user's: what's in their way ends
/// it, saying `INWAY <folder>`.
pub fn view_worktree_script(main: &Path, worktree: &Path, name: &str) -> String {
    let main = shell_quote(&main.to_string_lossy());
    let worktree = shell_quote(&worktree.to_string_lossy());
    let name = shell_quote(name);
    format!(
        r#"m={main}; w={worktree}; r={name}
f=$(git -C "$m" rev-parse --git-path info/exclude) && case "$f" in /*) ;; *) f="$m/$f";; esac
mkdir -p "$(dirname "$f")" && {{ grep -qxF .features/ "$f" 2>/dev/null || echo .features/ >> "$f"; }}
if [ ! -e "$w/.git" ]; then
  git -C "$m" worktree prune
  mkdir -p "$(dirname "$w")"
  git -C "$m" worktree add -q --detach "$w" "$r" || exit 5
else
  h=$(git -C "$w" rev-parse -q --verify HEAD)
  t=$(git -C "$m" rev-parse -q --verify "$r")
  if [ "$h" != "$t" ]; then
    git -C "$w" checkout -q --detach "$t" || {{ echo "INWAY $w"; exit 6; }}
  fi
fi
"#
    )
}

/// Run `script` for `sh` here, off the UI thread.
pub fn local_script(script: &str, cancel: &AtomicBool) -> Result<String, Trouble> {
    let mut sh = Command::new("sh");
    sh.arg("-s");
    for var in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"] {
        sh.env_remove(var);
    }
    let out = machines::run_with_input(sh, script.as_bytes(), SCRIPT, cancel)
        .map_err(|failure| Trouble::Work(failure.why()))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let said = String::from_utf8_lossy(&out.stdout);
    if let Some(folder) = said.lines().find_map(|l| l.strip_prefix("OCCUPIED ")) {
        return Err(Trouble::Work(format!(
            "{folder} holds something else: name a folder for it under machine_folders in the project's file"
        )));
    }
    if let Some(folder) = said.lines().find_map(|l| l.strip_prefix("INWAY ")) {
        return Err(Trouble::Work(format!(
            "changes in {folder} are in the way of what came"
        )));
    }
    Err(trouble("A command here failed", &out))
}

/// The script setting up the worktrees there once the code is pushed: each on
/// its branch, at what was pushed or ahead of it, or read-only at its commit.
/// A worktree gone another way than what was pushed ends it (`merge` says
/// so). The guard goes on the branch as it does here: an agent there doesn't
/// push it by itself.
pub fn worktree_script(places: &[Place], docs: &[String], data: &str) -> String {
    let mut script = format!(
        r#"exclude() {{
  ( cd "$1" && f=$(git rev-parse --git-path info/exclude) && mkdir -p "$(dirname "$f")" &&
    {{ grep -qxF "$2" "$f" 2>/dev/null || echo "$2" >> "$f"; }} )
}}
guard() {{
  ( cd "$1" || exit 1
    h=$(git rev-parse --git-path hooks) && mkdir -p "$h" || exit 1
    if [ -f "$h/pre-push" ] && ! grep -q 'gerdr feature guard' "$h/pre-push"; then
      mv "$h/pre-push" "$h/{chained}"
    fi
    cat > "$h/pre-push" <<'GERDR_HOOK'
{hook}GERDR_HOOK
    chmod +x "$h/pre-push"
    g="$(git rev-parse --git-common-dir)/gerdr/guard/$2"
    mkdir -p "$(dirname "$g")" && {{ [ -f "$g" ] || : > "$g"; }} )
}}
branch() {{
  m=$1; w=$2; b=$3
  git -C "$m" rev-parse -q --verify "refs/heads/$b" >/dev/null ||
    git -C "$m" branch "$b" "refs/gerdr/$b" || exit 5
  if [ ! -e "$w/.git" ]; then
    git -C "$m" worktree prune
    mkdir -p "$(dirname "$w")"
    git -C "$m" worktree add -q "$w" "$b" || exit 5
  fi
  git -C "$w" merge -q --ff-only "refs/gerdr/$b" || exit 6
  guard "$w" "$b" || exit 5
}}
detached() {{
  m=$1; w=$2; c=$3
  if [ ! -e "$w/.git" ]; then
    git -C "$m" worktree prune
    mkdir -p "$(dirname "$w")"
    git -C "$m" worktree add -q --detach "$w" "$c" || exit 5
  fi
}}
context() {{
  m=$1; c=$2
  if ! git -C "$m" rev-parse -q --verify HEAD >/dev/null ||
     ! git -C "$m" symbolic-ref -q HEAD >/dev/null; then
    git -C "$m" checkout -q --detach "$c" || exit 5
  fi
}}
"#,
        chained = hook::CHAINED,
        hook = hook::HOOK,
    );
    for place in places {
        let main = shell_quote(&place.main);
        if !place.base.is_empty() {
            // What a prompt says the feature starts from has its name there
            // too. One there already, moved on or not, is left alone.
            script.push_str(&format!(
                "git -C {main} rev-parse -q --verify refs/heads/{base} >/dev/null || git -C {main} branch {base} {commit} || exit 5\n",
                base = shell_quote(&place.base),
                commit = shell_quote(&place.commit_ref),
            ));
        }
        script.push_str(&format!("exclude {main} .features/\n"));
        script.push_str(&format!("exclude {main} {}/\n", super::graph::OUT_DIR));
        script.push_str(&format!("exclude {main} /{}\n", super::layout::DOCS_LINK));
        match (&place.worktree, &place.branch) {
            (Some(worktree), Some(branch)) => script.push_str(&format!(
                "branch {main} {} {}\n",
                shell_quote(worktree),
                shell_quote(branch),
            )),
            (Some(worktree), None) => script.push_str(&format!(
                "detached {main} {} {}\n",
                shell_quote(worktree),
                shell_quote(&place.commit_ref),
            )),
            (None, _) => script.push_str(&format!(
                "context {main} {}\n",
                shell_quote(&place.commit_ref)
            )),
        }
    }
    let data = shell_quote(data);
    script.push_str(&format!("mkdir -p {data}\n"));
    for dir in docs {
        script.push_str(&format!(
            "mkdir -p {dir} && ln -sfn {data} {dir}/{link}\n",
            dir = shell_quote(dir),
            link = super::layout::DOCS_LINK,
        ));
    }
    script
}

/// The script taking a removed feature off the machine: its worktrees, branch
/// and files. The repositories stay: their history saves the next feature a
/// long first push. What can't go is said in its own words, and the rest still
/// goes.
pub fn forget_script(places: &[Place], docs: &[String], data: &str) -> String {
    let mut script = String::new();
    for place in places {
        let main = shell_quote(&place.main);
        if let Some(worktree) = &place.worktree {
            script.push_str(&format!(
                "git -C {main} worktree remove --force {}\n",
                shell_quote(worktree)
            ));
        }
        if let Some(branch) = &place.branch {
            let branch = shell_quote(branch);
            script.push_str(&format!("git -C {main} branch -q -D {branch}\n"));
            script.push_str(&format!(
                "git -C {main} update-ref -d refs/gerdr/{branch}\n"
            ));
        }
        script.push_str(&format!(
            "git -C {main} update-ref -d {}\n",
            shell_quote(&place.commit_ref)
        ));
    }
    for dir in docs {
        let dir = shell_quote(dir);
        script.push_str(&format!(
            "rm -f {dir}/{link}; rmdir {dir} 2>/dev/null\n",
            link = super::layout::DOCS_LINK
        ));
    }
    script.push_str(&format!("rm -rf {}\n", shell_quote(data)));
    script
}

/// The script saying where the worktrees there stand: a line per repository,
/// its key, its commit and `dirty` if changes aren't committed.
pub fn status_script(places: &[Place]) -> String {
    let mut script = String::from(
        r#"st() {
  [ -d "$2" ] || return 0
  h=$(git -C "$2" rev-parse -q --verify HEAD)
  d=$(git -C "$2" status --porcelain | head -n 1)
  printf '%s\t%s\t%s\n' "$1" "$h" "${d:+dirty}"
}
"#,
    );
    for place in places {
        if let Some(worktree) = &place.worktree {
            script.push_str(&format!(
                "st {} {}\n",
                shell_quote(&place.key),
                shell_quote(worktree)
            ));
        }
    }
    script
}

/// What a terminal there needs before a check or a setup command, in `sh`: the
/// virtual environment of its folder, as a new terminal here gets. Nothing tells
/// `sh` about one there otherwise, and a setup builds it in the worktree.
pub const ACTIVATE: &str = "for v in .venv venv; do if [ -f \"$v/pyvenv.cfg\" ] && [ -f \"$v/bin/activate\" ]; then . \"$v/bin/activate\"; break; fi; done\n";

/// The script saying what's in folder `dir`, of the files matching `patterns`:
/// the clock there on the first line, then a line per file, its name, when it
/// was written and its first line. No folder yet reads as no files.
pub fn look_script(dir: &str, patterns: &str) -> String {
    format!(
        r#"date +%s
cd {} 2>/dev/null || exit 0
for f in {patterns}; do
  [ -e "$f" ] || continue
  printf '%s\t%s\t%s\n' "$f" "$(stat -c %Y "$f")" "$(head -n 1 "$f")"
done
"#,
        shell_quote(dir),
    )
}

/// What `look_script` said.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Look {
    /// The clock there, in Unix seconds.
    pub now: u64,
    /// By file name: when it was written, and its first line.
    pub files: BTreeMap<String, (u64, String)>,
}

impl Look {
    /// `None` when `text` isn't what the script says: not to be taken for a
    /// folder with nothing in it.
    pub fn parse(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        let now = lines.next()?.trim().parse().ok()?;
        let mut files = BTreeMap::new();
        for line in lines {
            let mut parts = line.splitn(3, '\t');
            let (Some(name), Some(at)) = (parts.next(), parts.next()) else {
                continue;
            };
            let Ok(at) = at.parse() else {
                continue;
            };
            let first = parts.next().unwrap_or_default().to_owned();
            files.insert(name.to_owned(), (at, first));
        }
        Some(Self { now, files })
    }

    /// The number in file `name`, once it is there whole.
    pub fn number(&self, name: &str) -> Option<i32> {
        self.files.get(name)?.1.trim().parse().ok()
    }

    /// When file `name` was written.
    pub fn written(&self, name: &str) -> Option<u64> {
        self.files.get(name).map(|(at, _)| *at)
    }
}

/// The script sending `signal` (a name, like `TERM`) to process group `group`
/// there. One gone is fine: what is wanted of it is that it ended.
pub fn kill_group_script(group: i32, signal: &str) -> String {
    format!("kill -s {signal} -- -{group} 2>/dev/null\nexit 0\n")
}

/// `kill_group_script`, for the group process `pid` is in: its shell's job.
/// Group 0 and 1 are never meant, whatever `ps` says.
pub fn kill_group_of_script(pid: i32, signal: &str) -> String {
    format!(
        "g=$(ps -o pgid= -p {pid} 2>/dev/null | tr -d ' ')\nif [ -n \"$g\" ] && [ \"$g\" -gt 1 ]; then kill -s {signal} -- \"-$g\" 2>/dev/null; fi\nexit 0\n"
    )
}

/// The script saying which of `items` worktree `worktree` there doesn't have, a
/// line each.
pub fn missing_script(worktree: &str, items: &[String]) -> String {
    let mut script = format!("cd {} || exit 1\n", shell_quote(worktree));
    for item in items {
        let item = shell_quote(item);
        script.push_str(&format!(
            "if [ ! -e {item} ] && [ ! -L {item} ]; then printf '%s\\n' {item}; fi\n"
        ));
    }
    script
}

/// The script taking from worktree `worktree` there what `items` git doesn't
/// ignore on its branch, saying each on a line: it would show as a change.
pub fn unignored_script(worktree: &str, items: &[String]) -> String {
    let mut script = format!("cd {} || exit 1\n", shell_quote(worktree));
    for item in items {
        let item = shell_quote(item);
        script.push_str(&format!(
            "if [ -n \"$(git --literal-pathspecs status --porcelain -- {item})\" ]; then rm -rf -- {item}; printf '%s\\n' {item}; fi\n"
        ));
    }
    script
}

/// A worktree there, as `status_script` says it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Standing {
    pub key: String,
    pub head: String,
    pub dirty: bool,
}

pub fn parse_status(text: &str) -> Vec<Standing> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            Some(Standing {
                key: parts.next()?.to_owned(),
                head: parts.next()?.to_owned(),
                dirty: parts.next() == Some("dirty"),
            })
        })
        .collect()
}

/// What keeps work from going there or coming back: said to the user. Lost
/// connections aren't told apart here: the link says those.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trouble {
    /// The connection failed, or the command didn't get to run.
    Link(String),
    /// Something the user can act on.
    Work(String),
}

impl Trouble {
    pub fn text(&self) -> &str {
        match self {
            Self::Link(text) | Self::Work(text) => text,
        }
    }
}

/// Why `what` failed, from what it said; SSH's own exit code is 255, as is
/// rsync's error for a connection (its 12 and 255).
fn trouble(what: &str, out: &std::process::Output) -> Trouble {
    let said = String::from_utf8_lossy(&out.stderr);
    let line = said
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .unwrap_or("it ended without saying why")
        .to_owned();
    let lost = matches!(out.status.code(), Some(255 | 12));
    match lost {
        true => Trouble::Link(format!("{what}: {line}")),
        false => Trouble::Work(format!("{what}: {line}")),
    }
}

/// What the commands reaching a machine go through: its link's master.
#[derive(Clone, Debug)]
pub struct Reach {
    pub target: String,
    pub control: PathBuf,
}

impl Reach {
    /// The `ssh` command git and rsync run: over the master, and quick to
    /// give up with it gone. The master's socket decides where it goes.
    fn ssh_line(&self) -> String {
        format!(
            "ssh -S {} -o BatchMode=yes -o ControlMaster=no -o ConnectTimeout=5",
            shell_quote(&self.control.to_string_lossy())
        )
    }

    /// The target as git and rsync write a host: no `ssh://`, and no port.
    /// The master has connected already.
    fn host(&self) -> &str {
        let target = self.target.strip_prefix("ssh://").unwrap_or(&self.target);
        match target.rsplit_once(':') {
            Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => host,
            _ => target,
        }
    }

    /// `path` there, as git names a remote.
    fn git_url(&self, path: &str) -> String {
        // git decodes these as a URL's.
        format!("ssh://{}{}", self.host(), path.replace('%', "%25"))
    }

    /// Run `script` for `sh` there, given on its input: a login shell's
    /// quoting, whichever it is, never gets to read it.
    pub fn script(&self, script: &str, cancel: &AtomicBool) -> Result<String, Trouble> {
        let mut ssh = machines::over_master(&self.control, &self.target);
        ssh.arg("sh -s");
        let out = machines::run_with_input(ssh, script.as_bytes(), SCRIPT, cancel)
            .map_err(|failure| Trouble::Link(failure.why()))?;
        if out.status.success() {
            return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
        }
        let said = String::from_utf8_lossy(&out.stdout);
        if let Some(folder) = said.lines().find_map(|l| l.strip_prefix("OCCUPIED ")) {
            return Err(Trouble::Work(format!(
                "{folder} holds something else: name a folder for it under machine_folders in the project's file"
            )));
        }
        Err(trouble("A command there failed", &out))
    }

    /// `command` there, over the master and reading and writing what is given
    /// to it, and quick to give up with the master gone.
    pub fn relay(&self, command: &str) -> Command {
        let mut ssh = Command::new("ssh");
        ssh.arg("-S")
            .arg(&self.control)
            .args(["-o", "BatchMode=yes", "-o", "ControlMaster=no"])
            .args(["-o", "ConnectTimeout=5"])
            .arg(&self.target)
            .arg(command);
        ssh
    }

    fn git(&self, dir: &Path) -> Command {
        let mut git = Command::new("git");
        for var in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"] {
            git.env_remove(var);
        }
        git.env("GIT_SSH_COMMAND", self.ssh_line())
            .arg("-C")
            .arg(dir);
        git
    }

    /// The first root commit of what `place` starts from, here.
    fn root_of(&self, place: &Place, cancel: &AtomicBool) -> Result<String, Trouble> {
        let mut git = self.git(&place.local_main);
        git.args(["rev-list", "--max-parents=0", &place.commit]);
        let out = machines::run(git, SCRIPT, cancel).map_err(|f| Trouble::Work(f.why()))?;
        if !out.status.success() {
            return Err(trouble(&format!("Couldn't read {}", place.key), &out));
        }
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .map(str::to_owned)
            .ok_or_else(|| Trouble::Work(format!("{} has no commits", place.key)))
    }

    /// Bring `feature`'s code there: its repositories, branches, worktrees
    /// and link to its files.
    pub fn push_code(
        &self,
        feature: &Feature,
        paths: &Paths,
        data: &str,
        cancel: &AtomicBool,
    ) -> Result<(), Trouble> {
        let places = places(feature, paths);
        let mut rooted = Vec::with_capacity(places.len());
        for place in &places {
            rooted.push((place.clone(), self.root_of(place, cancel)?));
        }
        self.script(&ensure_script(&rooted), cancel)?;
        for place in &places {
            let mut git = self.git(&place.local_main);
            git.args(["push", "-q", "-f", "--no-verify"])
                .arg(self.git_url(&place.main))
                .arg(format!("{}:{}", place.commit, place.commit_ref));
            if let Some(branch) = &place.branch {
                git.arg(format!("refs/heads/{branch}:refs/gerdr/{branch}"));
            }
            let out = machines::run(git, CODE, cancel).map_err(|f| Trouble::Link(f.why()))?;
            if !out.status.success() {
                return Err(trouble(&format!("Couldn't push {}", place.key), &out));
            }
        }
        let docs = doc_dirs(feature, paths);
        self.script(&worktree_script(&places, &docs, data), cancel)
            .map(drop)
    }

    /// Bring what was committed there here, for what the worktrees there
    /// `say`, by fast-forward: what was committed here since, or changes in
    /// the way, are for the user to sort out.
    pub fn fetch_code(
        &self,
        places: &[Place],
        say: &[Standing],
        cancel: &AtomicBool,
    ) -> Result<(), Trouble> {
        for standing in say.iter().filter(|s| !s.head.is_empty()) {
            let Some(place) = (places.iter()).find(|p| p.key == standing.key) else {
                continue;
            };
            let (Some(worktree), Some(branch)) = (&place.local_worktree, &place.branch) else {
                continue;
            };
            let mut head = self.git(worktree);
            head.args(["rev-parse", "HEAD"]);
            let here = machines::run(head, SCRIPT, cancel).map_err(|f| Trouble::Work(f.why()))?;
            if String::from_utf8_lossy(&here.stdout).trim() == standing.head {
                continue;
            }
            let mut fetch = self.git(worktree);
            fetch
                .args(["fetch", "-q"])
                .arg(self.git_url(&place.main))
                .arg(format!("refs/heads/{branch}"));
            let out = machines::run(fetch, CODE, cancel).map_err(|f| Trouble::Link(f.why()))?;
            if !out.status.success() {
                return Err(trouble(&format!("Couldn't fetch {}", place.key), &out));
            }
            let mut merge = self.git(worktree);
            merge.args([
                "-c",
                "core.hooksPath=/dev/null",
                "merge",
                "-q",
                "--ff-only",
                "FETCH_HEAD",
            ]);
            let out = machines::run(merge, SCRIPT, cancel).map_err(|f| Trouble::Work(f.why()))?;
            if !out.status.success() {
                return Err(Trouble::Work(format!(
                    "{}'s commits there can't be fast-forwarded here: it changed here too",
                    place.key
                )));
            }
        }
        Ok(())
    }

    /// Bring the features folder `there` here as it is there, records too: what
    /// is here goes by it, so a stray change here doesn't last. What's in `skip`
    /// (folders of that one) doesn't come. Evidence can name secrets: only this
    /// account reads it here.
    pub fn mirror(
        &self,
        there: &str,
        local: &Path,
        skip: &[String],
        cancel: &AtomicBool,
    ) -> Result<(), Trouble> {
        let mut rsync = Command::new("rsync");
        rsync
            .args(["-rlts", "--delete", "--chmod=D700,F600"])
            .arg("-e")
            .arg(self.ssh_line());
        for folder in skip {
            rsync.arg("--exclude").arg(format!("/{folder}/"));
        }
        rsync
            .arg(self.rsync_path(there))
            .arg(format!("{}/", local.display()));
        self.run_rsync("Couldn't copy the features from there", rsync, cancel)
    }

    /// Bring here the code of `feature`, as the machine it is written on has it,
    /// by `places` (`there` is that machine). `known` says what is here already;
    /// whether anything came. All that's made here is the repository if there is
    /// none, its refs for this and a worktree at them.
    pub fn view_code(
        &self,
        feature: &Feature,
        places: &[Place],
        known: &mut BTreeMap<String, (String, String)>,
        cancel: &AtomicBool,
    ) -> Result<bool, Trouble> {
        let places: Vec<&Place> = places.iter().filter(|p| p.worktree.is_some()).collect();
        let owned: Vec<Place> = places.iter().map(|p| (*p).clone()).collect();
        let said = parse_heads(&self.script(&heads_script(&owned), cancel)?);
        let mut came = false;
        for place in places {
            let Some((head, base)) = said.get(&place.key) else {
                continue;
            };
            let at = (head.clone(), base.clone());
            let worktree = place.local_worktree.as_deref().unwrap_or(Path::new(""));
            if known.get(&place.key) == Some(&at) && worktree.join(".git").exists() {
                continue;
            }
            if base.is_empty() {
                return Err(Trouble::Work(format!("{} has no commits there", place.key)));
            }
            if !known.contains_key(&place.key) {
                let root = self.script(
                    &format!(
                        "git -C {} rev-list --max-parents=0 {} | head -n 1
",
                        shell_quote(&place.main),
                        shell_quote(base),
                    ),
                    cancel,
                )?;
                let here = Place {
                    main: place.local_main.to_string_lossy().into_owned(),
                    ..place.clone()
                };
                local_script(
                    &ensure_script(&[(here, root.trim().to_owned())]),
                    cancel,
                )?;
            }
            let head_at = if head.is_empty() { base } else { head };
            let head_ref = view_head_ref(&feature.slug, &place.key);
            let mut git = self.git(&place.local_main);
            git.args(["fetch", "-q", "--no-tags"])
                .arg(self.git_url(&place.main))
                .arg(format!("+{head_at}:{head_ref}"));
            if let Some(branch) = &place.branch {
                git.arg(format!("+{base}:{}", view_base_ref(branch)));
            }
            let out = machines::run(git, CODE, cancel).map_err(|f| Trouble::Link(f.why()))?;
            if !out.status.success() {
                return Err(trouble(&format!("Couldn't fetch {}", place.key), &out));
            }
            local_script(
                &view_worktree_script(&place.local_main, worktree, &head_ref),
                cancel,
            )?;
            known.insert(place.key.clone(), at);
            came = true;
        }
        Ok(came)
    }

    fn rsync(&self, delete: bool) -> Command {
        let mut rsync = Command::new("rsync");
        rsync
            .args(["-rlts", "--exclude", store::FILE])
            .arg("-e")
            .arg(self.ssh_line());
        if delete {
            rsync.arg("--delete");
        }
        rsync
    }

    /// `path` there, as rsync names it.
    fn rsync_path(&self, path: &str) -> String {
        format!("{}:{}/", self.host(), path.trim_end_matches('/'))
    }

    fn run_rsync(&self, what: &str, rsync: Command, cancel: &AtomicBool) -> Result<(), Trouble> {
        let out = match machines::run(rsync, DATA, cancel) {
            Ok(out) => out,
            Err(machines::Failure::NoSsh) => {
                return Err(Trouble::Work(
                    "rsync isn't installed on this machine".into(),
                ));
            }
            Err(failure) => return Err(Trouble::Link(failure.why())),
        };
        if out.status.success() {
            return Ok(());
        }
        Err(trouble(what, &out))
    }

    /// Copy the feature's data folder there. Only `exact` removes what isn't
    /// here, right before an agent starts: with one at work there it would
    /// remove what it just wrote, and without it only what is newer goes.
    /// Evidence can name secrets: only that account reads it there.
    pub fn push_data(
        &self,
        local: &Path,
        there: &str,
        exact: bool,
        cancel: &AtomicBool,
    ) -> Result<(), Trouble> {
        let mut rsync = self.rsync(exact);
        rsync.arg("--chmod=D700,F600");
        if !exact {
            rsync.arg("--update");
        }
        rsync
            .arg(format!("{}/", local.display()))
            .arg(self.rsync_path(there));
        self.run_rsync("Couldn't copy the feature's files there", rsync, cancel)
    }

    /// Copy the code graphs of `feature`'s folders there: its prompts name
    /// them. A help to its agent, so one that doesn't get there is no failure.
    pub fn push_graphs(&self, feature: &Feature, paths: &Paths, cancel: &AtomicBool) {
        for folder in super::graph::folders(feature) {
            let local = folder.dir.join(super::graph::OUT_DIR);
            if !local.is_dir() {
                continue;
            }
            let mut rsync = self.rsync(false);
            rsync
                .arg(format!("{}/", local.display()))
                .arg(self.rsync_path(&paths.there(&local)));
            if let Err(trouble) = self.run_rsync("graph", rsync, cancel) {
                tracing::debug!("copying a code graph: {}", trouble.text());
            }
        }
    }

    /// Copy what is newer there of the feature's data folder here.
    pub fn pull_data(&self, local: &Path, there: &str, cancel: &AtomicBool) -> Result<(), Trouble> {
        if let Err(err) = std::fs::create_dir_all(local) {
            return Err(Trouble::Work(format!(
                "Couldn't make {}: {err}",
                local.display()
            )));
        }
        let mut rsync = self.rsync(false);
        rsync
            .arg("--update")
            .arg(self.rsync_path(there))
            .arg(format!("{}/", local.display()));
        self.run_rsync("Couldn't copy the feature's files back", rsync, cancel)
    }

    /// Copy file `there` here as it is there: a log, which only there grows.
    pub fn pull_file(&self, local: &Path, there: &str, cancel: &AtomicBool) -> Result<(), Trouble> {
        if let Some(parent) = local.parent()
            && let Err(err) = std::fs::create_dir_all(parent)
        {
            return Err(Trouble::Work(format!(
                "Couldn't make {}: {err}",
                parent.display()
            )));
        }
        let mut rsync = Command::new("rsync");
        rsync
            .arg("-t")
            .arg("-e")
            .arg(self.ssh_line())
            .arg(format!("{}:{there}", self.host()))
            .arg(local);
        self.run_rsync("Couldn't copy a file back", rsync, cancel)
    }

    /// Copy `items` (paths inside `main`) to the worktree `there`, leaving
    /// what is there already. They are ignored files like `.env`: only that
    /// account reads them there.
    pub fn push_ignored(
        &self,
        main: &Path,
        there: &str,
        items: &[String],
        cancel: &AtomicBool,
    ) -> Result<(), Trouble> {
        let mut rsync = Command::new("rsync");
        rsync
            .args(["-rlpts", "--chmod=go-rwx", "--ignore-existing"])
            .args(["--from0", "--files-from=-"])
            .arg("-e")
            .arg(self.ssh_line())
            .arg(format!("{}/", main.display()))
            .arg(self.rsync_path(there));
        let list = items.join("\0");
        let out = match machines::run_with_input(rsync, list.as_bytes(), DATA, cancel) {
            Ok(out) => out,
            Err(machines::Failure::NoSsh) => {
                return Err(Trouble::Work(
                    "rsync isn't installed on this machine".into(),
                ));
            }
            Err(failure) => return Err(Trouble::Link(failure.why())),
        };
        if out.status.success() {
            return Ok(());
        }
        Err(trouble("Couldn't copy ignored files there", &out))
    }

    /// What's missing there for `program` to run, as what to say: git and
    /// rsync for commands, and `program` in a login shell's PATH.
    pub fn missing(&self, program: &str, cancel: &AtomicBool) -> Result<Option<String>, Trouble> {
        let script = format!(
            r#"for t in git rsync; do command -v "$t" >/dev/null 2>&1 || echo "$t"; done
"${{SHELL:-/bin/sh}}" -lc {} </dev/null >/dev/null 2>&1 || echo {}
"#,
            shell_quote(&format!("command -v {}", shell_quote(program))),
            shell_quote(program),
        );
        let out = self.script(&script, cancel)?;
        Ok(out.lines().next().map(str::to_owned))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn sh_c(script: &str) -> std::process::Output {
        Command::new("sh").arg("-c").arg(script).output().unwrap()
    }

    #[test]
    fn a_look_reads_files_by_the_clock_there() {
        let t = crate::feature::tests::Temp::new();
        std::fs::write(t.0.as_path().join("0.pid"), "77\n").unwrap();
        std::fs::write(t.0.as_path().join("0.exit"), "3\nmore\n").unwrap();
        std::fs::write(t.0.as_path().join("gerdr-check-0"), "script").unwrap();
        let dir = t.0.as_path().to_string_lossy().into_owned();
        let out = sh_c(&look_script(&dir, "*.pid *.queued *.exit"));
        let look = Look::parse(&String::from_utf8_lossy(&out.stdout)).unwrap();
        assert!(look.now > 0);
        assert_eq!(look.number("0.pid"), Some(77));
        assert_eq!(look.number("0.exit"), Some(3));
        assert!(look.written("0.pid").is_some());
        assert_eq!(look.number("0.queued"), None);
        assert!(!look.files.contains_key("gerdr-check-0"));
    }

    #[test]
    fn a_look_at_no_folder_is_no_files_and_no_answer_is_not_that() {
        let out = sh_c(&look_script("/no/such/folder", "*.pid"));
        let look = Look::parse(&String::from_utf8_lossy(&out.stdout)).unwrap();
        assert!(look.files.is_empty());
        assert_eq!(Look::parse(""), None);
        assert_eq!(Look::parse("ssh: lost\n"), None);
    }

    #[test]
    fn signals_there_reach_the_checks_group_and_nothing_else() {
        // A process group of its own, as a shell's job is.
        use std::os::unix::process::CommandExt;
        let mut child = Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        let out = sh_c(&kill_group_of_script(pid, "KILL"));
        assert!(out.status.success(), "{out:?}");
        assert!(child.wait().unwrap().code().is_none());
        // Gone by now: asked again, it is no failure and kills nothing.
        assert!(sh_c(&kill_group_script(pid, "TERM")).status.success());
        assert!(sh_c(&kill_group_of_script(pid, "KILL")).status.success());
    }

    #[test]
    fn what_a_branch_doesnt_ignore_goes_again() {
        let t = crate::feature::tests::Temp::new();
        let run = |args: &[&str]| crate::feature::tests::run(t.0.as_path(), args);
        run(&["init", "-q"]);
        std::fs::write(t.0.as_path().join(".gitignore"), ".env\n").unwrap();
        std::fs::write(t.0.as_path().join(".env"), "a").unwrap();
        std::fs::write(t.0.as_path().join("local.py"), "b").unwrap();
        let wt = t.0.as_path().to_string_lossy().into_owned();
        let items = [".env".to_owned(), "local.py".to_owned()];
        let out = sh_c(&unignored_script(&wt, &items));
        assert_eq!(String::from_utf8_lossy(&out.stdout), "local.py\n");
        assert!(t.0.as_path().join(".env").exists());
        assert!(!t.0.as_path().join("local.py").exists());
    }

    #[test]
    fn output_there_reads_as_it_does_here() {
        let back = paths().back();
        assert_eq!(
            back.text("at /home/ana/Personal/x/src/a.rs:3 and /var/ana/gerdr/f"),
            "at /home/lg/Personal/x/src/a.rs:3 and /home/lg/.local/share/gerdr/f"
        );
    }

    use super::*;
    use crate::feature::{ContextRepo, Repo};

    fn sides() -> (Side, Side) {
        (
            Side {
                home: "/home/lg".into(),
                data: "/home/lg/.local/share/gerdr".into(),
            },
            Side {
                home: "/home/ana".into(),
                data: "/var/ana/gerdr".into(),
            },
        )
    }

    fn paths() -> Paths {
        let (here, there) = sides();
        Paths::new(&here, &there, &BTreeMap::new(), None)
    }

    #[test]
    fn a_path_reads_as_the_most_exact_folder_says() {
        let p = paths();
        assert_eq!(
            p.map(Path::new("/home/lg/Personal/app")),
            Path::new("/home/ana/Personal/app")
        );
        assert_eq!(
            p.map(Path::new("/home/lg/.local/share/gerdr/features/p/s")),
            Path::new("/var/ana/gerdr/features/p/s")
        );
        assert_eq!(p.map(Path::new("/home/lg")), Path::new("/home/ana"));
        // Another user's, and one that only starts the same.
        assert_eq!(p.map(Path::new("/home/lga/x")), Path::new("/home/lga/x"));
        let (here, there) = sides();
        let folders = [("/home/lg/Personal/app".to_owned(), "/srv/app".to_owned())].into();
        let p = Paths::new(&here, &there, &folders, None);
        assert_eq!(
            p.map(Path::new("/home/lg/Personal/app/.features/a")),
            Path::new("/srv/app/.features/a")
        );
    }

    #[test]
    fn what_a_feature_holds_outside_the_home_gets_a_place_there() {
        let (here, there) = sides();
        let mut f = crate::feature::tests::feature("a", 0);
        f.root = "/opt/app".into();
        f.dir = "/opt/app/.features/a".into();
        f.repos.clear();
        f.context.push(ContextRepo {
            name: "docs".into(),
            main: "/srv/docs".into(),
            project: None,
        });
        let p = Paths::new(&here, &there, &BTreeMap::new(), Some(&f));
        assert_eq!(
            p.map(Path::new("/opt/app/.features/a")),
            Path::new("/home/ana/gerdr-work/opt/app/.features/a")
        );
        assert_eq!(
            p.map(Path::new("/srv/docs")),
            Path::new("/home/ana/gerdr-work/srv/docs")
        );
    }

    #[test]
    fn text_is_translated_at_whole_paths_only() {
        let p = paths();
        assert_eq!(
            p.text("Read /home/lg/.local/share/gerdr/features/a/plan.md, then '/home/lg/x y'."),
            "Read /var/ana/gerdr/features/a/plan.md, then '/home/ana/x y'."
        );
        // Ending a sentence, it is the path; not when the name goes on.
        assert_eq!(p.text("see /home/lg."), "see /home/ana.");
        assert_eq!(
            p.text("/home/lgal/x /home/lg.bak"),
            "/home/lgal/x /home/lg.bak"
        );
        // Not inside another path or word.
        assert_eq!(
            p.text("/mnt/home/lg/x a/home/lg"),
            "/mnt/home/lg/x a/home/lg"
        );
        assert_eq!(p.text("--add-dir=/home/lg/x"), "--add-dir=/home/ana/x");
    }

    #[test]
    fn a_home_inside_the_other_home_is_translated_once() {
        let here = Side {
            home: "/home/a".into(),
            data: "/d".into(),
        };
        let there = Side {
            home: "/home/a/a".into(),
            data: "/e".into(),
        };
        let p = Paths::new(&here, &there, &BTreeMap::new(), None);
        assert_eq!(p.text("/home/a/x and /home/a"), "/home/a/a/x and /home/a/a");
    }

    #[test]
    fn a_standing_is_read_with_or_without_changes() {
        assert_eq!(
            parse_status("api\tabc\tdirty\nweb\tdef\t\n"),
            [
                Standing {
                    key: "api".into(),
                    head: "abc".into(),
                    dirty: true
                },
                Standing {
                    key: "web".into(),
                    head: "def".into(),
                    dirty: false
                },
            ]
        );
    }

    #[test]
    fn a_host_is_written_for_git_and_rsync() {
        let reach = |target: &str| Reach {
            target: target.into(),
            control: "/c".into(),
        };
        assert_eq!(
            reach("ana@box").git_url("/home/ana/a%b"),
            "ssh://ana@box/home/ana/a%25b"
        );
        assert_eq!(reach("ssh://ana@box:2222").host(), "ana@box");
        assert_eq!(reach("ana@box").rsync_path("/d/x/"), "ana@box:/d/x/");
    }

    /// A folder of its own, removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "gerdr-remote-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self, rel: &str) -> String {
            self.0.join(rel).to_string_lossy().into_owned()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn sh(script: &str) -> std::process::Output {
        Command::new("sh").args(["-c", script]).output().unwrap()
    }

    fn git(dir: &str, args: &str) -> String {
        let line =
            format!("git -C {dir} -c user.name=t -c user.email=t@t -c commit.gpgsign=false {args}");
        let out = sh(&line);
        assert!(out.status.success(), "{line}: {out:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    /// A repository with a commit, on `feature/x`, and the place it has at
    /// `there`.
    fn repo(tmp: &Scratch, there: &str) -> (Place, String) {
        let main = tmp.path("here");
        assert!(sh(&format!("git init -q -b main {main}")).status.success());
        std::fs::write(tmp.0.join("here/a"), "a").unwrap();
        git(&main, "add a");
        git(&main, "commit -qm one");
        git(&main, "branch feature/x");
        let root = git(&main, "rev-parse HEAD");
        let place = Place {
            key: "app".into(),
            local_main: main.clone().into(),
            local_worktree: Some(main.clone().into()),
            main: tmp.path(there),
            worktree: Some(format!("{}/.features/x", tmp.path(there))),
            branch: Some("feature/x".into()),
            base: "main".into(),
            commit: root.clone(),
            commit_ref: "refs/gerdr/base/x".into(),
        };
        (place, root)
    }

    #[test]
    fn a_folder_holding_only_feature_folders_is_free_for_a_repository() {
        let tmp = Scratch::new();
        let (mut place, root) = repo(&tmp, "there");
        std::fs::create_dir_all(tmp.0.join("there/.features/x")).unwrap();
        place.main = tmp.path("there");
        let ensure = ensure_script(&[(place.clone(), root.clone())]);
        assert!(sh(&ensure).status.success());
        assert!(tmp.0.join("there/.git").exists());
        // Anything else there is the user's.
        std::fs::create_dir_all(tmp.0.join("other")).unwrap();
        std::fs::write(tmp.0.join("other/notes"), "n").unwrap();
        place.main = tmp.path("other");
        let out = sh(&ensure_script(&[(place, root)]));
        assert!(String::from_utf8_lossy(&out.stdout).contains("OCCUPIED"), "{out:?}");
    }

    #[test]
    fn a_look_here_follows_commits_and_leaves_changes_alone() {
        let tmp = Scratch::new();
        let (place, _) = repo(&tmp, "there");
        let main = place.local_main.to_string_lossy().into_owned();
        let worktree = tmp.0.join("here/.features/x");
        let name = view_head_ref("x", "app");
        git(&main, &format!("update-ref {name} HEAD"));
        let script = view_worktree_script(&place.local_main, &worktree, &name);
        let cancel = AtomicBool::new(false);
        local_script(&script, &cancel).unwrap();
        let wt = worktree.to_string_lossy().into_owned();
        assert_eq!(git(&wt, "rev-parse HEAD"), git(&main, &format!("rev-parse {name}")));
        assert_eq!(git(&main, "rev-parse --abbrev-ref HEAD"), "main");
        assert_eq!(git(&wt, "rev-parse --abbrev-ref HEAD"), "HEAD");
        assert_eq!(git(&main, "status --porcelain"), "");
        // New commits there move it.
        git(&main, "checkout -q feature/x");
        std::fs::write(tmp.0.join("here/a"), "b").unwrap();
        git(&main, "commit -qam two");
        git(&main, &format!("update-ref {name} HEAD"));
        local_script(&script, &cancel).unwrap();
        assert_eq!(std::fs::read_to_string(worktree.join("a")).unwrap(), "b");
        // What the user changed here isn't run over, and it says so.
        std::fs::write(worktree.join("a"), "mine").unwrap();
        std::fs::write(tmp.0.join("here/a"), "c").unwrap();
        git(&main, "commit -qam three");
        git(&main, &format!("update-ref {name} HEAD"));
        let refused = local_script(&script, &cancel).unwrap_err();
        assert!(refused.text().contains("in the way"), "{}", refused.text());
        assert_eq!(std::fs::read_to_string(worktree.join("a")).unwrap(), "mine");
    }

    #[test]
    fn heads_say_the_worktree_and_where_it_started() {
        let tmp = Scratch::new();
        let (mut place, root) = repo(&tmp, "there");
        place.main = place.local_main.to_string_lossy().into_owned();
        place.worktree = Some(place.main.clone());
        let out = sh(&heads_script(std::slice::from_ref(&place)));
        let said = parse_heads(&String::from_utf8_lossy(&out.stdout));
        assert_eq!(said["app"], (root.clone(), root.clone()));
        // No worktree there yet: only where it started.
        place.worktree = Some(tmp.path("gone"));
        let out = sh(&heads_script(std::slice::from_ref(&place)));
        let said = parse_heads(&String::from_utf8_lossy(&out.stdout));
        assert_eq!(said["app"], (String::new(), root));
    }

    #[test]
    fn a_feature_written_elsewhere_reads_from_where_it_left_there() {
        let tmp = Scratch::new();
        let (place, root) = repo(&tmp, "there");
        let main = place.local_main.to_string_lossy().into_owned();
        let repo = crate::feature::Repo {
            name: "app".into(),
            main: place.local_main.clone(),
            base: "main".into(),
            base_commit: root.clone(),
            worktree: place.local_main.clone(),
            touched: true,
            branch: Some("feature/x".into()),
            project: None,
        };
        assert_eq!(super::super::layout::base_ref(&repo), "main");
        git(&main, &format!("update-ref {} HEAD", view_base_ref("feature/x")));
        assert_eq!(
            super::super::layout::base_ref(&repo),
            "refs/gerdr/view-base/feature/x"
        );
    }

    /// `git push` to the place, as `Reach::push_code` does over SSH.
    fn pushed(place: &Place) {
        git(
            &place.local_main.to_string_lossy(),
            &format!(
                "push -q -f {} {}:{} refs/heads/feature/x:refs/gerdr/feature/x",
                place.main, place.commit, place.commit_ref
            ),
        );
    }

    #[test]
    fn a_repository_goes_there_and_gets_a_worktree_on_its_branch() {
        let tmp = Scratch::new();
        let (place, root) = repo(&tmp, "there");
        let data = tmp.path("data");
        let places = [place.clone()];
        let ensure = ensure_script(&[(place.clone(), root.clone())]);
        let out = sh(&ensure);
        assert!(out.status.success(), "{out:?}");
        pushed(&place);
        let docs = [place.worktree.clone().unwrap()];
        let script = worktree_script(&places, &docs, &data);
        let out = sh(&script);
        assert!(out.status.success(), "{out:?}");
        let worktree = place.worktree.clone().unwrap();
        assert_eq!(git(&worktree, "rev-parse HEAD"), root);
        assert_eq!(git(&worktree, "symbolic-ref --short HEAD"), "feature/x");
        assert_eq!(git(&place.main, "rev-parse refs/heads/main"), root);
        // Its link to the files is there, and git doesn't see it, or the
        // worktrees in the repository's folder.
        assert_eq!(
            std::fs::read_link(format!("{worktree}/feature-docs")).unwrap(),
            Path::new(&data)
        );
        assert_eq!(git(&worktree, "status --porcelain"), "");
        assert_eq!(git(&place.main, "status --porcelain"), "");
        // The branch is guarded there, as here.
        assert!(hook::guarded(Path::new(&place.main), "feature/x"));
        // Again it changes nothing, and a new commit here comes along.
        let main = place.local_main.to_string_lossy().into_owned();
        git(&main, "checkout -q feature/x");
        std::fs::write(tmp.0.join("here/b"), "b").unwrap();
        git(&main, "add b");
        git(&main, "commit -qm two");
        let two = git(&main, "rev-parse HEAD");
        assert!(sh(&ensure).status.success());
        pushed(&place);
        let out = sh(&script);
        assert!(out.status.success(), "{out:?}");
        assert_eq!(git(&worktree, "rev-parse HEAD"), two);
        // What the worktree stands at is read, changes too.
        let status = status_script(&places);
        let said = parse_status(&String::from_utf8_lossy(&sh(&status).stdout));
        assert_eq!(
            (said[0].head.as_str(), said[0].dirty),
            (two.as_str(), false)
        );
        std::fs::write(format!("{worktree}/c"), "c").unwrap();
        let said = parse_status(&String::from_utf8_lossy(&sh(&status).stdout));
        assert!(said[0].dirty);
    }

    #[test]
    fn a_removed_feature_leaves_only_the_repository_there() {
        let tmp = Scratch::new();
        let (place, root) = repo(&tmp, "there");
        assert!(
            sh(&ensure_script(&[(place.clone(), root)]))
                .status
                .success()
        );
        pushed(&place);
        let data = tmp.path("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(format!("{data}/plan.md"), "x").unwrap();
        let docs = [place.worktree.clone().unwrap()];
        let places = [place.clone()];
        assert!(sh(&worktree_script(&places, &docs, &data)).status.success());
        let out = sh(&forget_script(&places, &docs, &data));
        assert!(out.status.success(), "{out:?}");
        assert!(!Path::new(place.worktree.as_deref().unwrap()).exists());
        assert!(!Path::new(&data).exists());
        assert!(Path::new(&place.main).join(".git").exists());
        assert_eq!(git(&place.main, "for-each-ref refs/gerdr"), "");
        let branches = git(&place.main, "branch '--format=%(refname:short)'");
        assert_eq!(branches, "main");
    }

    #[test]
    fn a_folder_holding_something_else_is_left_alone_and_said() {
        let tmp = Scratch::new();
        let (place, root) = repo(&tmp, "there");
        std::fs::create_dir_all(tmp.0.join("there")).unwrap();
        std::fs::write(tmp.0.join("there/mine"), "mine").unwrap();
        let script = ensure_script(&[(place.clone(), root.clone())]);
        let out = sh(&script);
        assert_eq!(out.status.code(), Some(3));
        assert!(String::from_utf8_lossy(&out.stdout).starts_with("OCCUPIED "));
        assert!(!tmp.0.join("there/.git").exists());
        // Another repository is something else too; an empty folder isn't.
        std::fs::remove_file(tmp.0.join("there/mine")).unwrap();
        assert!(sh(&script).status.success());
        let there = tmp.path("there");
        std::fs::write(tmp.0.join("there/z"), "z").unwrap();
        git(&there, "add z");
        git(&there, "commit -qm other");
        assert_eq!(sh(&script).status.code(), Some(3));
    }

    #[test]
    fn a_worktree_gone_another_way_is_not_moved() {
        let tmp = Scratch::new();
        let (place, root) = repo(&tmp, "there");
        assert!(
            sh(&ensure_script(&[(place.clone(), root)]))
                .status
                .success()
        );
        pushed(&place);
        let docs: [String; 0] = [];
        let script = worktree_script(std::slice::from_ref(&place), &docs, &tmp.path("data"));
        assert!(sh(&script).status.success());
        let worktree = place.worktree.clone().unwrap();
        // A commit there, and another here.
        std::fs::write(format!("{worktree}/there"), "x").unwrap();
        git(&worktree, "add there");
        git(&worktree, "commit -qm there");
        let there = git(&worktree, "rev-parse HEAD");
        let main = place.local_main.to_string_lossy().into_owned();
        git(&main, "checkout -q feature/x");
        std::fs::write(tmp.0.join("here/here"), "x").unwrap();
        git(&main, "add here");
        git(&main, "commit -qm here");
        pushed(&place);
        assert_eq!(sh(&script).status.code(), Some(6));
        assert_eq!(git(&worktree, "rev-parse HEAD"), there);
    }

    #[test]
    fn places_name_a_repositorys_worktree_and_a_context_repositorys_main() {
        let p = paths();
        let mut f = crate::feature::tests::feature("f", 0);
        f.repos = vec![Repo {
            name: "app".into(),
            main: "/home/lg/app".into(),
            base: "main".into(),
            base_commit: "c".into(),
            worktree: "/home/lg/app/.features/f".into(),
            touched: true,
            branch: Some("feature/f".into()),
            project: None,
        }];
        f.context.push(ContextRepo {
            name: "docs".into(),
            main: "/home/lg/docs".into(),
            project: None,
        });
        let places = places(&f, &p);
        assert_eq!(places[0].main, "/home/ana/app");
        assert_eq!(
            places[0].worktree.as_deref(),
            Some("/home/ana/app/.features/f")
        );
        assert_eq!(places[0].branch.as_deref(), Some("feature/f"));
        assert_eq!(places[1].main, "/home/ana/docs");
        assert_eq!(places[1].worktree, None);
    }
}

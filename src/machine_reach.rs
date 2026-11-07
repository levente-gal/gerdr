//! Which gerdrs reached this machine. A gerdr connecting to a machine leaves
//! `<data>/gerdr/reached-by/<its id>` there, saying what it is called and where
//! it came from, and touches it now and then while it stays connected.
//!
//! The machine's own gerdr reads them: it offers once to add a gerdr that
//! reached it, as each one shows only the features of machines it added. And a
//! file's time tells a gerdr still around from one long gone, for the sandboxes
//! another gerdr left on a machine.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::{Gerdr, desktop::shell_quote, menu::MenuAction};

/// The folder of those files, in gerdr's data folder.
pub(crate) const DIR: &str = "reached-by";

/// How often a connected gerdr touches its file there.
pub(crate) const TOUCH_EVERY: Duration = Duration::from_secs(60 * 60);

/// A file touched this recently means its gerdr is still around: a few
/// touches missed, as a machine sleeps, don't make it gone.
pub(crate) const AROUND: u64 = 2 * 24 * 60 * 60;

/// How often this gerdr looks for gerdrs that reached it.
const LOOK_EVERY: Duration = Duration::from_secs(10);

/// What a gerdr that reached a machine left there.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Reached {
    /// Its machine id.
    pub id: String,
    /// What its machine calls itself.
    pub name: String,
    /// The user it runs as.
    pub user: String,
    /// The address it came from, as this machine saw it.
    pub from: String,
    /// When it last connected or touched the file, in Unix seconds.
    pub at: u64,
}

impl Reached {
    /// What to connect to it as: its user at the address it came from, or at
    /// its name when the address isn't known.
    pub fn target(&self) -> String {
        let host = if self.from.is_empty() { &self.name } else { &self.from };
        match self.user.is_empty() {
            true => host.clone(),
            false => format!("{}@{host}", self.user),
        }
    }

    /// Whether its gerdr counts as still around at `now`.
    pub fn around(&self, now: u64) -> bool {
        now.saturating_sub(self.at) < AROUND
    }
}

/// Whether `id` is fit for a file name and a shell word as it is: ids are made
/// of hex digits.
fn plain_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Lines for a script there, after `$d` is set to its data folder: leave this
/// gerdr's file, `id` called `name`, running as `user`.
pub(crate) fn leave_lines(id: &str, name: &str, user: &str) -> String {
    if !plain_id(id) {
        return String::new();
    }
    format!(
        r#"mkdir -p "$d/gerdr/{DIR}" && printf 'name=%s\nuser=%s\nfrom=%s\n' {} {} "${{SSH_CONNECTION%% *}}" > "$d/gerdr/{DIR}/{id}"
"#,
        shell_quote(name),
        shell_quote(user),
    )
}

/// A script touching file `id` in gerdr's folder `gerdr` there.
pub(crate) fn touch_script(gerdr: &str, id: &str) -> Option<String> {
    plain_id(id).then(|| format!("touch {}", shell_quote(&format!("{gerdr}/{DIR}/{id}"))))
}

/// A script saying, for each file in gerdr's folder `gerdr` there, its id, its
/// time and the name in it: what `parse_list` reads.
pub(crate) fn list_script(gerdr: &str) -> String {
    format!(
        r#"cd {} 2>/dev/null || exit 0
for f in *; do case "$f" in *.told) continue;; esac; [ -f "$f" ] || continue
printf '%s\t%s\t%s\n' "$f" "$(stat -c %Y "$f")" "$(sed -n 's/^name=//p' "$f" | head -n 1)"; done
"#,
        shell_quote(&format!("{gerdr}/{DIR}"))
    )
}

/// What `list_script` printed.
pub(crate) fn parse_list(text: &str) -> Vec<Reached> {
    (text.lines())
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let id = parts.next()?.to_owned();
            let at = parts.next()?.trim().parse().ok()?;
            let name = parts.next().unwrap_or_default().to_owned();
            plain_id(&id).then_some(Reached {
                id,
                name,
                at,
                ..Reached::default()
            })
        })
        .collect()
}

/// What a file holds, the gerdr `id` left at `at`.
fn parse_file(id: &str, text: &str, at: u64) -> Reached {
    let mut reached = Reached {
        id: id.to_owned(),
        at,
        ..Reached::default()
    };
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().to_owned();
        match key {
            "name" => reached.name = value,
            "user" => reached.user = value,
            "from" => reached.from = value,
            _ => {}
        }
    }
    reached
}

/// The folder of those files here.
pub(crate) fn dir_here() -> PathBuf {
    crate::projects::data_dir().join(DIR)
}

/// The gerdrs that reached this machine, with whether this one already offered
/// to add each.
pub(crate) fn read_dir(dir: &Path) -> Vec<(Reached, bool)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<(Reached, bool)> = (entries.flatten())
        .filter_map(|entry| {
            let id = entry.file_name().to_str()?.to_owned();
            if !plain_id(&id) {
                return None;
            }
            let at = (entry.metadata().ok()?.modified().ok()?)
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let text = std::fs::read_to_string(entry.path()).ok()?;
            let told = dir.join(format!("{id}.told")).exists();
            Some((parse_file(&id, &text, at), told))
        })
        .collect();
    found.sort_by(|a, b| a.0.id.cmp(&b.0.id));
    found
}

/// Note that adding `id` was offered, so it isn't again.
fn mark_told(dir: &Path, id: &str) {
    if let Err(err) = std::fs::write(dir.join(format!("{id}.told")), "") {
        tracing::warn!("noting {id} was offered: {err}");
    }
}

/// The ids of the machines this gerdr connected to: those connected now, and
/// those it ever kept anything of.
fn known_ids(gerdr: &Gerdr) -> Vec<String> {
    let mut ids: Vec<String> = gerdr.machines.names.keys().cloned().collect();
    if let Ok(entries) = std::fs::read_dir(crate::projects::data_dir().join("machines")) {
        ids.extend((entries.flatten()).filter_map(|e| e.file_name().to_str().map(str::to_owned)));
    }
    ids
}

/// Of `found`, what to offer to add: gerdrs around now that this one doesn't
/// know and hasn't offered yet, nor this gerdr itself.
fn to_offer(
    found: Vec<(Reached, bool)>,
    own: Option<&str>,
    known: &[String],
    now: u64,
) -> Vec<Reached> {
    (found.into_iter())
        .filter(|(reached, told)| {
            !told
                && reached.around(now)
                && own != Some(reached.id.as_str())
                && !known.contains(&reached.id)
        })
        .map(|(reached, _)| reached)
        .collect()
}

/// Seconds since the epoch, now.
pub(crate) fn now_secs() -> u64 {
    (SystemTime::now().duration_since(UNIX_EPOCH)).map_or(0, |d| d.as_secs())
}

/// What the toast offering to add `reached` says.
fn offer_text(reached: &Reached) -> String {
    let name = if reached.name.is_empty() { "Another machine" } else { &reached.name };
    format!("{name} connected to this machine: add it here too, to see its features")
}

impl Gerdr {
    /// Now and then: offer to add each gerdr that reached this machine and that
    /// this one doesn't know, once.
    pub(crate) fn reach_tick(&mut self) {
        if (self.machines.reach_looked).is_some_and(|at| at.elapsed() < LOOK_EVERY) {
            return;
        }
        self.machines.reach_looked = Some(Instant::now());
        let dir = dir_here();
        let found = read_dir(&dir);
        if found.is_empty() {
            return;
        }
        let own = crate::machine_views::own_id();
        let known = known_ids(self);
        for reached in to_offer(found, own.as_deref(), &known, now_secs()) {
            mark_told(&dir, &reached.id);
            let action = MenuAction::AddMachineFrom {
                target: reached.target(),
                name: reached.name.clone(),
            };
            self.toast_with(
                offer_text(&reached),
                crate::toast::Level::Info,
                Some(("Add\u{2026}".to_owned(), action)),
            );
        }
    }

    /// The Add button of that toast: the connect dialog, filled in.
    pub(crate) fn add_machine_from(&mut self, target: String, name: String) {
        if self.machines.dialog.is_none() {
            self.machines.draft = Some((target, name));
        }
        self.open_machine_dialog();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reached(id: &str, at: u64) -> Reached {
        Reached {
            id: id.into(),
            name: format!("{id}-box"),
            at,
            ..Reached::default()
        }
    }

    #[test]
    fn a_file_says_who_and_from_where() {
        let r = parse_file("ab12", "name=laptop\nuser=ana\nfrom=10.0.0.7\n", 5);
        assert_eq!(r.name, "laptop");
        assert_eq!(r.target(), "ana@10.0.0.7");
        let no_address = parse_file("ab12", "name=laptop\nuser=ana\nfrom=\n", 5);
        assert_eq!(no_address.target(), "ana@laptop");
    }

    #[test]
    fn only_a_new_gerdr_still_around_is_offered() {
        let now = 10 * AROUND;
        let found = vec![
            (reached("new", now - 60), false),
            (reached("told", now - 60), true),
            (reached("gone", now - AROUND - 1), false),
            (reached("me", now), false),
            (reached("known", now), false),
        ];
        let offered = to_offer(found, Some("me"), &["known".to_owned()], now);
        assert_eq!(offered.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["new"]);
    }

    #[test]
    fn the_list_there_is_read_back() {
        let list = parse_list("ab12\t1700000000\tlaptop\nbad id\t1\tx\ncd34\tnot a time\t\n");
        assert_eq!(list, [Reached { name: "laptop".into(), ..reached("ab12", 1_700_000_000) }]);
    }

    #[test]
    fn what_a_script_writes_is_quoted() {
        let lines = leave_lines("ab12", "it's", "ana");
        assert!(lines.contains(r#"'it'\''s'"#), "{lines}");
        assert!(lines.contains("reached-by/ab12\""), "{lines}");
        assert!(leave_lines("../x", "n", "u").is_empty());
        assert!(touch_script("/d", "a b").is_none());
    }

    #[test]
    fn files_here_are_read_with_what_was_offered() {
        let dir = std::env::temp_dir().join(format!("gerdr-reach-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ab12"), "name=laptop\nuser=ana\nfrom=1.2.3.4\n").unwrap();
        std::fs::write(dir.join("cd34"), "name=desk\n").unwrap();
        mark_told(&dir, "cd34");
        let found = read_dir(&dir);
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].0.target().as_str(), found[0].1), ("ana@1.2.3.4", false));
        assert_eq!((found[1].0.name.as_str(), found[1].1), ("desk", true));
        assert!(found[0].0.around(now_secs()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

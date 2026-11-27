//! The skills a stage's agent is told to use: named per stage in the
//! project's `workflow.skills`, else `DEFAULTS`. Only those installed for
//! the agent's CLI are named, and gerdr never copies a skill's text: a
//! skill renamed or gone upstream is simply left out. Each one named is
//! stamped on the run with its folder hash from the `skills` CLI's lock
//! file, so a change in how an agent works can be traced to a skill's
//! update.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use super::Stage;
use crate::{ai_work::locate::Homes, integrations::Integration};

/// The skills a stage names when its project names none for it.
pub const DEFAULTS: &[(Stage, &[&str])] = &[
    (Stage::Discover, &["grilling", "domain-modeling"]),
    (Stage::Plan, &["codebase-design"]),
    (Stage::Build, &["tdd", "diagnosing-bugs"]),
    (Stage::AiReview, &["codebase-design"]),
];

/// The skills `stage` names: `set`'s for it where it has an entry (an
/// empty one for none), else `DEFAULTS`'.
pub fn wanted(set: &BTreeMap<Stage, Vec<String>>, stage: Stage) -> Vec<String> {
    if let Some(names) = set.get(&stage) {
        return names.clone();
    }
    (DEFAULTS.iter().find(|(s, _)| *s == stage))
        .map(|(_, names)| names.iter().map(|n| (*n).to_owned()).collect())
        .unwrap_or_default()
}

/// Those of `names` with a `<name>/SKILL.md` in one of `dirs`, in their
/// order. A name that isn't a plain folder name is left out: it comes from
/// the store file.
pub fn installed(names: &[String], dirs: &[PathBuf]) -> Vec<String> {
    (names.iter())
        .filter(|name| plain(name))
        .filter(|name| {
            dirs.iter()
                .any(|dir| dir.join(name).join("SKILL.md").is_file())
        })
        .cloned()
        .collect()
}

/// Those of `names` installed for `agent` in folder `cwd`, on this machine.
pub fn installed_for(agent: &dyn Integration, cwd: &Path, names: &[String]) -> Vec<String> {
    let Some(homes) = Homes::here() else {
        return Vec::new();
    };
    let config = crate::chat::commands::config_dir();
    installed(names, &agent.skill_dirs(cwd, &homes, &config))
}

fn plain(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && (name.chars()).all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

/// The prompt's list of `names`, one `- \`name\`` line each.
pub fn listed(names: &[String]) -> String {
    (names.iter())
        .map(|name| format!("- `{name}`"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `names` stamped with their folder hashes' first 7 digits from lock file
/// text `lock`, where it has them: ` skills:tdd@667dbdd,pr`. Empty for no
/// names.
pub fn stamp(names: &[String], lock: &str) -> String {
    if names.is_empty() {
        return String::new();
    }
    let lock: serde_json::Value = serde_json::from_str(lock).unwrap_or_default();
    let each: Vec<String> = (names.iter())
        .map(|name| {
            let hash = lock["skills"][name]["skillFolderHash"].as_str();
            match hash.and_then(|h| h.get(..7)) {
                Some(short) => format!("{name}@{short}"),
                None => name.clone(),
            }
        })
        .collect();
    format!(" skills:{}", each.join(","))
}

/// The `skills` CLI's lock file here: `$XDG_STATE_HOME/skills/.skill-lock.json`,
/// else `~/.agents/.skill-lock.json`. Empty when there is none.
pub fn lock_here() -> String {
    let state = (std::env::var_os("XDG_STATE_HOME").map(PathBuf::from))
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join("skills/.skill-lock.json"));
    let home = crate::home_dir().map(|home| home.join(".agents/.skill-lock.json"));
    (state.into_iter().chain(home))
        .find_map(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(of: &[&str]) -> Vec<String> {
        of.iter().map(|n| (*n).to_owned()).collect()
    }

    #[test]
    fn a_stage_names_the_projects_skills_else_the_defaults() {
        let mut set = BTreeMap::new();
        assert_eq!(
            wanted(&set, Stage::Build),
            names(&["tdd", "diagnosing-bugs"])
        );
        assert!(wanted(&set, Stage::Verify).is_empty());
        set.insert(Stage::Build, names(&["tests-first"]));
        set.insert(Stage::Discover, Vec::new());
        assert_eq!(wanted(&set, Stage::Build), names(&["tests-first"]));
        // An empty entry names none, over the defaults.
        assert!(wanted(&set, Stage::Discover).is_empty());
    }

    #[test]
    fn only_skills_found_in_a_folder_are_named() {
        let root = std::env::temp_dir().join(format!("gerdr-skills-{}", std::process::id()));
        let (a, b) = (root.join("a"), root.join("b"));
        for (dir, skill) in [(&a, "tdd"), (&b, "grilling")] {
            std::fs::create_dir_all(dir.join(skill)).unwrap();
            std::fs::write(dir.join(skill).join("SKILL.md"), "---\nname: x\n---\n").unwrap();
        }
        std::fs::create_dir_all(a.join("empty")).unwrap();
        let found = installed(
            &names(&["grilling", "empty", "gone", "tdd", "../a/tdd", ""]),
            &[a, b],
        );
        assert_eq!(found, names(&["grilling", "tdd"]));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_stamp_names_each_skill_with_its_folder_hash() {
        let lock = r#"{"version":3,"skills":{"tdd":{"skillFolderHash":"667dbdd00c11"}}}"#;
        assert_eq!(
            stamp(&names(&["tdd", "pr"]), lock),
            " skills:tdd@667dbdd,pr"
        );
        assert_eq!(stamp(&names(&["tdd"]), "not json"), " skills:tdd");
        assert_eq!(stamp(&[], lock), "");
        assert_eq!(listed(&names(&["tdd", "pr"])), "- `tdd`\n- `pr`");
    }
}

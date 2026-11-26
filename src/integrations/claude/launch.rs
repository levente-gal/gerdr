//! Starting Claude Code: its folders, the chat id it takes, its models.

use std::path::{Path, PathBuf};

use crate::{
    ai_work::locate::Homes, desktop::shell_quote, integrations::common, stage_models::Offer,
};

/// What `--model` takes for the latest of each model: it has no command
/// listing its models.
pub fn offers() -> Vec<Offer> {
    [
        ("fable", "Fable"),
        ("opus", "Opus"),
        ("sonnet", "Sonnet"),
        ("haiku", "Haiku"),
    ]
    .map(|(id, name)| Offer {
        id: id.to_owned(),
        name: name.to_owned(),
    })
    .to_vec()
}

/// What makes its new chat's id ours, kept in `id_file` (shell-quoted).
pub fn new_chat(id_file: &str) -> String {
    format!(" --session-id \"$(tee {id_file} </proc/sys/kernel/random/uuid)\"")
}

/// What has it read the instructions of its `--add-dir` folders.
const CLAUDE_MD_ENV: &str = "CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD";

/// Command `line` allowed into `dirs`. It asks to trust each `--add-dir`
/// folder every time it starts unless its repository was trusted by
/// starting it in it (checked on 2.1.326), so its folders go in as the
/// `additionalDirectories` of `--settings`, which it doesn't ask about. It
/// doesn't load their CLAUDE.md or AGENTS.md then: the prompt says to read
/// them (`assets/prompts/repos.md`). A command with `--settings` of its own
/// keeps `--add-dir`, told to read those folders' instructions.
pub fn add_dirs(mut line: String, dirs: &[PathBuf]) -> String {
    if !line.contains("--settings") {
        let settings = serde_json::json!({ "permissions": { "additionalDirectories": dirs } });
        line.push_str(" --settings ");
        line.push_str(&shell_quote(&settings.to_string()));
        return line;
    }
    if !line.contains(CLAUDE_MD_ENV) {
        line = format!("{CLAUDE_MD_ENV}=1 {line}");
    }
    common::launch::add_dirs(line, dirs)
}

/// How much of a transcript's end is read first for its model: most of a
/// long chat needn't be read to find its last answer.
const TRANSCRIPT_TAIL: u64 = 256 * 1024;

/// The model of the last answer in a transcript's `text`. Lines it wrote
/// itself name `<synthetic>`, and subagents' their own.
fn last_model(text: &str) -> Option<String> {
    text.lines()
        .rev()
        .filter(|l| l.contains("\"model\""))
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["type"] == "assistant" && v["isSidechain"] != true)
        .find_map(|v| {
            let model = v["message"]["model"].as_str()?;
            (!model.is_empty() && model != "<synthetic>").then(|| model.to_owned())
        })
}

/// The model of chat `id`, from its transcript in any config folder of
/// `homes`. None without an answer yet.
pub fn run_model(homes: &Homes, id: &str) -> Option<String> {
    (homes.claude.iter()).find_map(|config| model_in(&config.join("projects"), id))
}

/// The model of chat `id`, from its transcript under `projects` (one
/// folder per working directory).
fn model_in(projects: &Path, id: &str) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let name = format!("{id}.jsonl");
    let path = (std::fs::read_dir(projects).ok()?.flatten())
        .map(|dir| dir.path().join(&name))
        .find(|path| path.is_file())?;
    let mut file = std::fs::File::open(&path).ok()?;
    let from = file.metadata().ok()?.len().saturating_sub(TRANSCRIPT_TAIL);
    file.seek(SeekFrom::Start(from)).ok()?;
    // A line cut by the seek isn't JSON: it is passed over.
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).ok()?;
    last_model(&String::from_utf8_lossy(&tail)).or_else(|| {
        let whole = (from > 0).then(|| std::fs::read(&path).ok())??;
        last_model(&String::from_utf8_lossy(&whole))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_work::write::tests::scratch;

    /// A transcript line of an answer by `model`.
    fn answer(model: &str, sidechain: bool) -> String {
        format!(
            "{{\"type\":\"assistant\",\"isSidechain\":{sidechain},\"message\":{{\"model\":\"{model}\"}}}}\n"
        )
    }

    /// A chat `id` of `text`, in a folder of a working directory.
    fn transcript(name: &str, id: &str, text: &str) -> PathBuf {
        let dir = scratch(&format!("claude-model-{name}"));
        let cwd = dir.join("-home-me-x");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(cwd.join(format!("{id}.jsonl")), text).unwrap();
        dir
    }

    #[test]
    fn a_claude_chat_names_the_model_of_its_last_real_answer() {
        let text = [
            answer("claude-sonnet-5-5", false),
            answer("claude-opus-5-5", false),
            answer("claude-haiku-5-5", true),
            answer("<synthetic>", false),
            "{\"type\":\"user\",\"model\":\"nope\"}\n".to_owned(),
        ]
        .concat();
        let dir = transcript("claude", "c1", &text);
        assert_eq!(model_in(&dir, "c1").as_deref(), Some("claude-opus-5-5"));
        assert_eq!(model_in(&dir, "other"), None);
        assert_eq!(model_in(&dir.join("none"), "c1"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_claude_answer_older_than_the_tail_is_still_found() {
        let filler = "{\"type\":\"user\"}\n".repeat(TRANSCRIPT_TAIL as usize / 8);
        let dir = transcript("tail", "c1", &(answer("claude-opus-5-5", false) + &filler));
        assert_eq!(model_in(&dir, "c1").as_deref(), Some("claude-opus-5-5"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn its_model_is_found_in_any_config_folder() {
        let dir = transcript("homes", "c1", &answer("claude-opus-5-5", false));
        let projects = dir.join("cfg").join("projects");
        std::fs::create_dir_all(&projects).unwrap();
        std::fs::rename(dir.join("-home-me-x"), projects.join("-home-me-x")).unwrap();
        let mut homes = Homes::under(&dir);
        homes.claude.push(dir.join("cfg"));
        assert_eq!(run_model(&homes, "c1").as_deref(), Some("claude-opus-5-5"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

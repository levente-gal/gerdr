//! What went wrong that the user should know about and can fix: a config
//! file gerdr couldn't read, a tool it needs that isn't installed, a
//! source the inbox couldn't reach. Each is kept, by where it came from,
//! until that source works again, for the chrome to list (the tab
//! strip's problems button).

use std::{collections::BTreeMap, sync::Mutex};

/// One thing that went wrong: where (`"config"`, `"keys"`, `"zenity"`,
/// `"inbox:github"`, ...) and what, as a sentence for the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub source: &'static str,
    pub text: String,
}

static PROBLEMS: Mutex<Vec<Problem>> = Mutex::new(Vec::new());

/// Keep `text` as a problem from `source` (once), and log it.
pub fn report(source: &'static str, text: impl Into<String>) {
    let text = text.into();
    tracing::warn!("{source}: {text}");
    let mut problems = PROBLEMS.lock().unwrap_or_else(|e| e.into_inner());
    if problems
        .iter()
        .any(|p| p.source == source && p.text == text)
    {
        return;
    }
    problems.push(Problem { source, text });
}

/// Let go of every problem from `source`: it works again.
pub fn clear(source: &'static str) {
    let mut problems = PROBLEMS.lock().unwrap_or_else(|e| e.into_inner());
    problems.retain(|p| p.source != source);
}

/// Forget problem `text` from `source` alone: it was fixed, while the
/// source's other problems stay.
pub fn forget(source: &'static str, text: &str) {
    let mut problems = PROBLEMS.lock().unwrap_or_else(|e| e.into_inner());
    problems.retain(|p| p.source != source || p.text != text);
}

/// Forget the problems from `source` whose text `gone` says no longer hold:
/// what they were about went away.
pub fn forget_matching(source: &'static str, gone: impl Fn(&str) -> bool) {
    let mut problems = PROBLEMS.lock().unwrap_or_else(|e| e.into_inner());
    problems.retain(|p| p.source != source || !gone(&p.text));
}

/// Make `texts` the problems from `source`, in place of the ones it had:
/// none when it works again.
pub fn set(source: &'static str, texts: Vec<String>) {
    let mut problems = PROBLEMS.lock().unwrap_or_else(|e| e.into_inner());
    let had: Vec<&String> = problems
        .iter()
        .filter(|p| p.source == source)
        .map(|p| &p.text)
        .collect();
    if had.len() == texts.len() && had.iter().zip(&texts).all(|(a, b)| *a == b) {
        return;
    }
    for text in &texts {
        if !had.contains(&text) {
            tracing::warn!("{source}: {text}");
        }
    }
    problems.retain(|p| p.source != source);
    problems.extend(texts.into_iter().map(|text| Problem { source, text }));
}

/// The problems kept, oldest first.
pub fn list() -> Vec<Problem> {
    PROBLEMS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The problems of the sources said by place (`report_at`), by source and
/// place.
static AT: Mutex<BTreeMap<(&'static str, String), String>> = Mutex::new(BTreeMap::new());

/// Keep `text` as the problem from `source` at `at` (a feature, a file), in
/// place of the one it had there: a source says each place's own, and one
/// place that works again (`clear_at`) leaves the others'. Logged once,
/// when it is new.
pub fn report_at(source: &'static str, at: &str, text: impl Into<String>) {
    let mut all = AT.lock().unwrap_or_else(|e| e.into_inner());
    all.insert((source, at.to_owned()), text.into());
    set_from(source, &all);
}

/// Let go of the problem from `source` at `at`: it works there again.
pub fn clear_at(source: &'static str, at: &str) {
    let mut all = AT.lock().unwrap_or_else(|e| e.into_inner());
    if all.remove(&(source, at.to_owned())).is_some() {
        set_from(source, &all);
    }
}

/// Let go of every problem from `source` at a place starting with `prefix`
/// (a folder read again: what is still wrong there is said again).
pub fn clear_under(source: &'static str, prefix: &str) {
    let mut all = AT.lock().unwrap_or_else(|e| e.into_inner());
    let before = all.len();
    all.retain(|(s, at), _| *s != source || !at.starts_with(prefix));
    if all.len() != before {
        set_from(source, &all);
    }
}

fn set_from(source: &'static str, all: &BTreeMap<(&'static str, String), String>) {
    let texts = (all.iter())
        .filter(|((s, _), _)| *s == source)
        .map(|(_, text)| text.clone())
        .collect();
    set(source, texts);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_place_that_works_again_leaves_the_others_problems() {
        let source = "test:report-at";
        let of = || -> Vec<String> {
            (list().into_iter())
                .filter(|p| p.source == source)
                .map(|p| p.text)
                .collect()
        };
        report_at(source, "p/a", "a failed");
        report_at(source, "p/b", "b failed");
        report_at(source, "p/a", "a failed again");
        assert_eq!(of(), ["a failed again", "b failed"]);
        clear_at(source, "p/b");
        assert_eq!(of(), ["a failed again"]);
        clear_at(source, "p/c");
        clear_at(source, "p/a");
        assert!(of().is_empty());
    }
}

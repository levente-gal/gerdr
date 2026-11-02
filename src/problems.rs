//! What went wrong that the user should know about and can fix: a config
//! file gerdr couldn't read, a tool it needs that isn't installed, a
//! source the inbox couldn't reach. Each is kept, by where it came from,
//! until that source works again, for the chrome to list (the tab
//! strip's problems button).

use std::sync::Mutex;

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

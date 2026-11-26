//! Writing an agent's hooks or plugin into its files: a change made once,
//! kept for Undo, with the rest of the user's text as it was.

use std::path::{Path, PathBuf};

/// A user's file gerdr changed, with what it held before (`None`: it
/// wasn't there), for Undo (checklist D1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Changed {
    pub path: PathBuf,
    pub before: Option<String>,
}

/// Write `merge(what path holds)` to `path`, unless it holds that already:
/// what changed, if anything.
pub fn install(
    path: &Path,
    merge: impl FnOnce(Option<&str>) -> Result<String, String>,
) -> Result<Option<Changed>, String> {
    let before = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(format!("{}: {err}", path.display())),
    };
    let merged = merge(before.as_deref()).map_err(|err| format!("{}: {err}", path.display()))?;
    if before.as_deref() == Some(merged.as_str()) {
        return Ok(None);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    }
    crate::projects::write_atomic(path, merged.as_bytes())
        .map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(Some(Changed {
        path: path.to_owned(),
        before,
    }))
}

/// `text`, a JSON object, with its top-level `key` set to `value`. Only
/// that value's text changes: serde_json here sorts keys, and the user's
/// files keep their order and spacing.
pub fn with_key(text: &str, key: &str, value: &serde_json::Value) -> Result<String, String> {
    let root: serde_json::Value = serde_json::from_str(text).map_err(|err| err.to_string())?;
    if !root.is_object() {
        return Err("not a JSON object".into());
    }
    let pretty = |indent: &str| {
        let text = serde_json::to_string_pretty(value).map_err(|err| err.to_string())?;
        Ok::<_, String>(text.replace('\n', &format!("\n{indent}")))
    };
    let b = text.as_bytes();
    // The object's members: where each value starts and ends, by key.
    let mut i = skip_ws(b, 0) + 1;
    let (mut found, mut last_end, mut indent) = (None, None, None::<String>);
    loop {
        i = skip_ws(b, i);
        if b.get(i) != Some(&b'"') {
            break;
        }
        let key_end = value_end(b, i);
        let name: String = serde_json::from_slice(&b[i..key_end]).map_err(|err| err.to_string())?;
        let line = text[..i].rfind('\n').map_or(0, |n| n + 1);
        let own = &text[line..i];
        let own = own.chars().all(char::is_whitespace).then(|| own.to_owned());
        let start = skip_ws(b, skip_ws(b, key_end) + 1);
        let end = value_end(b, start);
        if name == key {
            found = Some((start, end, own.clone().unwrap_or_default()));
        }
        indent = indent.or(own);
        last_end = Some(end);
        i = skip_ws(b, end);
        if b.get(i) == Some(&b',') {
            i += 1;
        }
    }
    let quoted = serde_json::to_string(key).map_err(|err| err.to_string())?;
    match (found, last_end) {
        (Some((start, end, own)), _) => Ok(format!(
            "{}{}{}",
            &text[..start],
            pretty(&own)?,
            &text[end..]
        )),
        (None, Some(end)) => {
            let indent = indent.unwrap_or_else(|| "  ".into());
            let member = format!(",\n{indent}{quoted}: {}", pretty(&indent)?);
            Ok(format!("{}{member}{}", &text[..end], &text[end..]))
        }
        (None, None) => serde_json::to_string_pretty(&serde_json::json!({ key: value }))
            .map(|text| text + "\n")
            .map_err(|err| err.to_string()),
    }
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while b.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    i
}

/// Where the JSON value starting at `i` of valid JSON `b` ends.
fn value_end(b: &[u8], mut i: usize) -> usize {
    let (mut depth, mut in_string) = (0usize, false);
    while let Some(&c) = b.get(i) {
        i += 1;
        if in_string {
            match c {
                b'\\' => i += 1,
                b'"' if depth == 0 => return i,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            b'"' => in_string = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' if depth <= 1 => return if depth == 0 { i - 1 } else { i },
            b'}' | b']' => depth -= 1,
            b',' if depth == 0 => return i - 1,
            c if depth == 0 && c.is_ascii_whitespace() => return i - 1,
            _ => {}
        }
    }
    i
}

/// Put back what `changed` held before gerdr changed it.
pub fn undo(changed: &Changed) -> Result<(), String> {
    let path = &changed.path;
    match &changed.before {
        Some(text) => crate::projects::write_atomic(path, text.as_bytes()),
        None => std::fs::remove_file(path),
    }
    .map_err(|err| format!("{}: {err}", path.display()))
}

/// The command a hook runs this gerdr with: `<exe> <subcommand>`.
pub fn hook_command(subcommand: &str) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    let exe = exe.to_string_lossy();
    let exe = exe.strip_suffix(" (deleted)").unwrap_or(&exe);
    Ok(format!("{} {subcommand}", crate::desktop::shell_quote(exe)))
}

/// Whether hook `command` is a gerdr's, running its `subcommand`.
pub fn is_gerdr_command(command: &str, subcommand: &str) -> bool {
    let Some(program) = command.trim().strip_suffix(subcommand) else {
        return false;
    };
    let program = program.trim().trim_matches(|c| c == '\'' || c == '"');
    Path::new(live_path(program))
        .file_name()
        .is_some_and(|name| name == "gerdr")
}

/// `path` as /proc names a program run from it: without the " (deleted)"
/// it gets once the file is built again under the running program.
fn live_path(path: &str) -> &str {
    path.strip_suffix(" (deleted)").unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::live::tests::TempDir;

    #[test]
    fn a_key_set_leaves_the_rest_of_the_text_as_it_was() {
        let text = "{\n  \"z\": {\"b\": 1, \"a\": [1, \"}\"]},\n  \"hooks\": {\"x\": 1},\n  \"m\": \"\\\"\"\n}\n";
        let value = serde_json::json!({ "y": [2] });
        assert_eq!(
            with_key(text, "hooks", &value).unwrap(),
            "{\n  \"z\": {\"b\": 1, \"a\": [1, \"}\"]},\n  \"hooks\": {\n    \"y\": [\n      2\n    ]\n  },\n  \"m\": \"\\\"\"\n}\n"
        );
        let added = with_key("{\n  \"z\": true\n}", "p", &serde_json::json!(["a"])).unwrap();
        assert_eq!(added, "{\n  \"z\": true,\n  \"p\": [\n    \"a\"\n  ]\n}");
        assert_eq!(
            with_key(" {} ", "p", &serde_json::json!(1)).unwrap(),
            "{\n  \"p\": 1\n}\n"
        );
        assert!(with_key("[]", "p", &serde_json::json!(1)).is_err());
        assert!(with_key("{oops", "p", &serde_json::json!(1)).is_err());
    }

    #[test]
    fn install_changes_once_and_undo_puts_it_back() {
        let dir = TempDir::new();
        let path = dir.path().join("a/settings.json");
        let changed = install(&path, |_| Ok("{}".into())).unwrap().unwrap();
        assert_eq!(changed.before, None);
        assert_eq!(install(&path, |t| Ok(t.unwrap().into())).unwrap(), None);
        undo(&changed).unwrap();
        assert!(!path.exists());
    }
}

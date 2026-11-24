//! OpenCode's TUI plugin, for live word (see `super`): the plugin
//! (`assets/opencode/gerdr-live/tui.js`) runs in each TUI, in its pane, and
//! tells gerdr; `files` puts it in OpenCode's config.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// The plugin, as OpenCode loads it from its folder.
const PLUGIN: &str = include_str!("../../../assets/opencode/gerdr-live/tui.js");
/// How `cli.json` names the plugin: its folder, beside `cli.json`.
const NAME: &str = "./gerdr-live";

/// What to write under `config` (`~/.config/opencode`) for the plugin to
/// load, each path with a merge of what it holds now: the plugin's file,
/// and `cli.json` naming it.
#[allow(clippy::type_complexity)]
pub fn files(
    config: &Path,
) -> Vec<(
    PathBuf,
    Box<dyn FnOnce(Option<&str>) -> Result<String, String>>,
)> {
    vec![
        (
            config.join("gerdr-live/tui.js"),
            Box::new(|_| Ok(PLUGIN.to_owned())),
        ),
        (config.join("cli.json"), Box::new(cli_json)),
    ]
}

/// `existing`, the text of OpenCode's `cli.json`, with the plugin in its
/// `"plugins"`, all else kept as it is. Text that isn't a JSON object (a
/// `cli.json` with comments too) is an error: it is never overwritten.
fn cli_json(existing: Option<&str>) -> Result<String, String> {
    let Some(text) = existing.filter(|t| !t.trim().is_empty()) else {
        return serde_json::to_string_pretty(&json!({ "plugins": [NAME] }))
            .map(|text| text + "\n")
            .map_err(|err| err.to_string());
    };
    let root: Value = serde_json::from_str(text).map_err(|err| err.to_string())?;
    let object = root.as_object().ok_or("not a JSON object")?;
    let mut plugins = object.get("plugins").cloned().unwrap_or_else(|| json!([]));
    let list = plugins.as_array_mut().ok_or("\"plugins\" isn't a list")?;
    if list.iter().any(|p| p.as_str() == Some(NAME)) {
        return Ok(text.to_owned());
    }
    list.push(NAME.into());
    super::with_key(text, "plugins", &plugins)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plugin_joins_the_others_in_cli_json() {
        let theirs = r#"{
  "$schema": "https://opencode.ai/v2/cli.json",
  "theme": {
    "name": "light"
  },
  "plugins": ["./herdr-opencode"],
  "animations": true
}
"#;
        let out = cli_json(Some(theirs)).unwrap();
        let want = theirs.replace(
            r#""plugins": ["./herdr-opencode"],"#,
            "\"plugins\": [\n    \"./herdr-opencode\",\n    \"./gerdr-live\"\n  ],",
        );
        assert_eq!(out, want);
        assert_eq!(cli_json(Some(&out)).unwrap(), out);
        let none = cli_json(Some("{\n  \"theme\": \"dark\"\n}\n")).unwrap();
        assert_eq!(
            none,
            "{\n  \"theme\": \"dark\",\n  \"plugins\": [\n    \"./gerdr-live\"\n  ]\n}\n"
        );
        let fresh = cli_json(None).unwrap();
        assert_eq!(cli_json(Some(&fresh)).unwrap(), fresh);
        assert!(cli_json(Some("{oops")).is_err());
        assert!(cli_json(Some("// mine\n{}")).is_err());
        assert!(cli_json(Some(r#"{"plugins": "x"}"#)).is_err());
    }

    #[test]
    fn the_plugin_goes_in_its_own_folder() {
        let config = Path::new("/c");
        let files = files(config);
        let paths: Vec<&Path> = files.iter().map(|(p, _)| p.as_path()).collect();
        assert_eq!(
            paths,
            [config.join("gerdr-live/tui.js"), config.join("cli.json")]
        );
        let (_, plugin) = files.into_iter().next().unwrap();
        let text = plugin(Some("old")).unwrap();
        assert!(
            text.contains("gerdr-live-${hash}.sock"),
            "the socket as gerdr names it"
        );
    }
}

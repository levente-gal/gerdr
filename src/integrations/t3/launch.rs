//! Starting T3 Code's threads: drafts in its app, set to a model as
//! `instance/model` (see `inT3` in the extension's worker.js). It offers
//! the models its threads ran lately, then those its model list names.

use std::path::Path;

use crate::{stage_models::Offer, t3_agents::ThreadRow};

/// The file in T3 Code's home naming each driver's current models.
const MANIFEST: &str = "userdata/model-manifest.json";

/// T3 Code's provider instance and model in `id` (`claudeAgent/claude-opus-5-5`),
/// as its draft takes them; none for an id not so.
pub fn selection(id: &str) -> Option<(&str, &str)> {
    let (instance, model) = id.split_once('/')?;
    let (instance, model) = (instance.trim(), model.trim());
    (!instance.is_empty() && !model.is_empty()).then_some((instance, model))
}

/// The model of T3 Code's `id`, as its threads name it: `claude-opus-5-5`.
pub fn model_name(id: &str) -> &str {
    selection(id).map_or(id, |(_, model)| model)
}

/// What T3 Code's drafts can be set to: the models `rows` (its threads, the
/// newest first) ran, then those of each driver in its model list at `home`.
// shortcut: a thread's provider is taken as its instance, which holds for
// T3 Code's own instances; a custom one's models are typed until read apart.
pub fn offers(home: Option<&Path>, rows: &[ThreadRow]) -> Vec<Offer> {
    let used = (rows.iter()).filter_map(|r| Some((r.provider.clone()?, r.model.clone()?)));
    let listed = home.map_or_else(Vec::new, |home| match std::fs::read(home.join(MANIFEST)) {
        Ok(bytes) => manifest_models(&bytes),
        Err(err) => {
            tracing::warn!("reading T3 Code's model list: {err}");
            Vec::new()
        }
    });
    let mut offers: Vec<Offer> = Vec::new();
    for (instance, model) in used.chain(listed) {
        let id = format!("{instance}/{model}");
        if model != "default" && !offers.iter().any(|o| o.id == id) {
            offers.push(Offer { id, name: model });
        }
    }
    offers
}

/// The (driver, model) pairs in T3 Code's model list `bytes`.
fn manifest_models(bytes: &[u8]) -> Vec<(String, String)> {
    let json: serde_json::Value = match serde_json::from_slice(bytes) {
        Ok(json) => json,
        Err(err) => {
            tracing::warn!("reading T3 Code's model list: {err}");
            return Vec::new();
        }
    };
    let Some(drivers) = json["manifest"]["currentModels"].as_object() else {
        tracing::warn!("T3 Code's model list has no manifest.currentModels");
        return Vec::new();
    };
    (drivers.iter())
        .flat_map(|(driver, models)| {
            (models.as_array().into_iter().flatten())
                .filter_map(|m| Some((driver.clone(), m.as_str()?.to_owned())))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t3_offers_go_by_threads_then_its_list() {
        let dir = std::env::temp_dir().join(format!("gerdr-t3-offers-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("userdata")).unwrap();
        let manifest = r#"{"manifest":{"currentModels":{"claudeAgent":["claude-opus-5-5","claude-haiku-5-5"],"codex":["gpt-6-astra"]}}}"#;
        std::fs::write(dir.join(MANIFEST), manifest).unwrap();
        let row = |provider: &str, model: &str| ThreadRow {
            provider: Some(provider.into()),
            model: Some(model.into()),
            ..Default::default()
        };
        let rows = [
            row("cursor", "grok-4.7"),
            row("cursor", "default"),
            row("claudeAgent", "claude-haiku-5-5"),
        ];
        let ids: Vec<String> = offers(Some(&dir), &rows)
            .into_iter()
            .map(|o| o.id)
            .collect();
        assert_eq!(
            ids,
            [
                "cursor/grok-4.7",
                "claudeAgent/claude-haiku-5-5",
                "claudeAgent/claude-opus-5-5",
                "codex/gpt-6-astra"
            ]
        );
        // No list read: the threads' models only.
        assert_eq!(offers(None, &rows[..1])[0].name, "grok-4.7");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_t3_model_is_instance_then_model() {
        assert_eq!(selection("cursor/grok-4.7"), Some(("cursor", "grok-4.7")));
        assert_eq!(
            selection("openrouter/anthropic/claude"),
            Some(("openrouter", "anthropic/claude"))
        );
        assert_eq!(selection("grok-4.7"), None);
        assert_eq!(selection("/grok"), None);
        assert_eq!(model_name("cursor/grok-4.7"), "grok-4.7");
    }
}

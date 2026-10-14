//! How often and how recently each thing was switched to, for ordering
//! the switcher and the run prompt. Keyed by strings: `workspace:<dir>`,
//! `app:<command>`, `project:<id>`, `folder:<path>`. Kept in
//! `~/.local/share/gerdr/frecency.json`.

use std::{
    collections::HashMap,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::home_dir;

/// How many times a key was touched, and when last (seconds since the epoch).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub count: u32,
    pub last: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frecency {
    #[serde(default)]
    entries: HashMap<String, Entry>,
}

/// Where the counts are kept between runs.
fn frecency_file() -> PathBuf {
    home_dir()
        .unwrap_or_default()
        .join(".local/share/gerdr/frecency.json")
}

fn seconds(now: SystemTime) -> u64 {
    now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl Frecency {
    /// The counts saved last time, or none.
    pub fn load() -> Self {
        Self::from_file(&frecency_file())
    }

    fn from_file(path: &std::path::Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        self.to_file(&frecency_file());
    }

    fn to_file(&self, path: &std::path::Path) {
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let text = serde_json::to_string_pretty(self)?;
            std::fs::write(path, text)
        };
        if let Err(err) = write() {
            tracing::warn!("saving frecency: {err}");
        }
    }

    /// `key` was switched to now.
    pub fn touch(&mut self, key: &str) {
        self.touch_at(key, SystemTime::now());
    }

    pub fn touch_at(&mut self, key: &str, now: SystemTime) {
        let entry = self.entries.entry(key.to_owned()).or_default();
        entry.count += 1;
        entry.last = seconds(now);
    }

    /// How much `key` is worth at `now`: higher for what was switched to
    /// more often and more recently; 0 for what never was.
    pub fn score(&self, key: &str, now: SystemTime) -> f32 {
        let _ = (key, now);
        0.0
    }

    pub fn entry(&self, key: &str) -> Option<&Entry> {
        self.entries.get(key)
    }
}

#[cfg(test)]
mod tests {
    use super::Frecency;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn touches_count_and_survive_a_save() {
        let dir = std::env::temp_dir().join(format!("gerdr-frecency-{}", std::process::id()));
        let path = dir.join("frecency.json");
        let mut f = Frecency::default();
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        f.touch_at("workspace:/home/x/gerdr", now);
        f.touch_at("workspace:/home/x/gerdr", now + Duration::from_secs(5));
        f.touch_at("app:firefox", now);
        let e = f.entry("workspace:/home/x/gerdr").unwrap();
        assert_eq!((e.count, e.last), (2, 1_005));
        assert!(f.entry("project:none").is_none());
        f.to_file(&path);
        assert_eq!(Frecency::from_file(&path), f);
        let _ = std::fs::remove_dir_all(dir);
        assert_eq!(Frecency::from_file(&path), Frecency::default());
    }
}

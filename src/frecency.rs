//! How often and how recently each thing was switched to, for ordering
//! the switcher and the run prompt. Keyed by strings: `workspace:<dir>`,
//! `tab:<dir>:<index>`, `agent:<pane id>`, `app:<command>`, `project:<id>`,
//! `folder:<path>`. Kept in `~/.local/share/gerdr/frecency.json`.
//!
//! A key's score is its count of touches, halved for every `HALF_LIFE`
//! since the last: what was used often stays ahead for a while, what was
//! used just now comes first.

use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::home_dir;

/// How long until a key's score halves.
pub const HALF_LIFE: Duration = Duration::from_secs(3 * 24 * 60 * 60);

/// Below this score a key is forgotten when another is touched.
const FORGET: f32 = 0.01;

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

/// What a count is worth `age` seconds after its last touch.
fn decay(age: u64) -> f32 {
    0.5f32.powf(age as f32 / HALF_LIFE.as_secs_f32())
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
        let at = seconds(now);
        let entry = self.entries.entry(key.to_owned()).or_default();
        entry.count += 1;
        entry.last = at;
        // What decayed to nothing is dropped, so the file stays small.
        self.entries
            .retain(|_, e| e.count as f32 * decay(at.saturating_sub(e.last)) >= FORGET);
    }

    /// How much `key` is worth at `now`: higher for what was switched to
    /// more often and more recently; 0 for what never was.
    pub fn score(&self, key: &str, now: SystemTime) -> f32 {
        let Some(entry) = self.entries.get(key) else {
            return 0.0;
        };
        entry.count as f32 * decay(seconds(now).saturating_sub(entry.last))
    }

    pub fn entry(&self, key: &str) -> Option<&Entry> {
        self.entries.get(key)
    }

    /// Start from `keys`, the most recent first, as if each had been
    /// touched once, a second apart: for a history kept before there was
    /// frecency. Does nothing when any of them is known already.
    pub fn seed(&mut self, keys: &[String], now: SystemTime) {
        if keys.iter().any(|key| self.entries.contains_key(key)) {
            return;
        }
        let at = seconds(now);
        for (i, key) in keys.iter().enumerate() {
            self.entries.insert(
                key.clone(),
                Entry {
                    count: 1,
                    last: at.saturating_sub(i as u64),
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Frecency, HALF_LIFE};
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

    #[test]
    fn scores_decay_by_half_lives() {
        let mut f = Frecency::default();
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
        f.touch_at("a", now);
        f.touch_at("a", now);
        assert_eq!(f.score("a", now), 2.0);
        assert!((f.score("a", now + HALF_LIFE) - 1.0).abs() < 1e-3);
        assert!((f.score("a", now + 2 * HALF_LIFE) - 0.5).abs() < 1e-3);
        assert_eq!(f.score("never", now), 0.0);
        // Time running backwards (a clock change) doesn't blow the score up.
        assert_eq!(f.score("a", now - HALF_LIFE), 2.0);
    }

    #[test]
    fn recent_beats_frequent_after_a_while_and_the_forgotten_are_dropped() {
        let mut f = Frecency::default();
        let start = UNIX_EPOCH + Duration::from_secs(1_000_000);
        for _ in 0..10 {
            f.touch_at("often", start);
        }
        let soon = start + Duration::from_secs(3600);
        f.touch_at("once", soon);
        assert!(f.score("often", soon) > f.score("once", soon));
        let later = start + 12 * HALF_LIFE;
        f.touch_at("once", later);
        assert!(f.score("once", later) > f.score("often", later));
        // Touching anything much later forgets what has decayed to nothing.
        f.touch_at("new", later + 20 * HALF_LIFE);
        assert!(f.entry("often").is_none());
        assert!(f.entry("once").is_none());
        assert!(f.entry("new").is_some());
    }

    #[test]
    fn seeds_a_history_once() {
        let mut f = Frecency::default();
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let keys = vec!["app:imv".to_string(), "app:firefox".to_string()];
        f.seed(&keys, now);
        assert!(f.score("app:imv", now) > f.score("app:firefox", now));
        assert!(f.score("app:firefox", now) > 0.0);
        f.touch_at("app:firefox", now + Duration::from_secs(60));
        f.seed(&keys, now + Duration::from_secs(120));
        assert_eq!(f.entry("app:firefox").unwrap().count, 2);
        assert_eq!(f.entry("app:imv").unwrap().count, 1);
    }
}

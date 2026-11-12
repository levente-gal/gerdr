//! The time now and how long things took, in the words the chrome uses.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch; 0 for a clock set before it.
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// How long something took, shortly: `12s`, `2m 5s`, `1h 5m`.
pub fn took(took: Duration) -> String {
    match took.as_secs() {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m {}s", s / 60, s % 60),
        s => format!("{}h {}m", s / 3600, s / 60 % 60),
    }
}

/// How long `age` is, in its largest unit: `45s`, `4m`, `2h`, `3d`.
pub fn age(age: Duration) -> String {
    match age.as_secs() {
        s @ 0..60 => format!("{s}s"),
        s @ 60..3600 => format!("{}m", s / 60),
        s @ 3600..86400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86400),
    }
}

/// How long ago Unix second `then` was at `now`, for people: "just now",
/// "5m ago", "3h ago", "yesterday", "4d ago", …
pub fn ago(now: u64, then: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    // A clock set back puts `then` in the future.
    match now.saturating_sub(then) {
        s if s < MINUTE => "just now".to_owned(),
        s if s < HOUR => format!("{}m ago", s / MINUTE),
        s if s < DAY => format!("{}h ago", s / HOUR),
        s if s < 2 * DAY => "yesterday".to_owned(),
        s if s < 7 * DAY => format!("{}d ago", s / DAY),
        s if s < 35 * DAY => format!("{}w ago", s / (7 * DAY)),
        s => format!("{}mo ago", s / (30 * DAY)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn took_has_hours() {
        let s = Duration::from_secs;
        assert_eq!(took(s(12)), "12s");
        assert_eq!(took(s(125)), "2m 5s");
        assert_eq!(took(s(7200)), "2h 0m");
    }

    #[test]
    fn age_is_its_largest_unit() {
        let s = Duration::from_secs;
        assert_eq!(age(s(45)), "45s");
        assert_eq!(age(s(250)), "4m");
        assert_eq!(age(s(7300)), "2h");
        assert_eq!(age(s(3 * 86400)), "3d");
    }

    #[test]
    fn ago_reads_for_people() {
        assert_eq!(ago(100, 200), "just now");
        assert_eq!(ago(10_000, 10_000 - 300), "5m ago");
        assert_eq!(ago(200_000, 200_000 - 90_000), "yesterday");
    }
}

//! A sound when an agent needs you or finishes, in sight or not: the
//! system's sound theme, or files the `[notifications]` table names, played
//! by PipeWire's or PulseAudio's player off the event loop.

use crate::config::Sounds;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Where the sound theme keeps its sounds (`sound-theme-freedesktop`).
const THEME: &str = "/usr/share/sounds/freedesktop/stereo";

/// Players tried in order; each plays Ogg, WAV and FLAC.
const PLAYERS: [&str; 2] = ["pw-play", "paplay"];

/// Play the sound for an agent that needs input (`blocked`) or finished,
/// unless sounds are off. A file named in the settings that is gone plays
/// the theme's sound instead.
pub fn play(sounds: &Sounds, blocked: bool) {
    if !sounds.enabled {
        return;
    }
    let own = if blocked {
        &sounds.blocked
    } else {
        &sounds.done
    };
    let file = own
        .clone()
        .filter(|f| f.is_file())
        .unwrap_or_else(|| theme_sound(blocked));
    std::thread::spawn(move || {
        if !PLAYERS.iter().any(|player| plays(player, &file)) {
            tracing::warn!("no sound: neither {PLAYERS:?} played {}", file.display());
        }
    });
}

/// The theme's sound for needing input, or for finishing.
fn theme_sound(blocked: bool) -> PathBuf {
    Path::new(THEME).join(if blocked {
        "dialog-warning.oga"
    } else {
        "complete.oga"
    })
}

/// Whether `player` played `file` to its end.
fn plays(player: &str, file: &Path) -> bool {
    Command::new(player)
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

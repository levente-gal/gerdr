//! pi's live word: an extension of gerdr's (`assets/pi/gerdr-live.ts`) that
//! pi loads from its extensions folder and that tells gerdr on its socket.

use std::path::Path;

use crate::chat::live::{self, Changed};

/// pi's extension, as gerdr writes it to `~/.pi/agent/extensions/`.
const EXTENSION: &str = include_str!("../../../assets/pi/gerdr-live.ts");

/// pi loads every extension in its folder: gerdr's goes beside herdr's,
/// if pi is installed under home folder `home`.
pub fn install(home: &Path) -> Vec<Result<Option<Changed>, String>> {
    if !home.join(".pi/agent").is_dir() {
        return Vec::new();
    }
    let path = home.join(".pi/agent/extensions/gerdr-live.ts");
    vec![live::install(&path, |_| Ok(EXTENSION.to_owned()))]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pis_extension_names_the_socket_as_gerdr_does() {
        assert!(EXTENSION.contains("gerdr-live-${hash}.sock"));
        assert!(EXTENSION.contains("digest(\"hex\").slice(0, 16)"));
    }
}

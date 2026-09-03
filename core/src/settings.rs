//! Persisted settings: today, just the port. Stored as `settings.json` under this addon's own
//! directory (`<GW2>/addons/tyrian_companion_nexus/`, from `nexus::paths::get_addon_dir`), the
//! place Nexus hands every addon for exactly this purpose.
//!
//! This module never touches the network or a `nexus` API call itself, so it can be tested
//! against a plain temp directory instead of a running game.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::protocol::DEFAULT_PORT;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub port: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Self { port: DEFAULT_PORT }
    }
}

const FILE_NAME: &str = "settings.json";

/// Loads settings from `dir/settings.json`. A missing file, an unreadable one, or one that
/// does not parse all fall back to [`Settings::default`] rather than failing addon load: a
/// broken settings file should not be the reason alerts stop appearing.
pub fn load(dir: &Path) -> Settings {
    let path = dir.join(FILE_NAME);
    let Ok(contents) = fs::read_to_string(&path) else {
        return Settings::default();
    };
    serde_json::from_str(&contents).unwrap_or_default()
}

/// Writes settings to `dir/settings.json`, creating `dir` if needed. Returns `Err` on any I/O
/// or serialization failure; the caller decides how loud to be about it (the options panel
/// shows it, the background client just keeps the in-memory value either way).
pub fn save(dir: &Path, settings: Settings) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let contents = serde_json::to_string_pretty(&settings)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    fs::write(dir.join(FILE_NAME), contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tyrian-companion-nexus-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn missing_file_falls_back_to_default() {
        let dir = temp_dir("missing");
        assert_eq!(load(&dir), Settings::default());
    }

    #[test]
    fn round_trips_through_save_and_load() {
        let dir = temp_dir("roundtrip");
        let settings = Settings { port: 54321 };
        save(&dir, settings).expect("save succeeds");
        assert_eq!(load(&dir), settings);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn garbage_file_falls_back_to_default_instead_of_failing_load() {
        let dir = temp_dir("garbage");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(FILE_NAME), "not json at all").unwrap();
        assert_eq!(load(&dir), Settings::default());
        let _ = fs::remove_dir_all(&dir);
    }
}

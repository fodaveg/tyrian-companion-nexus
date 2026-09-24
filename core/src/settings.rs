//! Persisted settings: the port and the token. Stored as `settings.json` under this addon's own
//! directory (`<GW2>/addons/tyrian_companion_nexus/`, from `nexus::paths::get_addon_dir`), the
//! place Nexus hands every addon for exactly this purpose, and the place the SPEC names for the
//! Nexus addon's copy of the secret ("un fichero en su carpeta de addon").
//!
//! The token sits there in clear, like any addon setting (the SPEC's risk 3: a local process
//! that reads it can impersonate the addon). What this module does promise is that it never
//! prints it: `Settings`'s `Debug` redacts the value, so a stray `{:?}` in a log line cannot leak
//! it.
//!
//! This module never touches the network or a `nexus` API call itself, so it can be tested
//! against a plain temp directory instead of a running game.

use std::fmt;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::protocol::DEFAULT_PORT;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub port: u16,
    /// The shared secret copied from the plugin's settings. Empty until the user pastes it;
    /// `#[serde(default)]` keeps a v1 `settings.json` (port only) loading.
    #[serde(default)]
    pub token: String,
}

impl fmt::Debug for Settings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let token = if self.token.is_empty() { "<empty>" } else { "<redacted>" };
        formatter.debug_struct("Settings").field("port", &self.port).field("token", &token).finish()
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self { port: DEFAULT_PORT, token: String::new() }
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
/// logs it, the background client just keeps the in-memory value either way). The error never
/// carries the token: it comes from the file system, not from the contents.
pub fn save(dir: &Path, settings: &Settings) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let contents = serde_json::to_string_pretty(settings)
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
        let settings = Settings { port: 54321, token: "a".repeat(43) };
        save(&dir, &settings).expect("save succeeds");
        assert_eq!(load(&dir), settings);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_v1_file_with_only_the_port_still_loads() {
        let dir = temp_dir("v1");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(FILE_NAME), r#"{ "port": 50001 }"#).unwrap();
        assert_eq!(load(&dir), Settings { port: 50001, token: String::new() });
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

    #[test]
    fn debug_output_never_contains_the_token() {
        let secret = "SuperSecretTokenValue-0123456789abcdefghij";
        let printed = format!("{:?}", Settings { port: 1, token: secret.into() });
        assert!(!printed.contains(secret), "{printed}");
        assert!(printed.contains("<redacted>"));
    }
}

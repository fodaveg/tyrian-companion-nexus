//! Persisted settings: the port and the token. Stored as `settings.json` under this addon's own
//! directory (`<GW2>/addons/tyrian_companion_nexus/`, from `nexus::paths::get_addon_dir`), the
//! place Nexus hands every addon for exactly this purpose, and the place the SPEC names for the
//! Nexus addon's copy of the secret ("un fichero en su carpeta de addon").
//!
//! The token sits there in clear, like any addon setting (the SPEC's risk 3: a local process
//! that reads it can impersonate the addon). What this module does promise is that it never
//! prints it: `Settings`'s `Debug` redacts the value, so a stray `{:?}` in a log line cannot leak
//! it. And it never keeps a Guild Wars 2 API key there: 0.2.0 saved one that had been pasted into
//! the token field, so [`load`] drops it from an older file and rewrites the file without it.
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
    /// Whether this addon should try to open Obsidian on its own first failed connection
    /// attempt each load (`core::obsidian_launch`; David, 24 sep 2026: "si empiezo a jugar y
    /// está cerrado, que se abra"). `#[serde(default = "default_open_obsidian_on_start")]` keeps
    /// a `settings.json` from before this field existed loading as `true`, the same default a
    /// fresh install gets.
    #[serde(default = "default_open_obsidian_on_start")]
    pub open_obsidian_on_start: bool,
}

fn default_open_obsidian_on_start() -> bool {
    true
}

impl fmt::Debug for Settings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let token = if self.token.is_empty() { "<empty>" } else { "<redacted>" };
        formatter
            .debug_struct("Settings")
            .field("port", &self.port)
            .field("token", &token)
            .field("open_obsidian_on_start", &self.open_obsidian_on_start)
            .finish()
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self { port: DEFAULT_PORT, token: String::new(), open_obsidian_on_start: true }
    }
}

const FILE_NAME: &str = "settings.json";

/// What [`load`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub settings: Settings,
    /// The file held a Guild Wars 2 API key as the token. It is not in `settings`, and `load`
    /// has already tried to rewrite the file without it; the caller tells the user why the token
    /// is gone.
    pub discarded_api_key: bool,
}

/// Loads settings from `dir/settings.json`. A missing file, an unreadable one, or one that
/// does not parse all fall back to [`Settings::default`] rather than failing addon load: a
/// broken settings file should not be the reason alerts stop appearing.
///
/// A token with the shape of a Guild Wars 2 API key is dropped: it comes back empty, and the file
/// is rewritten at once so the key does not stay on disk. If that rewrite fails the key is still
/// not used; the failure is logged without the value.
pub fn load(dir: &Path) -> Loaded {
    let path = dir.join(FILE_NAME);
    let Ok(contents) = fs::read_to_string(&path) else {
        return Loaded { settings: Settings::default(), discarded_api_key: false };
    };
    let mut settings: Settings = serde_json::from_str(&contents).unwrap_or_default();
    if !crate::token::is_gw2_api_key(settings.token.trim()) {
        return Loaded { settings, discarded_api_key: false };
    }
    settings.token.clear();
    match save(dir, &settings) {
        Ok(()) => log::warn!("the saved token was a Guild Wars 2 API key; removed it from settings.json"),
        Err(error) => log::error!(
            "the saved token was a Guild Wars 2 API key; it is not used, but settings.json could not be rewritten \
             without it: {error}"
        ),
    }
    Loaded { settings, discarded_api_key: true }
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
        assert_eq!(load(&dir).settings, Settings::default());
    }

    #[test]
    fn round_trips_through_save_and_load() {
        let dir = temp_dir("roundtrip");
        let settings = Settings { port: 54321, token: "a".repeat(43), open_obsidian_on_start: false };
        save(&dir, &settings).expect("save succeeds");
        assert_eq!(load(&dir), Loaded { settings, discarded_api_key: false });
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_v1_file_with_only_the_port_still_loads() {
        let dir = temp_dir("v1");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(FILE_NAME), r#"{ "port": 50001 }"#).unwrap();
        assert_eq!(load(&dir).settings, Settings { port: 50001, ..Settings::default() });
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_from_before_open_obsidian_on_start_existed_loads_it_as_true() {
        // A pre-0.3.0 settings.json (port and token only, this addon's own v1/v2 shape) must
        // load with the same default a fresh install gets: David's decision (24 sep 2026) is
        // "si empiezo a jugar y está cerrado, que se abra", on by default.
        let dir = temp_dir("pre-open-obsidian-flag");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(FILE_NAME), format!(r#"{{ "port": 50003, "token": "{}" }}"#, "a".repeat(40))).unwrap();
        assert!(load(&dir).settings.open_obsidian_on_start);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn garbage_file_falls_back_to_default_instead_of_failing_load() {
        let dir = temp_dir("garbage");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(FILE_NAME), "not json at all").unwrap();
        assert_eq!(load(&dir).settings, Settings::default());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_saved_api_key_is_discarded_on_load_and_removed_from_disk() {
        // What 0.2.0 wrote when the API key was pasted into the token field, whitespace included.
        let api_key = "0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f90a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9";
        let dir = temp_dir("apikey");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(FILE_NAME), format!(r#"{{ "port": 50002, "token": " {api_key}\n" }}"#)).unwrap();

        let loaded = load(&dir);
        assert_eq!(
            loaded,
            Loaded { settings: Settings { port: 50002, ..Settings::default() }, discarded_api_key: true }
        );

        let on_disk = fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert!(!on_disk.to_lowercase().contains(api_key), "the key stayed on disk: {on_disk}");
        assert_eq!(load(&dir), Loaded { settings: loaded.settings, discarded_api_key: false }, "the port survives");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn debug_output_never_contains_the_token() {
        let secret = "SuperSecretTokenValue-0123456789abcdefghij";
        let printed = format!("{:?}", Settings { port: 1, token: secret.into(), ..Settings::default() });
        assert!(!printed.contains(secret), "{printed}");
        assert!(printed.contains("<redacted>"));
    }
}

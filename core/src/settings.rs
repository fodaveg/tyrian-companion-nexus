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
//! The file is the only copy of the token on this side, so two things protect it. [`save`]
//! replaces it whole or not at all. And a file that is there but cannot be loaded is left as it
//! is until the user saves again: [`SaveGuard`] holds everything that saves by itself, which
//! would otherwise write the defaults, and an empty token, over it.
//!
//! This module never touches the network or a `nexus` API call itself, so it can be tested
//! against a plain temp directory instead of a running game.

use std::fmt;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::obsidian_launch::LaunchApp;
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
    /// Which app that launch opens: Obsidian or Hebra (`core::obsidian_launch::LaunchApp`). The
    /// `open_obsidian_on_start` key above keeps its name on disk so an older file still loads;
    /// `#[serde(default)]` makes a file without this key mean Obsidian, as before.
    #[serde(default)]
    pub launch_app: LaunchApp,
    /// Optional read-only Labyrinth window; older settings leave it hidden.
    #[serde(default)]
    pub show_farming_panel: bool,
    /// Spanish by default; the Options panel can switch the farming window to English.
    #[serde(default)]
    pub farming_english: bool,
    /// The panel's own title bar button: no window background, only the text. Older settings
    /// leave it off.
    #[serde(default)]
    pub farming_panel_transparent: bool,
    /// The panel folded down to its title bar, with the same button the native bar had.
    #[serde(default)]
    pub farming_panel_collapsed: bool,
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
            .field("launch_app", &self.launch_app)
            .field("show_farming_panel", &self.show_farming_panel)
            .field("farming_english", &self.farming_english)
            .field("farming_panel_transparent", &self.farming_panel_transparent)
            .field("farming_panel_collapsed", &self.farming_panel_collapsed)
            .finish()
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self { port: DEFAULT_PORT, token: String::new(), open_obsidian_on_start: true, launch_app: LaunchApp::default(),
            show_farming_panel: false, farming_english: false, farming_panel_transparent: false, farming_panel_collapsed: false }
    }
}

const FILE_NAME: &str = "settings.json";

/// Where [`save`] writes the new contents before they take the place of `settings.json`.
const TEMPORARY_NAME: &str = "settings.json.tmp";

/// What [`load`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub settings: Settings,
    /// The file held a Guild Wars 2 API key as the token. It is not in `settings`, and `load`
    /// has already tried to rewrite the file without it; the caller tells the user why the token
    /// is gone.
    pub discarded_api_key: bool,
    /// The file is there and could not be used: it could not be read, or it does not parse.
    /// `settings` are the defaults and the file is as it was, with whatever it still holds, the
    /// token included. The caller must not let anything but the user's own Save replace it
    /// ([`SaveGuard`]). A file that is not there is a first run, not this.
    pub unreadable: bool,
}

/// Loads settings from `dir/settings.json`. A missing file, an unreadable one, or one that
/// does not parse all fall back to [`Settings::default`] rather than failing addon load: a
/// broken settings file should not be the reason alerts stop appearing.
///
/// The last two are told apart from the first in [`Loaded::unreadable`]: the defaults they fall
/// back to are not what the user had, and saving them by itself would lose what the file holds.
///
/// A token with the shape of a Guild Wars 2 API key is dropped: it comes back empty, and the file
/// is rewritten at once so the key does not stay on disk. If that rewrite fails the key is still
/// not used; the failure is logged without the value.
pub fn load(dir: &Path) -> Loaded {
    let unreadable = || Loaded { settings: Settings::default(), discarded_api_key: false, unreadable: true };
    let path = dir.join(FILE_NAME);
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Loaded { settings: Settings::default(), discarded_api_key: false, unreadable: false };
        }
        Err(error) => {
            log::error!("settings.json could not be read ({:?}); using default settings and leaving the file as it is", error.kind());
            return unreadable();
        }
    };
    let mut settings: Settings = match serde_json::from_str(&contents) {
        Ok(settings) => settings,
        Err(error) => {
            // Where and what kind, never the error's own text: it can quote a value of the file.
            log::error!(
                "settings.json does not parse ({:?} at line {}, column {}); using default settings and leaving the file as it is",
                error.classify(),
                error.line(),
                error.column()
            );
            return unreadable();
        }
    };
    if !crate::token::is_gw2_api_key(settings.token.trim()) {
        return Loaded { settings, discarded_api_key: false, unreadable: false };
    }
    settings.token.clear();
    match save(dir, &settings) {
        Ok(()) => log::warn!("the saved token was a Guild Wars 2 API key; removed it from settings.json"),
        Err(error) => log::error!(
            "the saved token was a Guild Wars 2 API key; it is not used, but settings.json could not be rewritten \
             without it: {error}"
        ),
    }
    Loaded { settings, discarded_api_key: true, unreadable: false }
}

/// Writes settings to `dir/settings.json`, creating `dir` if needed. Returns `Err` on any I/O
/// or serialization failure; the caller decides how loud to be about it (the options panel
/// logs it, the background client just keeps the in-memory value either way). The error never
/// carries the token: it comes from the file system, not from the contents.
///
/// The file is replaced whole or not at all: the new contents are written next to it, in
/// `settings.json.tmp`, and renamed over it. A write cut short, by the game being killed or a
/// full disk, then leaves the file of before instead of half of a new one that would not parse
/// on the next load. It is not flushed to the disk first: this runs on the frame that took the
/// click, and a power cut is not what it is for.
pub fn save(dir: &Path, settings: &Settings) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let contents = serde_json::to_string_pretty(settings)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let temporary = dir.join(TEMPORARY_NAME);
    let written = fs::write(&temporary, contents).and_then(|()| fs::rename(&temporary, dir.join(FILE_NAME)));
    if written.is_err() {
        // Whatever part of it was written holds the token: it does not stay behind.
        let _ = fs::remove_file(&temporary);
    }
    written
}

/// What asks for a save.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveRequest {
    /// The user pressed Save on the port and the token.
    Explicit,
    /// Anything that saves by itself: a checkbox, a button of the panel's bar, a quick access
    /// icon.
    Automatic,
}

/// What Options says while a [`SaveGuard`] holds, in the two languages the addon has.
pub const UNREADABLE_NOTICE: [&str; 2] = [
    "settings.json could not be read. The addon is using the default settings and has left the file as it is. \
     Nothing is saved until you paste the token and press Save, which replaces that file.",
    "No se pudo leer settings.json. El addon usa los ajustes por defecto y ha dejado el fichero como estaba. \
     No se guarda nada hasta que pegues el token y pulses Save, que reemplaza ese fichero.",
];

/// Keeps what saves by itself from replacing a settings file that could not be loaded.
///
/// After such a load the addon runs on the defaults, with no token. A checkbox or a button of
/// the panel's bar would then save those defaults over a file that may still hold the token and
/// everything else. While the guard holds, only [`SaveRequest::Explicit`] writes, and a write
/// that succeeds lifts the hold: from then on the file is what the user saved.
#[derive(Debug, Default)]
pub struct SaveGuard {
    held: std::sync::atomic::AtomicBool,
}

impl SaveGuard {
    pub const fn new() -> Self {
        Self { held: std::sync::atomic::AtomicBool::new(false) }
    }

    /// The load failed ([`Loaded::unreadable`]): hold every automatic save.
    pub fn hold(&self) {
        self.held.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether automatic saves are being held, which is when Options shows
    /// [`UNREADABLE_NOTICE`].
    pub fn held(&self) -> bool {
        self.held.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// [`save`], unless the guard holds and `request` is automatic. `Ok(true)` when the file was
    /// written, `Ok(false)` when the save was held and the file is as it was.
    pub fn save(&self, dir: &Path, settings: &Settings, request: SaveRequest) -> std::io::Result<bool> {
        if request == SaveRequest::Automatic && self.held() {
            return Ok(false);
        }
        save(dir, settings)?;
        self.held.store(false, std::sync::atomic::Ordering::Relaxed);
        Ok(true)
    }
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
        let settings = Settings { port: 54321, token: "a".repeat(43), open_obsidian_on_start: false, launch_app: LaunchApp::Hebra,
            show_farming_panel: true, farming_english: true, farming_panel_transparent: true, farming_panel_collapsed: true };
        save(&dir, &settings).expect("save succeeds");
        assert_eq!(load(&dir), Loaded { settings, discarded_api_key: false, unreadable: false });
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_from_before_the_panel_flags_existed_loads_them_off() {
        // A 0.7.2 settings.json: the panel comes back opaque and unfolded, as it was.
        let dir = temp_dir("pre-panel-flags");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(FILE_NAME), r#"{ "port": 50005, "show_farming_panel": true, "farming_english": true }"#).unwrap();
        let settings = load(&dir).settings;
        assert!(settings.show_farming_panel && settings.farming_english);
        assert!(!settings.farming_panel_transparent && !settings.farming_panel_collapsed);
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
    fn a_file_from_before_launch_app_existed_loads_it_as_obsidian() {
        // A 0.3.0 settings.json has `open_obsidian_on_start` but no `launch_app`: it must keep
        // opening Obsidian, and keep its own value of the flag, exactly as it did.
        let dir = temp_dir("pre-launch-app");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(FILE_NAME),
            format!(r#"{{ "port": 50004, "token": "{}", "open_obsidian_on_start": false }}"#, "a".repeat(40)),
        )
        .unwrap();
        let settings = load(&dir).settings;
        assert_eq!(settings.launch_app, LaunchApp::Obsidian);
        assert!(!settings.open_obsidian_on_start);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_chosen_app_is_written_under_the_launch_app_key_and_the_old_flag_key_stays() {
        let dir = temp_dir("launch-app-keys");
        let settings = Settings { launch_app: LaunchApp::Hebra, ..Settings::default() };
        save(&dir, &settings).expect("save succeeds");
        let written = fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert!(written.contains(r#""launch_app": "hebra""#), "{written}");
        assert!(written.contains(r#""open_obsidian_on_start": true"#), "{written}");
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

    /// A settings file as a write cut short would leave it: the token is in it, and it does not
    /// parse.
    fn cut_short() -> String {
        format!(r#"{{ "port": 50007, "token": "{}", "show_farming_"#, "t".repeat(43))
    }

    #[test]
    fn a_save_goes_through_a_temporary_file_and_leaves_none_behind() {
        let dir = temp_dir("atomic");
        let first = Settings { port: 50010, token: "a".repeat(43), ..Settings::default() };
        save(&dir, &first).expect("save succeeds");
        save(&dir, &Settings { port: 50011, ..first.clone() }).expect("a second save replaces the first");
        let names: Vec<_> = fs::read_dir(&dir).unwrap().map(|entry| entry.unwrap().file_name()).collect();
        assert_eq!(names, [std::ffi::OsString::from(FILE_NAME)], "only settings.json is left");
        assert_eq!(load(&dir).settings.port, 50011);

        // The new contents are never written in place: with the temporary file out of reach the
        // save fails and settings.json is still the one before, whole.
        let before = fs::read(dir.join(FILE_NAME)).unwrap();
        fs::create_dir(dir.join(TEMPORARY_NAME)).unwrap();
        assert!(save(&dir, &Settings { port: 50012, ..first }).is_err());
        assert_eq!(fs::read(dir.join(FILE_NAME)).unwrap(), before);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_reader_never_sees_half_a_settings_file_while_it_is_saved() {
        let dir = temp_dir("atomic-reader");
        let one = Settings { port: 50020, token: "a".repeat(128), ..Settings::default() };
        let other = Settings { port: 50021, token: "b".repeat(128), show_farming_panel: true, ..Settings::default() };
        save(&dir, &one).expect("save succeeds");
        let done = std::sync::atomic::AtomicBool::new(false);
        let (reads, torn, sizes) = std::thread::scope(|scope| {
            let reader = scope.spawn(|| {
                let (mut reads, mut torn, mut sizes) = (0u32, 0u32, std::collections::BTreeSet::new());
                while !done.load(std::sync::atomic::Ordering::Relaxed) {
                    // What `load` does with the file, without its fallback to the defaults.
                    let contents = fs::read_to_string(dir.join(FILE_NAME)).expect("the file is always there");
                    match serde_json::from_str::<Settings>(&contents) {
                        Ok(settings) => assert!(settings == one || settings == other),
                        Err(_) => {
                            torn += 1;
                            sizes.insert(contents.len());
                        }
                    }
                    reads += 1;
                }
                (reads, torn, sizes)
            });
            for round in 0..2_000 {
                save(&dir, if round % 2 == 0 { &other } else { &one }).expect("save succeeds");
            }
            done.store(true, std::sync::atomic::Ordering::Relaxed);
            reader.join().unwrap()
        });
        assert_eq!(torn, 0, "{torn} of {reads} reads saw a file that does not parse, of these sizes in bytes: {sizes:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_is_a_first_run_and_not_a_failed_load() {
        let dir = temp_dir("first-run");
        assert_eq!(load(&dir), Loaded { settings: Settings::default(), discarded_api_key: false, unreadable: false });
        // The addon's directory is there and the file is not: the same.
        fs::create_dir_all(&dir).unwrap();
        assert!(!load(&dir).unreadable);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_cannot_be_used_is_reported_and_left_as_it_is() {
        // It does not parse.
        let dir = temp_dir("unreadable");
        fs::create_dir_all(&dir).unwrap();
        for contents in [cut_short(), String::new(), "not json at all".to_string()] {
            fs::write(dir.join(FILE_NAME), &contents).unwrap();
            assert_eq!(load(&dir), Loaded { settings: Settings::default(), discarded_api_key: false, unreadable: true }, "{contents:?}");
            assert_eq!(fs::read_to_string(dir.join(FILE_NAME)).unwrap(), contents, "load does not touch it");
        }
        // It is not text.
        fs::write(dir.join(FILE_NAME), [0xff, 0xfe, 0x00, 0x7b]).unwrap();
        assert!(load(&dir).unreadable);
        // It cannot be read at all: here, because it is a directory.
        fs::remove_file(dir.join(FILE_NAME)).unwrap();
        fs::create_dir(dir.join(FILE_NAME)).unwrap();
        assert_eq!(load(&dir), Loaded { settings: Settings::default(), discarded_api_key: false, unreadable: true });
        let _ = fs::remove_dir_all(&dir);
    }

    /// The fault of the audit: a file that does not parse loads as the defaults, and the next
    /// thing that saves by itself (a checkbox, a button of the panel's bar, a quick access icon)
    /// wrote those defaults, with an empty token, over a file that still held the token.
    #[test]
    fn an_automatic_save_does_not_replace_a_file_that_failed_to_load() {
        let dir = temp_dir("held");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(FILE_NAME), cut_short()).unwrap();
        let loaded = load(&dir);
        let guard = SaveGuard::new();
        assert!(!guard.held());
        if loaded.unreadable {
            guard.hold();
        }
        assert!(guard.held());
        // What a click on "Show Labyrinth farming panel" would save: the defaults it loaded.
        let toggled = Settings { show_farming_panel: true, ..loaded.settings };
        for _ in 0..2 {
            assert!(!guard.save(&dir, &toggled, SaveRequest::Automatic).expect("held is not an error"));
            assert_eq!(fs::read_to_string(dir.join(FILE_NAME)).unwrap(), cut_short(), "the file is as it was");
            assert!(guard.held());
        }
        // The user pastes the token and presses Save: that replaces the file and lifts the hold.
        let pasted = Settings { token: "p".repeat(43), ..toggled };
        assert!(guard.save(&dir, &pasted, SaveRequest::Explicit).expect("save succeeds"));
        assert!(!guard.held());
        assert_eq!(load(&dir), Loaded { settings: pasted.clone(), discarded_api_key: false, unreadable: false });
        // From then on the automatic saves write again.
        let folded = Settings { farming_panel_collapsed: true, ..pasted };
        assert!(guard.save(&dir, &folded, SaveRequest::Automatic).expect("save succeeds"));
        assert_eq!(load(&dir).settings, folded);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_explicit_save_that_fails_keeps_the_hold() {
        let dir = temp_dir("held-failed");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(FILE_NAME), cut_short()).unwrap();
        let guard = SaveGuard::new();
        guard.hold();
        fs::create_dir(dir.join(TEMPORARY_NAME)).unwrap();
        assert!(guard.save(&dir, &Settings::default(), SaveRequest::Explicit).is_err());
        assert!(guard.held(), "nothing replaced the file, so nothing may yet");
        assert_eq!(fs::read_to_string(dir.join(FILE_NAME)).unwrap(), cut_short());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_guard_that_holds_nothing_saves_every_request() {
        let dir = temp_dir("not-held");
        let guard = SaveGuard::new();
        for (port, request) in [(50030, SaveRequest::Automatic), (50031, SaveRequest::Explicit)] {
            assert!(guard.save(&dir, &Settings { port, ..Settings::default() }, request).expect("save succeeds"));
            assert_eq!(load(&dir).settings.port, port);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_notice_of_a_held_save_is_in_both_languages_and_names_the_way_out() {
        let [english, spanish] = UNREADABLE_NOTICE;
        assert_ne!(english, spanish);
        for notice in UNREADABLE_NOTICE {
            assert!(notice.contains("settings.json") && notice.contains("Save"), "{notice}");
        }
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
            Loaded { settings: Settings { port: 50002, ..Settings::default() }, discarded_api_key: true, unreadable: false }
        );

        let on_disk = fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert!(!on_disk.to_lowercase().contains(api_key), "the key stayed on disk: {on_disk}");
        assert_eq!(load(&dir), Loaded { settings: loaded.settings, discarded_api_key: false, unreadable: false }, "the port survives");
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

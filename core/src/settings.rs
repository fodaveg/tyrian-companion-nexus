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
///
/// Call it once, when the addon loads and before anything can save: it clears the temporary
/// file a save writes, which at that moment can only be one left by a save that never finished.
pub fn load(dir: &Path) -> Loaded {
    let unreadable = || Loaded { settings: Settings::default(), discarded_api_key: false, unreadable: true };
    // A save that never finished, the game killed between its write and its rename, left its
    // temporary file behind: settings that never took effect, with the token in them. This
    // runs before anything of the addon can save, so whatever is there is that. It is removed
    // and never read; if it cannot be removed the load goes on all the same.
    let _ = fs::remove_file(dir.join(TEMPORARY_NAME));
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
/// on the next load. It is not flushed to the disk first: a power cut is not what it is for.
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

/// What Options says while the last save that was tried failed ([`SaveGuard::failed`]), in the
/// two languages the addon has.
pub const SAVE_FAILED_NOTICE: [&str; 2] = [
    "The settings could not be saved: settings.json is as it was before. What you changed is in \
     use until the game closes. Press Save to try again.",
    "No se pudieron guardar los ajustes: settings.json sigue como estaba. Lo que cambiaste vale \
     hasta que cierres el juego. Pulsa Save para intentarlo otra vez.",
];

/// A save's place in line: which settings are newer than which ([`SaveGuard::ticket`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaveTicket(u64);

/// How a save that did not fail ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Saved {
    /// The file is these settings now.
    Written,
    /// An automatic save while the guard holds: the file is as it was.
    Held,
    /// Newer settings were already written: these would have put older ones back.
    Superseded,
}

/// The one way to the settings file: who may replace it, and in which order.
///
/// **Who.** After a load that failed ([`Loaded::unreadable`]) the addon runs on the defaults,
/// with no token. A checkbox or a button of the panel's bar would then save those defaults over
/// a file that may still hold the token and everything else. While the guard holds, only
/// [`SaveRequest::Explicit`] writes, and a write that succeeds lifts the hold: from then on the
/// file is what the user saved.
///
/// **In which order.** A save is asked for on one of two threads, Nexus's input thread or the
/// render one, and the file is written after the lock the settings are read under has been
/// let go, so that nobody waits on the disk for that lock. Two saves can then reach the file in
/// the other order. Each takes a [`SaveTicket`] while it still holds that lock, and a save
/// whose ticket is older than what is on disk writes nothing. The writes themselves go one at
/// a time, so two of them never share `settings.json.tmp`.
#[derive(Debug, Default)]
pub struct SaveGuard {
    held: std::sync::atomic::AtomicBool,
    /// The newest settings that were asked to be saved are not on disk: the file is not what is
    /// in use. Kept next to `line`, where it is decided, so that a frame can ask without
    /// taking that lock.
    failed: std::sync::atomic::AtomicBool,
    /// The last ticket handed out.
    issued: std::sync::atomic::AtomicU64,
    /// What is on disk and what is not, and the lock every write is made under. Taken only to
    /// write: never by a frame that saves nothing.
    line: std::sync::Mutex<Line>,
}

/// Where the saves stand.
#[derive(Debug, Default)]
struct Line {
    /// The ticket of the settings on disk.
    written: u64,
    /// The newest settings that reached the disk and were refused by it, with their ticket.
    /// They are still what the user asked for last: the next save that gets through writes
    /// them, if its own are older, and until one does the notice stays.
    unwritten: Option<(u64, Settings)>,
}

impl SaveGuard {
    pub const fn new() -> Self {
        Self {
            held: std::sync::atomic::AtomicBool::new(false),
            failed: std::sync::atomic::AtomicBool::new(false),
            issued: std::sync::atomic::AtomicU64::new(0),
            line: std::sync::Mutex::new(Line { written: 0, unwritten: None }),
        }
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

    /// The place in line of a save whose settings are being read now. Take it while still
    /// holding the lock those settings are read under: that is what makes a later ticket mean
    /// later settings.
    pub fn ticket(&self) -> SaveTicket {
        SaveTicket(self.issued.fetch_add(1, std::sync::atomic::Ordering::Relaxed).saturating_add(1))
    }

    /// [`save`], unless the guard holds and `request` is automatic, or settings with a later
    /// `ticket` are already on disk. Call it with no other lock held: it waits for the disk,
    /// and for a write that is under way.
    ///
    /// What is written is the newest settings there are: these, or newer ones that an earlier
    /// save could not get onto the disk. Two saves can cross and the newer one fail; the older
    /// one, coming after it, then carries the newer settings instead of putting its own on
    /// disk under settings that are in use and not saved.
    pub fn save(&self, dir: &Path, settings: &Settings, request: SaveRequest, ticket: SaveTicket) -> std::io::Result<Saved> {
        let mut line = self.line.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        // Asked under the lock: an automatic save that waited for the explicit one that lifts
        // the hold goes through after it.
        if request == SaveRequest::Automatic && self.held() {
            return Ok(Saved::Held);
        }
        if ticket.0 < line.written {
            return Ok(Saved::Superseded);
        }
        let (newest, newest_settings) = match &line.unwritten {
            Some((refused, refused_settings)) if *refused > ticket.0 => (*refused, refused_settings),
            _ => (ticket.0, settings),
        };
        let saved = save(dir, newest_settings);
        if saved.is_ok() {
            line.written = newest;
            line.unwritten = None;
            self.held.store(false, std::sync::atomic::Ordering::Relaxed);
        } else if newest == ticket.0 {
            line.unwritten = Some((ticket.0, settings.clone()));
        }
        // Told to the user until what was asked for last is on disk: the error itself only
        // goes to the log, and the window that asked looks the same either way.
        self.failed.store(line.unwritten.is_some(), std::sync::atomic::Ordering::Relaxed);
        saved.map(|()| Saved::Written)
    }

    /// Whether the newest settings that were asked to be saved are not on disk, which is when
    /// Options shows [`SAVE_FAILED_NOTICE`]. A save that was held or overtaken changes nothing
    /// here, and neither does an older one that is written while a newer one is not: it is
    /// cleared when the newest settings there are get written.
    pub fn failed(&self) -> bool {
        self.failed.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// A save that could not even be tried: the caller has no directory to write in. Its
    /// settings are kept and told like those of one that failed on the disk, unless it is one
    /// the guard would have held anyway.
    pub fn could_not_try(&self, settings: &Settings, request: SaveRequest, ticket: SaveTicket) {
        let mut line = self.line.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if request == SaveRequest::Automatic && self.held() {
            return;
        }
        let newer = line.unwritten.as_ref().is_none_or(|(refused, _)| ticket.0 >= *refused);
        if ticket.0 >= line.written && newer {
            line.unwritten = Some((ticket.0, settings.clone()));
        }
        self.failed.store(line.unwritten.is_some(), std::sync::atomic::Ordering::Relaxed);
    }
}

/// One write for [`SaveWriter`] to make.
type SaveJob = Box<dyn FnOnce() + Send>;

/// A thread of its own for the writes, so the frame that took a click does not wait for the disk.
///
/// A save is asked for on the frame of a click, and the write and the rename wait for the disk:
/// on a cold or busy one that is a stutter in the middle of the game's drawing. The frame builds
/// the save (the settings, their [`SaveTicket`], the request) and hands the write to this thread
/// with [`SaveWriter::submit`]; the thread makes the writes one after the other, in the order
/// they were handed in. That order, and the tickets that [`SaveGuard`] puts in line, are what
/// keep the newest settings from being overtaken, so the last value a click set is the last one
/// written. A write that fails is told by the guard ([`SaveGuard::failed`]), not by who made it.
///
/// Nothing is lost on the way out: [`SaveWriter::finish`] takes every write that was handed in
/// to the file, then ends the thread, which is what lets the addon be unloaded after it. And a
/// save asked for while the thread is not running, before [`SaveWriter::start`] or after
/// `finish`, is written by whoever asked, as it always was.
pub struct SaveWriter {
    running: std::sync::Mutex<Option<WriterThread>>,
}

struct WriterThread {
    jobs: std::sync::mpsc::Sender<SaveJob>,
    thread: std::thread::JoinHandle<()>,
}

impl SaveWriter {
    pub const fn new() -> Self {
        Self { running: std::sync::Mutex::new(None) }
    }

    /// Starts the thread, unless it is running already. If it cannot be started the saves go on
    /// being written by whoever asks, and the error says why.
    pub fn start(&self) -> std::io::Result<()> {
        let mut running = self.running.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if running.is_some() {
            return Ok(());
        }
        let (jobs, queue) = std::sync::mpsc::channel::<SaveJob>();
        let thread = std::thread::Builder::new().name("tyrian-companion-nexus-settings".into()).spawn(move || {
            // Ends when the sender is dropped, once everything that was handed in has been run.
            for job in queue {
                // A job that panics must not take the writes behind it with it.
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
            }
        })?;
        *running = Some(WriterThread { jobs, thread });
        Ok(())
    }

    /// Has `job` run on the thread, after the ones handed in before it, without waiting for it.
    /// With no thread running it runs here and now.
    pub fn submit(&self, job: impl FnOnce() + Send + 'static) {
        let job: SaveJob = Box::new(job);
        let job = {
            let running = self.running.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            match running.as_ref() {
                // `send` only fails if the thread is gone; the job comes back in the error.
                Some(writer) => match writer.jobs.send(job) {
                    Ok(()) => return,
                    Err(failed) => failed.0,
                },
                None => job,
            }
        };
        job();
    }

    /// Runs every write that was handed in, ends the thread and waits for it. Saves asked for
    /// after this are written by whoever asks.
    pub fn finish(&self) {
        let writer = self.running.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
        if let Some(WriterThread { jobs, thread }) = writer {
            drop(jobs);
            let _ = thread.join();
        }
    }
}

impl Default for SaveWriter {
    fn default() -> Self {
        Self::new()
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
            assert_eq!(guard.save(&dir, &toggled, SaveRequest::Automatic, guard.ticket()).expect("held is not an error"), Saved::Held);
            assert_eq!(fs::read_to_string(dir.join(FILE_NAME)).unwrap(), cut_short(), "the file is as it was");
            assert!(guard.held());
        }
        // The user pastes the token and presses Save: that replaces the file and lifts the hold.
        let pasted = Settings { token: "p".repeat(43), ..toggled };
        assert_eq!(guard.save(&dir, &pasted, SaveRequest::Explicit, guard.ticket()).expect("save succeeds"), Saved::Written);
        assert!(!guard.held());
        assert_eq!(load(&dir), Loaded { settings: pasted.clone(), discarded_api_key: false, unreadable: false });
        // From then on the automatic saves write again.
        let folded = Settings { farming_panel_collapsed: true, ..pasted };
        assert_eq!(guard.save(&dir, &folded, SaveRequest::Automatic, guard.ticket()).expect("save succeeds"), Saved::Written);
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
        assert!(guard.save(&dir, &Settings::default(), SaveRequest::Explicit, guard.ticket()).is_err());
        assert!(guard.held(), "nothing replaced the file, so nothing may yet");
        assert_eq!(fs::read_to_string(dir.join(FILE_NAME)).unwrap(), cut_short());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_guard_that_holds_nothing_saves_every_request() {
        let dir = temp_dir("not-held");
        let guard = SaveGuard::new();
        for (port, request) in [(50030, SaveRequest::Automatic), (50031, SaveRequest::Explicit)] {
            assert_eq!(guard.save(&dir, &Settings { port, ..Settings::default() }, request, guard.ticket()).expect("save succeeds"), Saved::Written);
            assert_eq!(load(&dir).settings.port, port);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// The game can die between the write of the temporary file and the rename. The file of
    /// before is whole, and next to it stays a copy of the settings that never took its place,
    /// token included, which nothing would ever read or remove.
    #[test]
    fn a_temporary_file_left_by_a_save_that_never_finished_is_removed_on_load() {
        let dir = temp_dir("stale-temporary");
        let saved = Settings { port: 50050, token: "s".repeat(43), ..Settings::default() };
        save(&dir, &saved).expect("save succeeds");
        let never_renamed = serde_json::to_string_pretty(&Settings { port: 50051, token: "n".repeat(43), ..Settings::default() }).unwrap();
        fs::write(dir.join(TEMPORARY_NAME), &never_renamed).unwrap();
        assert_eq!(load(&dir), Loaded { settings: saved, discarded_api_key: false, unreadable: false }, "the file of before, not the temporary one");
        assert!(!dir.join(TEMPORARY_NAME).exists(), "the temporary file is still there, with its token");
        // With no settings.json at all it is still a first run: nothing is taken from it.
        fs::remove_file(dir.join(FILE_NAME)).unwrap();
        fs::write(dir.join(TEMPORARY_NAME), &never_renamed).unwrap();
        assert_eq!(load(&dir), Loaded { settings: Settings::default(), discarded_api_key: false, unreadable: false });
        assert!(!dir.join(TEMPORARY_NAME).exists());
        // One that cannot be removed is no reason for the load to fail, or to say the file is
        // unreadable: here it is a directory.
        fs::create_dir(dir.join(TEMPORARY_NAME)).unwrap();
        assert_eq!(load(&dir), Loaded { settings: Settings::default(), discarded_api_key: false, unreadable: false });
        let _ = fs::remove_dir_all(&dir);
    }

    /// A save that fails only went to the log, and the window that asked for it looks the same
    /// as after one that worked: the user was left thinking the settings were saved.
    #[test]
    fn a_save_that_fails_is_told_until_one_is_written() {
        let dir = temp_dir("save-failed");
        let guard = SaveGuard::new();
        let settings = Settings { port: 50060, ..Settings::default() };
        assert!(!guard.failed());
        assert_eq!(guard.save(&dir, &settings, SaveRequest::Explicit, guard.ticket()).expect("save succeeds"), Saved::Written);
        assert!(!guard.failed());
        // The disk refuses: here, the temporary file cannot be written.
        fs::create_dir(dir.join(TEMPORARY_NAME)).unwrap();
        for request in [SaveRequest::Explicit, SaveRequest::Automatic] {
            assert!(guard.save(&dir, &Settings { port: 50061, ..settings.clone() }, request, guard.ticket()).is_err());
            assert!(guard.failed(), "{request:?}");
        }
        assert_eq!(load(&dir).settings.port, 50060, "the file is the one of before");
        // A save that writes nothing says nothing about the disk: it is still to be told.
        let overtaken = guard.ticket();
        fs::remove_dir(dir.join(TEMPORARY_NAME)).unwrap();
        guard.hold();
        assert_eq!(guard.save(&dir, &settings, SaveRequest::Automatic, guard.ticket()).expect("not an error"), Saved::Held);
        assert!(guard.failed());
        // The next one that is written clears it.
        assert_eq!(guard.save(&dir, &Settings { port: 50062, ..settings.clone() }, SaveRequest::Explicit, guard.ticket()).expect("save succeeds"), Saved::Written);
        assert!(!guard.failed());
        assert_eq!(guard.save(&dir, &settings, SaveRequest::Automatic, overtaken).expect("not an error"), Saved::Superseded);
        assert!(!guard.failed());
        assert_eq!(load(&dir).settings.port, 50062);
        // No directory to save in is told the same way, and cleared the same way.
        let nowhere = Settings { port: 50063, ..settings.clone() };
        guard.could_not_try(&nowhere, SaveRequest::Automatic, guard.ticket());
        assert!(guard.failed());
        assert_eq!(guard.save(&dir, &settings, SaveRequest::Automatic, guard.ticket()).expect("save succeeds"), Saved::Written);
        assert!(!guard.failed());
        assert_eq!(load(&dir).settings.port, 50060, "the save after it is newer, and writes its own");
        // One the guard would have held anyway is not a failure to tell.
        guard.hold();
        guard.could_not_try(&nowhere, SaveRequest::Automatic, guard.ticket());
        assert!(!guard.failed());
        let [english, spanish] = SAVE_FAILED_NOTICE;
        assert_ne!(english, spanish);
        for notice in SAVE_FAILED_NOTICE {
            assert!(notice.contains("settings.json") && notice.contains("Save"), "{notice}");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// Two saves cross, and the newer one fails on the disk. The older one, which gets there
    /// after it, must not leave its own settings on disk and the notice off: what the user
    /// asked for last would be in use and not saved, with nothing saying so.
    #[test]
    fn an_older_save_that_works_does_not_hide_a_newer_one_that_failed() {
        let dir = temp_dir("newer-failed");
        let guard = SaveGuard::new();
        let start = Settings { port: 50070, ..Settings::default() };
        assert_eq!(guard.save(&dir, &start, SaveRequest::Explicit, guard.ticket()).expect("save succeeds"), Saved::Written);
        let older = (Settings { show_farming_panel: true, ..start.clone() }, guard.ticket());
        let newer = (Settings { farming_panel_collapsed: true, ..older.0.clone() }, guard.ticket());
        // The newer one reaches the disk first, and the disk refuses it.
        fs::create_dir(dir.join(TEMPORARY_NAME)).unwrap();
        assert!(guard.save(&dir, &newer.0, SaveRequest::Automatic, newer.1).is_err());
        assert!(guard.failed());
        fs::remove_dir(dir.join(TEMPORARY_NAME)).unwrap();
        // The older one comes after it and the disk takes it: what is written is the newest
        // settings there are, and only because of that the notice goes.
        assert_eq!(guard.save(&dir, &older.0, SaveRequest::Automatic, older.1).expect("save succeeds"), Saved::Written);
        assert_eq!(load(&dir).settings, newer.0, "what the user asked for last is what is on disk");
        assert!(!guard.failed());
        // The same when the disk goes on refusing: nothing is written and the notice stays,
        // whichever of the two is tried.
        let (third, fourth) = (guard.ticket(), guard.ticket());
        let latest = Settings { farming_english: true, ..newer.0.clone() };
        fs::create_dir(dir.join(TEMPORARY_NAME)).unwrap();
        assert!(guard.save(&dir, &latest, SaveRequest::Automatic, fourth).is_err());
        assert!(guard.save(&dir, &newer.0, SaveRequest::Automatic, third).is_err());
        assert!(guard.failed());
        assert_eq!(load(&dir).settings, newer.0);
        fs::remove_dir(dir.join(TEMPORARY_NAME)).unwrap();
        // A save newer than the one that failed carries its own settings, which are newer still.
        let newest = Settings { farming_panel_transparent: true, ..latest.clone() };
        assert_eq!(guard.save(&dir, &newest, SaveRequest::Automatic, guard.ticket()).expect("save succeeds"), Saved::Written);
        assert_eq!(load(&dir).settings, newest);
        assert!(!guard.failed());
        let _ = fs::remove_dir_all(&dir);
    }

    /// The file is written after the lock the settings were read under is let go, so two saves
    /// can reach it in the other order. The one with the older settings must not win.
    #[test]
    fn a_save_overtaken_by_a_newer_one_does_not_put_the_older_settings_back() {
        let dir = temp_dir("overtaken");
        let guard = SaveGuard::new();
        // The input thread shows the panel, then the frame folds it: two settings, in this order.
        let shown = (Settings { show_farming_panel: true, ..Settings::default() }, guard.ticket());
        let folded = (Settings { farming_panel_collapsed: true, ..shown.0.clone() }, guard.ticket());
        // The frame gets to the disk first.
        assert_eq!(guard.save(&dir, &folded.0, SaveRequest::Automatic, folded.1).expect("save succeeds"), Saved::Written);
        assert_eq!(guard.save(&dir, &shown.0, SaveRequest::Automatic, shown.1).expect("not an error"), Saved::Superseded);
        assert_eq!(load(&dir).settings, folded.0, "the newer settings are on disk");
        // An explicit save is in line like any other: older settings do not come back by it.
        assert_eq!(guard.save(&dir, &shown.0, SaveRequest::Explicit, shown.1).expect("not an error"), Saved::Superseded);
        assert_eq!(load(&dir).settings, folded.0);
        // In order, each one is written.
        let next = (Settings { farming_english: true, ..folded.0.clone() }, guard.ticket());
        assert_eq!(guard.save(&dir, &next.0, SaveRequest::Automatic, next.1).expect("save succeeds"), Saved::Written);
        assert_eq!(load(&dir).settings, next.0);
        // A newer save that failed is not on disk, and an older one after it is not overtaken:
        // it goes through, with the newer settings (the test below).
        let older = (Settings { port: 50040, ..next.0.clone() }, guard.ticket());
        let newest = (Settings { port: 50041, ..next.0.clone() }, guard.ticket());
        fs::create_dir(dir.join(TEMPORARY_NAME)).unwrap();
        assert!(guard.save(&dir, &newest.0, SaveRequest::Automatic, newest.1).is_err());
        fs::remove_dir(dir.join(TEMPORARY_NAME)).unwrap();
        assert_eq!(guard.save(&dir, &older.0, SaveRequest::Automatic, older.1).expect("save succeeds"), Saved::Written);
        assert_eq!(load(&dir).settings.port, 50041);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Eight threads that change the settings under a lock, let it go and save: what ends on
    /// disk is the last change, and no save trips over another's temporary file.
    #[test]
    fn saves_from_several_threads_end_with_the_newest_settings_and_never_share_the_temporary_file() {
        let dir = temp_dir("crossing");
        let guard = SaveGuard::new();
        // What the addon's `PENDING` is: the settings, and the lock they are read under.
        let applied = std::sync::Mutex::new(20_000u16);
        save(&dir, &Settings { port: 20_000, ..Settings::default() }).expect("save succeeds");
        let outcomes = std::thread::scope(|scope| {
            let threads: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        let (mut written, mut superseded, mut failed) = (0u32, 0u32, Vec::new());
                        for _ in 0..200 {
                            let (settings, ticket) = {
                                let mut port = applied.lock().unwrap();
                                *port += 1;
                                (Settings { port: *port, ..Settings::default() }, guard.ticket())
                            };
                            // One save in three is slow to reach the disk, and is overtaken
                            // by the ones asked for after it.
                            if ticket.0 % 3 == 0 {
                                std::thread::sleep(std::time::Duration::from_micros(300));
                            }
                            match guard.save(&dir, &settings, SaveRequest::Automatic, ticket) {
                                Ok(Saved::Written) => written += 1,
                                Ok(Saved::Superseded) => superseded += 1,
                                Ok(Saved::Held) => failed.push("held".to_string()),
                                Err(error) => failed.push(error.to_string()),
                            }
                        }
                        (written, superseded, failed)
                    })
                })
                .collect();
            threads.into_iter().map(|thread| thread.join().unwrap()).collect::<Vec<_>>()
        });
        let failed: Vec<_> = outcomes.iter().flat_map(|(_, _, failed)| failed.iter()).collect();
        assert!(failed.is_empty(), "{} saves failed: {:?}", failed.len(), &failed[..failed.len().min(3)]);
        let (written, superseded) = outcomes.iter().fold((0, 0), |(written, superseded), outcome| (written + outcome.0, superseded + outcome.1));
        assert_eq!(written + superseded, 1_600);
        assert_eq!(load(&dir).settings.port, 21_600, "the last change is the one on disk ({written} written, {superseded} superseded)");
        assert!(superseded > 0, "no save was overtaken, so the order was never put to the test");
        let names: Vec<_> = fs::read_dir(&dir).unwrap().map(|entry| entry.unwrap().file_name()).collect();
        assert_eq!(names, [std::ffi::OsString::from(FILE_NAME)], "no temporary file is left");
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

    /// The frame that took a click hands the write over and goes on: it does not wait for a
    /// write that is under way, and the writes run in the order they were handed in.
    #[test]
    fn a_write_handed_to_the_writer_does_not_make_the_caller_wait_and_runs_in_order() {
        use std::sync::{mpsc, Arc, Mutex};
        let writer = SaveWriter::new();
        writer.start().expect("the thread starts");
        let ran = Arc::new(Mutex::new(Vec::new()));
        let (release, held) = mpsc::channel::<()>();
        let (started, has_started) = mpsc::channel::<()>();
        let caller = std::thread::current().id();
        let first = Arc::clone(&ran);
        writer.submit(move || {
            started.send(()).unwrap();
            // The disk, as slow as it can be: until the test lets it go.
            held.recv().unwrap();
            first.lock().unwrap().push((1, std::thread::current().id()));
        });
        has_started.recv().unwrap();
        for n in 2..=4 {
            let ran = Arc::clone(&ran);
            writer.submit(move || ran.lock().unwrap().push((n, std::thread::current().id())));
        }
        // Handed in, with the first one still on the disk: nothing has run, nobody has waited.
        assert!(ran.lock().unwrap().is_empty());
        release.send(()).unwrap();
        writer.finish();
        let ran = ran.lock().unwrap();
        assert_eq!(ran.iter().map(|(n, _)| *n).collect::<Vec<_>>(), [1, 2, 3, 4]);
        assert!(ran.iter().all(|(_, thread)| *thread != caller), "the caller wrote");
    }

    /// With no thread, a save is written by whoever asks: before the addon starts it and after
    /// it has finished it, a click is never dropped.
    #[test]
    fn with_no_thread_running_the_caller_writes() {
        use std::sync::{Arc, Mutex};
        let writer = SaveWriter::new();
        let ran = Arc::new(Mutex::new(Vec::new()));
        let note = |ran: &Arc<Mutex<Vec<std::thread::ThreadId>>>| {
            let ran = Arc::clone(ran);
            move || ran.lock().unwrap().push(std::thread::current().id())
        };
        writer.submit(note(&ran));
        writer.start().unwrap();
        writer.start().unwrap(); // A second start changes nothing.
        writer.finish();
        writer.finish(); // Nor does a second finish.
        writer.submit(note(&ran));
        let ran = ran.lock().unwrap();
        assert_eq!(*ran, [std::thread::current().id(); 2]);
    }

    /// What the guard promises does not depend on who writes: the last of many clicks is what
    /// is on disk when the writer finishes, and a write the disk refused is still told.
    #[test]
    fn saves_through_the_writer_end_with_the_last_settings_and_tell_a_failure() {
        use std::sync::Arc;
        let dir = temp_dir("writer");
        let guard = Arc::new(SaveGuard::new());
        let writer = SaveWriter::new();
        writer.start().unwrap();
        let ask = |settings: Settings, request| {
            let (guard, dir) = (Arc::clone(&guard), dir.clone());
            let ticket = guard.ticket();
            writer.submit(move || {
                let _ = guard.save(&dir, &settings, request, ticket);
            });
        };
        for port in 50000..50100 {
            ask(Settings { port, ..Settings::default() }, SaveRequest::Automatic);
        }
        writer.finish();
        assert_eq!(load(&dir).settings.port, 50099, "the last click is the file");
        assert!(!guard.failed());
        // The disk refuses the next one: nobody waited for it, and the notice is up all the same.
        writer.start().unwrap();
        fs::create_dir(dir.join(TEMPORARY_NAME)).unwrap();
        ask(Settings { port: 50100, ..Settings::default() }, SaveRequest::Automatic);
        writer.finish();
        assert!(guard.failed());
        assert_eq!(load(&dir).settings.port, 50099, "the file is as it was");
        // The save after it gets through, and writes the newest there are.
        fs::remove_dir(dir.join(TEMPORARY_NAME)).unwrap();
        writer.start().unwrap();
        ask(Settings { port: 50101, ..Settings::default() }, SaveRequest::Automatic);
        writer.finish();
        assert!(!guard.failed());
        assert_eq!(load(&dir).settings.port, 50101);
        let _ = fs::remove_dir_all(&dir);
    }
}

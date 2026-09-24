//! Shared, thread-safe state the background client thread and the (optional) ImGui options
//! panel both touch. Nexus's `render!` macro coerces a render callback to a plain `fn(&Ui)`
//! (see `nexus_codegen`'s `render.rs`: `const __CALLBACK: fn(&Ui) = $callback`), so a
//! capturing closure cannot carry state into it — everything the panel reads or writes has to
//! live in a `static`, which is what [`shared`] is. Tests build their own [`SharedState`].

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicU8, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::protocol::{Alert, AlertKind, DEFAULT_PORT};

/// What the client is doing right now, for the panel's status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
    /// No plugin listening yet, or between reconnections. The normal state right after launch.
    WaitingForPlugin = 0,
    /// The plugin accepted the token and sent its `welcome`.
    Connected = 1,
    /// No usable token has been saved: the client does not connect.
    MissingToken = 2,
    /// The plugin answered `auth_rejected`: the client waits for a new token.
    TokenRejected = 3,
    /// The plugin answered `version_unsupported`: the client waits for the settings to change.
    UpdateRequired = 4,
    /// The game window is closing: the client said `bye` and does not reconnect.
    GameExiting = 5,
}

impl Status {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Connected,
            2 => Self::MissingToken,
            3 => Self::TokenRejected,
            4 => Self::UpdateRequired,
            5 => Self::GameExiting,
            _ => Self::WaitingForPlugin,
        }
    }
}

/// One entry in the small in-memory history the options panel displays. Independent of the
/// native `GUI_SendAlert` call, which paints `content` on its own and does not need this.
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub kind: AlertKind,
    pub content: String,
}

/// Bound on how many recent alerts the panel keeps, so a burst of loot does not grow this
/// forever.
const HISTORY_CAPACITY: usize = 20;

pub struct SharedState {
    /// Current port, read by the client thread before each connection attempt and written by
    /// the options panel. Changing it takes effect on the *next* (re)connection attempt; it
    /// does not tear down a live connection (documented in the README).
    port: AtomicU16,
    /// The shared secret the user pasted. Never logged, never shown back in the panel.
    token: Mutex<String>,
    /// Bumped every time the user saves the settings. The client compares it to the value it
    /// saw when the plugin refused it, which is how "do not retry until the settings change"
    /// works without the panel having to know anything about the client.
    settings_generation: AtomicU64,
    status: AtomicU8,
    /// Last alert acted on, as `(server, seq)`. The plugin's `seq` restarts at 1 whenever
    /// Obsidian restarts, and its `server` id changes at the same time, so the pair is what
    /// identifies an alert; `seq` alone dropped every alert after a plugin restart (the bug of
    /// Blish's module 0.1.0 the SPEC names).
    last_alert: Mutex<Option<(String, u64)>>,
    /// Each flag lets one message show exactly once per load of the addon.
    warned_about_version: AtomicBool,
    warned_about_missing_token: AtomicBool,
    history: Mutex<VecDeque<HistoryEntry>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Default for SharedState {
    fn default() -> Self {
        Self::new()
    }
}

impl SharedState {
    pub fn new() -> Self {
        Self {
            port: AtomicU16::new(DEFAULT_PORT),
            token: Mutex::new(String::new()),
            settings_generation: AtomicU64::new(0),
            status: AtomicU8::new(Status::WaitingForPlugin as u8),
            last_alert: Mutex::new(None),
            warned_about_version: AtomicBool::new(false),
            warned_about_missing_token: AtomicBool::new(false),
            history: Mutex::new(VecDeque::with_capacity(HISTORY_CAPACITY)),
        }
    }

    pub fn port(&self) -> u16 {
        self.port.load(Ordering::Relaxed)
    }

    /// A copy of the saved token. The caller must not log it.
    pub fn token(&self) -> String {
        lock(&self.token).clone()
    }

    /// Replaces the port and the token together and marks the settings as changed, which lifts
    /// a stop caused by `auth_rejected` or `version_unsupported`.
    pub fn apply_settings(&self, port: u16, token: &str) {
        self.port.store(port, Ordering::Relaxed);
        *lock(&self.token) = token.to_string();
        self.settings_generation.fetch_add(1, Ordering::Relaxed);
    }

    pub fn settings_generation(&self) -> u64 {
        self.settings_generation.load(Ordering::Relaxed)
    }

    pub fn status(&self) -> Status {
        Status::from_u8(self.status.load(Ordering::Relaxed))
    }

    pub fn set_status(&self, status: Status) {
        self.status.store(status as u8, Ordering::Relaxed);
    }

    pub fn connected(&self) -> bool {
        self.status() == Status::Connected
    }

    /// `true` the first time it is called; `false` on every call after, for the lifetime of
    /// this addon's load. Lets the caller show the "update the addon" alert exactly once.
    pub fn warn_about_version_once(&self) -> bool {
        self.warned_about_version.compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed).is_ok()
    }

    /// Same as [`SharedState::warn_about_version_once`], for the "paste the token" alert.
    pub fn warn_about_missing_token_once(&self) -> bool {
        self.warned_about_missing_token.compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed).is_ok()
    }

    /// `true` if the alert `(server, seq)` is new and should be shown; `false` if this server has
    /// already had this `seq` or a later one acted on. An alert without a `seq` is always shown.
    /// A different `server` means the plugin restarted, so its counter starts over and the new
    /// alert is accepted whatever its number.
    pub fn accept_alert(&self, server: &str, seq: Option<u64>) -> bool {
        let Some(seq) = seq else { return true };
        let mut last = lock(&self.last_alert);
        if let Some((last_server, last_seq)) = last.as_ref() {
            if last_server == server && seq <= *last_seq {
                return false;
            }
        }
        *last = Some((server.to_string(), seq));
        true
    }

    pub fn push_history(&self, alert: &Alert) {
        let mut history = lock(&self.history);
        if history.len() == HISTORY_CAPACITY {
            history.pop_front();
        }
        history.push_back(HistoryEntry { kind: alert.kind, content: alert.content.clone() });
    }

    /// Most recent first.
    pub fn history_snapshot(&self) -> Vec<HistoryEntry> {
        lock(&self.history).iter().rev().cloned().collect()
    }
}

static STATE: OnceLock<SharedState> = OnceLock::new();

pub fn shared() -> &'static SharedState {
    STATE.get_or_init(SharedState::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::AlertKind;

    fn alert(seq: Option<u64>) -> Alert {
        Alert {
            seq,
            kind: AlertKind::ValuableLoot,
            name: "Mystic Coin".into(),
            quantity: 1,
            total_copper: Some(1),
            content: "content".into(),
        }
    }

    #[test]
    fn accepts_an_increasing_sequence_from_one_server() {
        let state = SharedState::new();
        assert!(state.accept_alert("A", Some(1)));
        assert!(state.accept_alert("A", Some(2)));
        assert!(state.accept_alert("A", Some(3)));
    }

    #[test]
    fn rejects_a_repeat_or_older_sequence_from_the_same_server() {
        let state = SharedState::new();
        assert!(state.accept_alert("A", Some(5)));
        assert!(!state.accept_alert("A", Some(5)));
        assert!(!state.accept_alert("A", Some(3)));
        assert!(state.accept_alert("A", Some(6)));
    }

    #[test]
    fn a_new_server_starts_its_sequence_over() {
        // Obsidian restarted: `seq` is back at 1 under a new `server`, and must show.
        let state = SharedState::new();
        assert!(state.accept_alert("A", Some(40)));
        assert!(state.accept_alert("B", Some(1)));
        assert!(state.accept_alert("B", Some(2)));
        assert!(!state.accept_alert("B", Some(2)));
    }

    #[test]
    fn no_sequence_number_always_gets_accepted() {
        let state = SharedState::new();
        assert!(state.accept_alert("A", None));
        assert!(state.accept_alert("A", None));
    }

    #[test]
    fn saving_settings_bumps_the_generation() {
        let state = SharedState::new();
        let before = state.settings_generation();
        state.apply_settings(50000, "t");
        assert_eq!(state.port(), 50000);
        assert_eq!(state.token(), "t");
        assert_eq!(state.settings_generation(), before + 1);
    }

    #[test]
    fn status_round_trips() {
        let state = SharedState::new();
        assert_eq!(state.status(), Status::WaitingForPlugin);
        for status in [Status::Connected, Status::MissingToken, Status::TokenRejected, Status::UpdateRequired, Status::GameExiting] {
            state.set_status(status);
            assert_eq!(state.status(), status);
        }
        state.set_status(Status::Connected);
        assert!(state.connected());
    }

    #[test]
    fn each_warning_fires_exactly_once() {
        let state = SharedState::new();
        assert!(state.warn_about_version_once());
        assert!(!state.warn_about_version_once());
        assert!(state.warn_about_missing_token_once());
        assert!(!state.warn_about_missing_token_once());
    }

    #[test]
    fn history_keeps_only_the_most_recent_entries() {
        let state = SharedState::new();
        for seq in 0..(HISTORY_CAPACITY as u64 + 5) {
            state.push_history(&alert(Some(seq)));
        }
        assert_eq!(state.history_snapshot().len(), HISTORY_CAPACITY);
    }
}

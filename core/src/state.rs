//! Shared, thread-safe state the background client thread and the (optional) ImGui options
//! panel both touch. Nexus's `render!` macro coerces a render callback to a plain `fn(&Ui)`
//! (see `nexus_codegen`'s `render.rs`: `const __CALLBACK: fn(&Ui) = $callback`), so a
//! capturing closure cannot carry state into it — everything the panel reads or writes has to
//! live in a `static`, which is what this module is.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::protocol::{Alert, AlertKind, DEFAULT_PORT};

/// Sentinel meaning "no sequence number seen yet" for the `AtomicU64` below.
const NO_SEQ: u64 = u64::MAX;

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
    /// Whether a client is currently connected to the plugin, for the panel's status line.
    connected: AtomicBool,
    /// Highest `seq` acted on so far, or [`NO_SEQ`] if none yet. A per-process counter from
    /// the plugin, used only to drop a duplicate, never to detect gaps or request replay.
    last_seq: AtomicU64,
    /// Set once this addon has shown the "update the addon" message, so it shows it exactly
    /// once for the lifetime of the load, not once per oversized-version line.
    warned_about_version: AtomicBool,
    history: Mutex<VecDeque<HistoryEntry>>,
}

impl SharedState {
    fn new() -> Self {
        Self {
            port: AtomicU16::new(DEFAULT_PORT),
            connected: AtomicBool::new(false),
            last_seq: AtomicU64::new(NO_SEQ),
            warned_about_version: AtomicBool::new(false),
            history: Mutex::new(VecDeque::with_capacity(HISTORY_CAPACITY)),
        }
    }

    pub fn port(&self) -> u16 {
        self.port.load(Ordering::Relaxed)
    }

    pub fn set_port(&self, port: u16) {
        self.port.store(port, Ordering::Relaxed);
    }

    pub fn set_connected(&self, connected: bool) {
        self.connected.store(connected, Ordering::Relaxed);
    }

    pub fn connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// `true` the first time it is called; `false` on every call after, for the lifetime of
    /// this addon's load. Lets the caller show the "update the addon" alert exactly once.
    pub fn warn_about_version_once(&self) -> bool {
        self.warned_about_version.compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed).is_ok()
    }

    /// `true` if `seq` is new (or no `seq` was given, so dedup does not apply) and should be
    /// acted on; `false` if it is a duplicate or older than one already acted on.
    pub fn accept_seq(&self, seq: Option<u64>) -> bool {
        let Some(seq) = seq else { return true };
        loop {
            let last = self.last_seq.load(Ordering::Relaxed);
            if last != NO_SEQ && seq <= last {
                return false;
            }
            if self.last_seq.compare_exchange(last, seq, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
                return true;
            }
        }
    }

    pub fn push_history(&self, alert: &Alert) {
        let mut history = self.history.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if history.len() == HISTORY_CAPACITY {
            history.pop_front();
        }
        history.push_back(HistoryEntry { kind: alert.kind, content: alert.content.clone() });
    }

    /// Most recent first.
    pub fn history_snapshot(&self) -> Vec<HistoryEntry> {
        let history = self.history.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        history.iter().rev().cloned().collect()
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
    fn accepts_an_increasing_sequence() {
        let state = SharedState::new();
        assert!(state.accept_seq(Some(1)));
        assert!(state.accept_seq(Some(2)));
        assert!(state.accept_seq(Some(3)));
    }

    #[test]
    fn rejects_a_repeat_or_older_sequence() {
        let state = SharedState::new();
        assert!(state.accept_seq(Some(5)));
        assert!(!state.accept_seq(Some(5)));
        assert!(!state.accept_seq(Some(3)));
        assert!(state.accept_seq(Some(6)));
    }

    #[test]
    fn no_sequence_number_always_gets_accepted() {
        let state = SharedState::new();
        assert!(state.accept_seq(None));
        assert!(state.accept_seq(None));
    }

    #[test]
    fn version_warning_fires_exactly_once() {
        let state = SharedState::new();
        assert!(state.warn_about_version_once());
        assert!(!state.warn_about_version_once());
        assert!(!state.warn_about_version_once());
    }

    #[test]
    fn history_keeps_only_the_most_recent_entries() {
        let state = SharedState::new();
        for seq in 0..(HISTORY_CAPACITY as u64 + 5) {
            state.push_history(&alert(Some(seq)));
        }
        let snapshot = state.history_snapshot();
        assert_eq!(snapshot.len(), HISTORY_CAPACITY);
    }
}

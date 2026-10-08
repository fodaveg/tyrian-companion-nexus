//! Shared, thread-safe state the background client thread and the (optional) ImGui options
//! panel both touch. Nexus's `render!` macro coerces a render callback to a plain `fn(&Ui)`
//! (see `nexus_codegen`'s `render.rs`: `const __CALLBACK: fn(&Ui) = $callback`), so a
//! capturing closure cannot carry state into it — everything the panel reads or writes has to
//! live in a `static`, which is what [`shared`] is. Tests build their own [`SharedState`].

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicU8, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use crate::farming::{FarmingFeed, FarmingState, FarmingView};
use crate::price::{PriceFeed, PriceState, PriceView};

use crate::obsidian_launch::{LaunchApp, ObsidianLaunchOutcome};
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
    farming: Mutex<FarmingFeed>,
    price: Mutex<PriceFeed>,
    live_status: Mutex<crate::live::LiveStatus>,
    live_context: Mutex<Option<crate::protocol::GameContext>>,
    /// The last capture's diagnostics and when that capture ran. They only change when a
    /// capture runs, so the instant is what tells a reading of now from one that is still here
    /// because nothing replaced it.
    inventory_diagnostics: Mutex<(crate::inventory::Diagnostics, Option<Instant>)>,
    /// Override of the wait before `live_open` is retried after a `source_conflict`; `None` keeps
    /// `live::CONFLICT_RETRY_INTERVAL`. Only tests set it.
    live_conflict_retry: Mutex<Option<std::time::Duration>>,
    /// The `open_obsidian_on_start` setting (`core::obsidian_launch`). Read by the client loop
    /// before its first connection attempt; written from settings load and from the Options
    /// panel's checkbox.
    open_obsidian_on_start: AtomicBool,
    /// Which app that launch opens (`core::obsidian_launch::LaunchApp`); same lifecycle as the
    /// flag above.
    launch_app: Mutex<LaunchApp>,
    /// What happened, if anything, when this load tried to open the chosen app automatically. `None`
    /// until the client loop's first connection attempt has run.
    obsidian_launch_outcome: Mutex<Option<ObsidianLaunchOutcome>>,
    /// Bumped after every change of something the Labyrinth panel is painted from: connection
    /// status, the `farm1` and `price2` feeds, the inventory status, the game context and the
    /// reader's diagnostics. The panel keeps what it computed while this stands still
    /// (`panel::PanelCache`), so a setter of any of those that did not bump it would leave the
    /// panel a quarter of a second behind, and one added later has to bump it too.
    panel_generation: AtomicU64,
    /// How many inventory epochs the source has opened since the addon loaded, over all its
    /// connections. For the reader diagnostics of the Options window; never sent.
    live_epochs: AtomicU64,
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
            farming: Mutex::new(FarmingFeed::default()),
            price: Mutex::new(PriceFeed::default()),
            live_status: Mutex::new(crate::live::LiveStatus::NotNegotiated),
            live_context: Mutex::new(None),
            inventory_diagnostics: Mutex::new((crate::inventory::Diagnostics::default(), None)),
            live_conflict_retry: Mutex::new(None),
            open_obsidian_on_start: AtomicBool::new(true),
            launch_app: Mutex::new(LaunchApp::default()),
            obsidian_launch_outcome: Mutex::new(None),
            panel_generation: AtomicU64::new(0),
            live_epochs: AtomicU64::new(0),
        }
    }

    /// Epochs the inventory source has opened since the addon loaded (`live::Channel::
    /// epochs_opened`, added up over its connections).
    pub fn live_epochs_opened(&self) -> u64 {
        self.live_epochs.load(Ordering::Relaxed)
    }

    /// Adds the epochs a capture opened: none, or one.
    pub fn count_live_epochs(&self, opened: u64) {
        self.live_epochs.fetch_add(opened, Ordering::Relaxed);
    }

    /// The number of the state the panel is painted from: it changes whenever any of it does.
    /// Read it BEFORE reading that state: a change that lands in between is then seen as a new
    /// number on the next look, never as an old state under the new number.
    pub fn panel_generation(&self) -> u64 {
        self.panel_generation.load(Ordering::Acquire)
    }

    /// Called by every setter of that state AFTER it has stored the new value, for the same
    /// reason: whoever sees the new number then sees the new value.
    fn touch_panel(&self) {
        self.panel_generation.fetch_add(1, Ordering::Release);
    }

    /// Latest reported character/map for local QA; never a memory address.
    pub fn live_context(&self) -> Option<crate::protocol::GameContext> { lock(&self.live_context).clone() }
    /// The client reports the context four times a second; only a different one is a change.
    pub fn set_live_context(&self, value: crate::protocol::GameContext) {
        let mut context = lock(&self.live_context);
        if context.as_ref() != Some(&value) {
            *context = Some(value);
            drop(context);
            self.touch_panel();
        }
    }
    /// Measurement state remains separate from TCP/game presence.
    pub fn live_status(&self) -> crate::live::LiveStatus { *lock(&self.live_status) }
    /// Set on every pass of the client loop as well; only a different status is a change.
    pub fn set_live_status(&self, value: crate::live::LiveStatus) {
        let changed = std::mem::replace(&mut *lock(&self.live_status), value) != value;
        if changed {
            self.touch_panel();
        }
    }
    /// Wait override for the `source_conflict` retry, read when a connection starts.
    pub fn live_conflict_retry(&self) -> Option<std::time::Duration> { *lock(&self.live_conflict_retry) }
    pub fn set_live_conflict_retry(&self, value: Option<std::time::Duration>) { *lock(&self.live_conflict_retry) = value; }
    /// Counters from the last bounded capture, without raw pointers or inventory rows.
    pub fn inventory_diagnostics(&self) -> crate::inventory::Diagnostics { lock(&self.inventory_diagnostics).0 }
    /// The same diagnostics with the instant of the capture they describe; `None` before the
    /// first one. Read together, so the pair is always of one capture.
    pub fn inventory_reading(&self) -> (crate::inventory::Diagnostics, Option<Instant>) { *lock(&self.inventory_diagnostics) }
    /// `at` is when the capture ran, on the caller's monotonic clock.
    pub fn set_inventory_diagnostics(&self, value: crate::inventory::Diagnostics, at: Instant) {
        *lock(&self.inventory_diagnostics) = (value, Some(at));
        self.touch_panel();
    }

    pub fn port(&self) -> u16 {
        self.port.load(Ordering::Relaxed)
    }

    /// A copy of the saved token. The caller must not log it.
    pub fn token(&self) -> String {
        lock(&self.token).clone()
    }

    /// Replaces the port, the token, the `open_obsidian_on_start` setting and the app it opens
    /// together and marks the settings as changed, which lifts a stop caused by `auth_rejected`
    /// or `version_unsupported`.
    pub fn apply_settings(&self, port: u16, token: &str, open_obsidian_on_start: bool, launch_app: LaunchApp) {
        self.port.store(port, Ordering::Relaxed);
        *lock(&self.token) = token.to_string();
        self.open_obsidian_on_start.store(open_obsidian_on_start, Ordering::Relaxed);
        *lock(&self.launch_app) = launch_app;
        self.settings_generation.fetch_add(1, Ordering::Relaxed);
    }

    pub fn open_obsidian_on_start(&self) -> bool {
        self.open_obsidian_on_start.load(Ordering::Relaxed)
    }

    /// Sets the `open_obsidian_on_start` setting on its own, without bumping the settings
    /// generation: unlike the port and the token, toggling this has nothing to do with the
    /// `auth_rejected`/`version_unsupported` halt the generation exists for, and bumping it
    /// there would wake up a client the plugin has explicitly told to stop retrying.
    pub fn set_open_obsidian_on_start(&self, value: bool) {
        self.open_obsidian_on_start.store(value, Ordering::Relaxed);
    }

    /// The app the automatic launch opens.
    pub fn launch_app(&self) -> LaunchApp {
        *lock(&self.launch_app)
    }

    /// Sets the app on its own, without bumping the settings generation, for the same reason as
    /// [`set_open_obsidian_on_start`](Self::set_open_obsidian_on_start).
    pub fn set_launch_app(&self, app: LaunchApp) {
        *lock(&self.launch_app) = app;
    }

    /// What the addon's attempt to open the chosen app this load did, if it has run yet.
    pub fn obsidian_launch_outcome(&self) -> Option<ObsidianLaunchOutcome> {
        *lock(&self.obsidian_launch_outcome)
    }

    pub fn set_obsidian_launch_outcome(&self, outcome: ObsidianLaunchOutcome) {
        *lock(&self.obsidian_launch_outcome) = Some(outcome);
    }

    pub fn settings_generation(&self) -> u64 {
        self.settings_generation.load(Ordering::Relaxed)
    }

    pub fn status(&self) -> Status {
        Status::from_u8(self.status.load(Ordering::Relaxed))
    }

    pub fn set_status(&self, status: Status) {
        if self.status.swap(status as u8, Ordering::Relaxed) != status as u8 {
            self.touch_panel();
        }
    }

    pub fn connected(&self) -> bool {
        self.status() == Status::Connected
    }

    /// Starts a capability handshake for this connection; previous readings remain stale.
    pub fn begin_farming_connection(&self, nonce: &str) {
        lock(&self.farming).begin(nonce);
        self.touch_panel();
    }

    pub fn disconnect_farming(&self) {
        lock(&self.farming).disconnect();
        self.touch_panel();
    }

    pub fn enable_farming(&self, nonce: &str) -> bool {
        let enabled = lock(&self.farming).enable(nonce);
        self.touch_panel();
        enabled
    }

    /// Accepts only a subscribed connection's increasing farming sequence. This deliberately
    /// never touches alert deduplication, receipts or alert history.
    pub fn accept_farming(&self, reading: FarmingState, now: Instant) -> bool {
        let accepted = lock(&self.farming).accept(reading, now);
        self.touch_panel();
        accepted
    }

    pub fn farming_view(&self, now: Instant) -> FarmingView {
        lock(&self.farming).view(now)
    }

    /// Starts a `price2` handshake for this connection.
    pub fn begin_price_connection(&self, nonce: &str) {
        lock(&self.price).begin(nonce);
        self.touch_panel();
    }

    /// Drops capability and figures at once.
    pub fn disconnect_price(&self) {
        lock(&self.price).disconnect();
        self.touch_panel();
    }

    pub fn enable_price(&self, nonce: &str) -> bool {
        let enabled = lock(&self.price).enable(nonce);
        self.touch_panel();
        enabled
    }

    /// Independent of alert deduplication and of the farming sequence.
    pub fn accept_price(&self, reading: PriceState, now: Instant) -> bool {
        let accepted = lock(&self.price).accept(reading, now);
        self.touch_panel();
        accepted
    }

    pub fn price_view(&self, now: Instant) -> PriceView {
        lock(&self.price).view(now)
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
        state.apply_settings(50000, "t", false, LaunchApp::Hebra);
        assert_eq!(state.port(), 50000);
        assert_eq!(state.token(), "t");
        assert!(!state.open_obsidian_on_start());
        assert_eq!(state.launch_app(), LaunchApp::Hebra);
        assert_eq!(state.settings_generation(), before + 1);
    }

    #[test]
    fn open_obsidian_on_start_defaults_to_true() {
        assert!(SharedState::new().open_obsidian_on_start());
    }

    #[test]
    fn toggling_open_obsidian_on_start_does_not_bump_the_generation() {
        let state = SharedState::new();
        let before = state.settings_generation();
        state.set_open_obsidian_on_start(false);
        assert!(!state.open_obsidian_on_start());
        assert_eq!(state.settings_generation(), before);
    }

    #[test]
    fn the_launch_app_defaults_to_obsidian() {
        assert_eq!(SharedState::new().launch_app(), LaunchApp::Obsidian);
    }

    #[test]
    fn choosing_the_launch_app_does_not_bump_the_generation() {
        let state = SharedState::new();
        let before = state.settings_generation();
        state.set_launch_app(LaunchApp::Hebra);
        assert_eq!(state.launch_app(), LaunchApp::Hebra);
        assert_eq!(state.settings_generation(), before);
    }

    #[test]
    fn the_obsidian_launch_outcome_starts_unset_and_records_what_is_set() {
        let state = SharedState::new();
        assert_eq!(state.obsidian_launch_outcome(), None);
        state.set_obsidian_launch_outcome(ObsidianLaunchOutcome::NoHandler);
        assert_eq!(state.obsidian_launch_outcome(), Some(ObsidianLaunchOutcome::NoHandler));
    }

    /// The panel keeps what it computed while this number stands still, so every setter of
    /// something it paints has to move it, and setting what was already there must not.
    #[test]
    fn every_change_of_what_the_panel_paints_moves_its_generation_and_a_repeat_does_not() {
        use crate::live::LiveStatus;
        use crate::protocol::{parse_server_line, GameContext, GameState, ServerLine};
        const NONCE: &str = "Zk3m1Qw9Lr0aT7yUc2Vb5g";
        let state = SharedState::new();
        let now = Instant::now();
        let farming = || {
            let line = format!(
                r#"{{"v":3,"type":"farming_state","tag":"farm1","nonce":"{NONCE}","seq":1,"ttl":15,"phase":"active","err":null,"elapsed":1,"observed":1,"net":null,"lo":null,"hi":null,"age":0,"slots":null,"slotSrc":"unknown","slotAge":null,"goal":"none","target":null,"progress":null,"eta":null,"mf":null,"mfKind":"unknown","prep":"unknown"}}"#
            );
            let ServerLine::FarmingState(reading) = parse_server_line(&line) else { panic!("{line}") };
            reading
        };
        let price = || {
            let line = format!(
                r#"{{"v":3,"type":"price_state","tag":"price2","nonce":"{NONCE}","seq":1,"ttl":15,"st":"pending","sell":null,"sellStack":null,"list":null,"listStack":null,"age":null}}"#
            );
            let ServerLine::PriceState(reading) = parse_server_line(&line) else { panic!("{line}") };
            reading
        };
        let in_labyrinth = || GameContext { state: GameState::Gameplay, map_id: Some(866), character: Some("Astra Uno".into()) };
        let changes: [(&str, &dyn Fn()); 14] = [
            ("the connection status", &|| state.set_status(Status::Connected)),
            ("a farm1 connection", &|| state.begin_farming_connection(NONCE)),
            ("the farm1 capability", &|| assert!(state.enable_farming(NONCE))),
            ("a farm1 frame", &|| assert!(state.accept_farming(farming(), now))),
            ("a price connection", &|| state.begin_price_connection(NONCE)),
            ("the price capability", &|| assert!(state.enable_price(NONCE))),
            ("a price frame", &|| assert!(state.accept_price(price(), now))),
            ("the inventory status", &|| state.set_live_status(LiveStatus::Measuring)),
            ("the game context", &|| state.set_live_context(in_labyrinth())),
            ("another game context", &|| state.set_live_context(GameContext::character_select())),
            ("the reader's diagnostics", &|| state.set_inventory_diagnostics(crate::inventory::Diagnostics::default(), now)),
            // The same diagnostics of a later cycle are another reading: its instant is new.
            ("the next cycle's diagnostics", &|| state.set_inventory_diagnostics(crate::inventory::Diagnostics::default(), now)),
            ("the farm1 disconnection", &|| state.disconnect_farming()),
            ("the price disconnection", &|| state.disconnect_price()),
        ];
        for (what, change) in changes {
            let before = state.panel_generation();
            change();
            assert!(state.panel_generation() > before, "{what} did not move the generation");
        }
        // What the client loop sets again on every pass, with nothing new.
        let before = state.panel_generation();
        state.set_status(Status::Connected);
        state.set_live_status(LiveStatus::Measuring);
        state.set_live_context(GameContext::character_select());
        assert_eq!(state.panel_generation(), before);
        // And what the panel is not painted from leaves it alone.
        state.apply_settings(50000, "t", false, LaunchApp::Hebra);
        state.push_history(&alert(Some(1)));
        state.set_obsidian_launch_outcome(ObsidianLaunchOutcome::NoHandler);
        assert_eq!(state.panel_generation(), before);
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

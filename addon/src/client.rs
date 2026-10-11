//! The Nexus half of the client: the [`Host`] the core's loop (`tyrian_companion_nexus_core::
//! client`) runs against inside the game. The loop itself, protocol and all, lives in the core
//! crate so `cargo test` can drive it on Linux; what is here is only what needs Nexus:
//!
//! - painting an alert: `nexus::alert::send_alert` (`GUI_SendAlert`), with the plugin's `content`
//!   verbatim;
//! - reading the game: `NexusLink::is_gameplay` and the Mumble Link Nexus shares with addons
//!   (`DL_MUMBLE_LINK`), plus the negotiated passive owned-inventory and wallet reader; no GW2
//!   API;
//! - knowing the game is closing: the `WndProc` callback in `lib.rs` reports `WM_CLOSE` (provisional)
//!   and `WM_DESTROY` (final) to [`EXIT_WATCH`], the only evidence the SPEC accepts for a `bye`
//!   with `game_exit`; the rule that takes a cancelled `WM_CLOSE` back is the core's `ExitWatch`.
//!
//! Nothing here sends an input to the game.

use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use std::time::Instant;

use nexus::data_link::{read_nexus_link, read_resource};
use tyrian_companion_nexus_core::exit_watch::ExitWatch;
use tyrian_companion_nexus_core::client::{GameReading, Host, Readiness};
use tyrian_companion_nexus_core::game_context::{MumbleSnapshot, MUMBLE_LINK_BYTES};
use tyrian_companion_nexus_core::obsidian_launch::{LaunchApp, ObsidianLaunchOutcome};

use crate::obsidian_launch;

/// Nexus's data link for the Mumble Link (`nexus::data_link::mumble::MUMBLE_LINK`,
/// which is behind the `mumble` feature this addon does not enable; see `game_context.rs`).
const MUMBLE_LINK: &str = "DL_MUMBLE_LINK";

/// What the `WndProc` callback saw of the game window closing; see `ExitWatch` for the rule.
static EXIT_WATCH: Mutex<ExitWatch> = Mutex::new(ExitWatch::new());

fn watch() -> std::sync::MutexGuard<'static, ExitWatch> {
    EXIT_WATCH.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The window received `WM_CLOSE`: provisional evidence, taken back if the game keeps running.
pub fn mark_close_requested() {
    watch().close_requested(Instant::now());
}

/// The window received `WM_DESTROY`: the game is closing, for good.
pub fn mark_destroyed() {
    watch().destroyed();
}

/// The host as Nexus provides it. Stateless: everything it reads lives in Nexus.
pub struct NexusHost;

impl Host for NexusHost {
    fn show_alert(&self, text: &str) {
        nexus::alert::send_alert(text);
    }

    fn read_game(&self) -> GameReading {
        let is_gameplay = read_nexus_link().map(|link| link.is_gameplay);
        // Safety: `DL_MUMBLE_LINK` points at the game's `LinkedMem`, which is exactly
        // `MUMBLE_LINK_BYTES` long; copying it out as a byte array of that size makes no
        // assumption about its contents, which `MumbleSnapshot::from_bytes` then parses
        // defensively. The game may be writing it at the same time; a torn copy is tolerated
        // there.
        let bytes = unsafe { read_resource::<[u8; MUMBLE_LINK_BYTES]>(MUMBLE_LINK) };
        let mumble = bytes.and_then(|bytes| MumbleSnapshot::from_bytes(&bytes));
        GameReading { is_gameplay, mumble }
    }

    fn read_inventory(&self, stop: &AtomicBool) -> Result<tyrian_companion_nexus_core::inventory::InventorySnapshot, tyrian_companion_nexus_core::inventory::ReadError> {
        crate::inventory::sample(stop)
    }
    fn inventory_diagnostics(&self) -> tyrian_companion_nexus_core::inventory::Diagnostics { crate::inventory::diagnostics() }
    fn discard_cycle(&self) { crate::inventory::discard_cycle(); }

    /// The game's executable is verified before the first sample, a slice of its hash on each
    /// pass: not ready while that is under way, with nothing to tell the plugin until it has
    /// taken too long.
    fn prepare_inventory(&self, stop: &AtomicBool, interrupted: &dyn Fn() -> bool) -> Readiness {
        crate::inventory::prepare(stop, interrupted)
    }

    fn game_exiting(&self) -> bool {
        // The Mumble Link is read here only while a `WM_CLOSE` is pending, to see whether the
        // game goes on ticking after it.
        //
        // The lock is not held across that read: the `WndProc` takes it on the game's window
        // thread, which must never wait for the worker to finish copying the link.
        let watching = watch().watching_ticks();
        let tick = if watching { self.read_game().mumble.map(|mumble| mumble.ui_tick) } else { None };
        let mut watch = watch();
        if watching {
            // A `WM_DESTROY` that arrived during the read is not undone: `observe` ignores it.
            watch.observe(Instant::now(), tick);
        }
        watch.is_exiting()
    }

    fn open_app(&self, app: LaunchApp) -> ObsidianLaunchOutcome {
        obsidian_launch::open_app(app)
    }
}

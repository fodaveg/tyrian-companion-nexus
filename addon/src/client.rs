//! The Nexus half of the client: the [`Host`] the core's loop (`tyrian_companion_nexus_core::
//! client`) runs against inside the game. The loop itself, protocol and all, lives in the core
//! crate so `cargo test` can drive it on Linux; what is here is only what needs Nexus:
//!
//! - painting an alert: `nexus::alert::send_alert` (`GUI_SendAlert`), with the plugin's `content`
//!   verbatim;
//! - reading the game: `NexusLink::is_gameplay` and the Mumble Link Nexus shares with addons
//!   (`DL_MUMBLE_LINK`), both through Nexus's data links. Nothing else of the game is read: no
//!   process memory, no GW2 API;
//! - knowing the game is closing: the `WndProc` callback in `lib.rs` sets [`GAME_EXITING`] on
//!   `WM_CLOSE`/`WM_DESTROY`, the only evidence the SPEC accepts for a `bye` with `game_exit`.
//!
//! Nothing here sends an input to the game.

use std::sync::atomic::{AtomicBool, Ordering};

use nexus::data_link::{read_nexus_link, read_resource};
use tyrian_companion_nexus_core::client::{GameReading, Host};
use tyrian_companion_nexus_core::game_context::{MumbleSnapshot, MUMBLE_LINK_BYTES};

/// Nexus's data link for the Mumble Link (`nexus::data_link::mumble::MUMBLE_LINK`,
/// which is behind the `mumble` feature this addon does not enable; see `game_context.rs`).
const MUMBLE_LINK: &str = "DL_MUMBLE_LINK";

/// Set by the `WndProc` callback when the game window is closing. Never cleared: the game does
/// not come back from `WM_DESTROY`.
static GAME_EXITING: AtomicBool = AtomicBool::new(false);

/// Records positive evidence that the game is closing.
pub fn mark_game_exiting() {
    GAME_EXITING.store(true, Ordering::Relaxed);
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

    fn game_exiting(&self) -> bool {
        GAME_EXITING.load(Ordering::Relaxed)
    }
}

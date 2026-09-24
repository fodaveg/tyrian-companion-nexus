//! Tyrian Companion — Nexus addon.
//!
//! Paints, inside Guild Wars 2, the alerts the Tyrian Companion Obsidian plugin emits, and
//! reports the game context (state, map, character) the plugin uses to mark play sessions.
//! `docs/SPEC-puente-ingame.md` in the `tyrian-companion` repo, protocol v2, is the contract this
//! addon implements.
//!
//! What this addon is, structurally, and why: an authenticated client of one loopback TCP port.
//! It sends the four line types the protocol allows (`hello`, `context`, `heartbeat`, `bye`),
//! reads what its host already exposes to every addon (`NexusLink::is_gameplay` and the Mumble
//! Link's map and character), and paints alerts. It never calls the GW2 API, never reads process
//! memory, and never sends an input to the game. That is what keeps it inside the "utility that
//! helps players without affecting others" branch of ArenaNet's third-party policy (see the
//! spec's "Política de ArenaNet"): what the plugin does with the context happens outside the
//! game, in the user's notes.
//!
//! Everything below is `#[cfg(windows)]`: `nexus` (and the `windows` crate underneath it)
//! only compiles when targeting Windows, matching `Cargo.toml`'s own `[target.'cfg(windows)'`
//! `.dependencies]` fence. On any other host this crate still builds — as an empty shell —
//! which is what lets `cargo build`/`cargo test`, run bare from the workspace root on this
//! repository's Linux host, succeed without needing `--target` for every invocation. The
//! testable logic (the wire protocol, the client loop, the game-context reading, the framer,
//! the backoff table, settings persistence) lives in `tyrian_companion_nexus_core`, which has
//! no such restriction; see its own doc.

#[cfg(windows)]
mod client;
#[cfg(windows)]
mod render;

#[cfg(windows)]
use nexus::gui::{register_render, render, RenderType};
#[cfg(windows)]
use nexus::wnd_proc::register_wnd_proc;
#[cfg(windows)]
use nexus::AddonFlags;
#[cfg(windows)]
use tyrian_companion_nexus_core::client::{ClientConfig, ClientHandle};
#[cfg(windows)]
use tyrian_companion_nexus_core::{instance, settings, state};
#[cfg(windows)]
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};

/// Name Nexus's `get_addon_dir` uses for this addon's own settings directory
/// (`<GW2>/addons/tyrian_companion_nexus/`).
#[cfg(windows)]
const ADDON_DIR_NAME: &str = "tyrian_companion_nexus";

/// Local, unpublished addon signature. Not a Raidcore-issued id: this addon is not listed on
/// Raidcore's addon library, so per the crate's own convention (see `nexus_example_addon`)
/// this is a negative number picked to be distinctive, not one assigned by Raidcore.
/// Spells "TYRI" in ASCII (`T`=0x54 `Y`=0x59 `R`=0x52 `I`=0x49), negated.
#[cfg(windows)]
const ADDON_SIGNATURE: i32 = -0x5459_5249;

/// This crate's own version, sent as `clientVersion` in the `hello` line.
#[cfg(windows)]
const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// `WM_DESTROY` and `WM_CLOSE`, spelled out rather than pulled from the `windows` crate's
/// `Win32_UI_WindowsAndMessaging` feature for two constants.
#[cfg(windows)]
const WM_DESTROY: u32 = 0x0002;
#[cfg(windows)]
const WM_CLOSE: u32 = 0x0010;

#[cfg(windows)]
static mut CLIENT_HANDLE: Option<ClientHandle> = None;

#[cfg(windows)]
nexus::export! {
    name: "Tyrian Companion",
    signature: ADDON_SIGNATURE,
    load,
    unload,
    flags: AddonFlags::None,
    provider: nexus::UpdateProvider::None,
    log_filter: "info",
}

#[cfg(windows)]
fn load() {
    log::info!("Tyrian Companion addon v{CLIENT_VERSION} loading");

    let settings = match nexus::paths::get_addon_dir(ADDON_DIR_NAME) {
        Ok(dir) => settings::load(&dir),
        Err(error) => {
            log::warn!("could not resolve the addon's own directory ({error}); using default settings");
            settings::Settings::default()
        }
    };
    state::shared().apply_settings(settings.port, &settings.token);
    render::init_pending(&settings);

    register_render(
        RenderType::OptionsRender,
        render!(|ui| render::options_render(ui)),
    )
    .revert_on_unload();
    register_wnd_proc(game_wnd_proc).revert_on_unload();

    let config = ClientConfig { client_version: CLIENT_VERSION.to_string(), instance: instance::new_instance_id() };
    match tyrian_companion_nexus_core::client::spawn(state::shared(), client::NexusHost, config) {
        // Safety: `load` and `unload` are only ever called by Nexus, serially, on the same
        // thread that owns this addon's lifecycle; nothing else in this crate touches
        // `CLIENT_HANDLE`.
        Ok(handle) => unsafe { CLIENT_HANDLE = Some(handle) },
        Err(error) => log::error!("could not start the connection thread: {error}"),
    }
}

#[cfg(windows)]
fn unload() {
    log::info!("Tyrian Companion addon unloading");
    // Safety: see the comment in `load`. Stopping here blocks until the background thread
    // has sent its `bye` and exited, which is what makes it safe for the DLL to be unmapped
    // right after: no thread is left holding an `AddonApi` pointer from an unloaded module.
    #[allow(static_mut_refs)]
    let handle = unsafe { CLIENT_HANDLE.take() };
    if let Some(handle) = handle {
        handle.stop();
    }
}

/// Watches the game window's messages for the one thing this addon needs from them: that the
/// game is closing, which is what turns the `bye` into `game_exit` instead of `addon_unload`.
///
/// It only observes. The return value follows Nexus's convention, as its official addon
/// template does (`return uMsg;`): zero would mean "handled, do not pass on", so returning the
/// message itself hands every message on to the game untouched.
#[cfg(windows)]
extern "C-unwind" fn game_wnd_proc(_window: HWND, message: u32, _w_param: WPARAM, _l_param: LPARAM) -> u32 {
    if message == WM_CLOSE || message == WM_DESTROY {
        client::mark_game_exiting();
    }
    message
}

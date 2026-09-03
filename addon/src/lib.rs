//! Tyrian Companion — Nexus addon.
//!
//! Paints, inside Guild Wars 2, the alerts the Tyrian Companion Obsidian plugin emits.
//! `docs/SPEC-puente-ingame.md` in the `tyrian-companion` repo is the contract this addon
//! implements; this crate is the M3 milestone it describes.
//!
//! What this addon is, structurally, and why: a read-only client of one loopback TCP port.
//! It sends exactly one line (`hello`) and after that only ever reads. It never calls the
//! GW2 API, never reads Mumble Link or NexusLink, and never sends an input to the game. That
//! is not a style choice: it is what keeps this addon inside the "utility that helps players
//! without affecting others" branch of ArenaNet's third-party policy (see the spec's
//! "Política de ArenaNet"), rather than the branch that gets addons banned.
//!
//! Everything below is `#[cfg(windows)]`: `nexus` (and the `windows` crate underneath it)
//! only compiles when targeting Windows, matching `Cargo.toml`'s own `[target.'cfg(windows)'`
//! `.dependencies]` fence. On any other host this crate still builds — as an empty shell —
//! which is what lets `cargo build`/`cargo test`, run bare from the workspace root on this
//! repository's Linux host, succeed without needing `--target` for every invocation. The
//! testable logic (the wire protocol, the framer, the backoff table, settings persistence)
//! lives in `tyrian_companion_nexus_core`, which has no such restriction; see its own doc.

#[cfg(windows)]
mod client;
#[cfg(windows)]
mod render;

#[cfg(windows)]
use nexus::gui::{register_render, render, RenderType};
#[cfg(windows)]
use nexus::AddonFlags;
#[cfg(windows)]
use tyrian_companion_nexus_core::{settings, state};

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

#[cfg(windows)]
static mut CLIENT_HANDLE: Option<client::ClientHandle> = None;

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
    state::shared().set_port(settings.port);
    render::init_pending_port(settings.port);

    register_render(
        RenderType::OptionsRender,
        render!(|ui| render::options_render(ui)),
    )
    .revert_on_unload();

    // Safety: `load` and `unload` are only ever called by Nexus, serially, on the same
    // thread that owns this addon's lifecycle; nothing else in this crate touches
    // `CLIENT_HANDLE`.
    #[allow(static_mut_refs)]
    unsafe {
        CLIENT_HANDLE = Some(client::spawn(CLIENT_VERSION.to_string()));
    }
}

#[cfg(windows)]
fn unload() {
    log::info!("Tyrian Companion addon unloading");
    // Safety: see the comment in `load`. Stopping here blocks until the background thread
    // has actually exited, which is what makes it safe for the DLL to be unmapped right
    // after: no thread is left holding an `AddonApi` pointer from an unloaded module.
    #[allow(static_mut_refs)]
    let handle = unsafe { CLIENT_HANDLE.take() };
    if let Some(handle) = handle {
        handle.stop();
    }
}

//! The optional ImGui surface: a small section appended to Nexus's own Options window
//! (`RenderType::OptionsRender`), showing connection status, the configured port, and the
//! last few alerts. Entirely optional per the spec: [`crate::client`] calls
//! `nexus::alert::send_alert` on its own, so every alert this addon is asked to show still
//! shows even if this module were removed.
//!
//! `nexus`'s `render!` macro requires a plain `fn(&Ui)` (see `state.rs`'s module doc), so
//! this reads and writes only through `state::shared()` and the module-level `PENDING_PORT`
//! below, never through a captured closure environment.

use std::sync::{Mutex, OnceLock};

use nexus::imgui::{TreeNodeFlags, Ui};

use tyrian_companion_nexus_core::settings::{self, Settings};
use tyrian_companion_nexus_core::state;

use crate::ADDON_DIR_NAME;

/// The port value the input box is currently showing, which may not yet be saved. Seeded
/// from the loaded settings once, at `load()`; see `init_pending_port`.
static PENDING_PORT: OnceLock<Mutex<i32>> = OnceLock::new();

pub fn init_pending_port(port: u16) {
    let _ = PENDING_PORT.set(Mutex::new(i32::from(port)));
}

fn pending_port() -> &'static Mutex<i32> {
    PENDING_PORT.get_or_init(|| Mutex::new(i32::from(tyrian_companion_nexus_core::protocol::DEFAULT_PORT)))
}

pub fn options_render(ui: &Ui) {
    let shared = state::shared();

    ui.text("Tyrian Companion");
    ui.separator();

    if shared.connected() {
        ui.text_colored([0.45, 0.85, 0.45, 1.0], "Status: connected");
    } else {
        ui.text_colored([0.90, 0.65, 0.30, 1.0], "Status: waiting for the plugin...");
    }

    {
        let mut pending = pending_port().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        ui.input_int("Port", &mut pending).build();
        if ui.button("Save") {
            let clamped = (*pending).clamp(1, i32::from(u16::MAX));
            *pending = clamped;
            let port = clamped as u16;
            shared.set_port(port);
            if let Ok(dir) = nexus::paths::get_addon_dir(ADDON_DIR_NAME) {
                if let Err(error) = settings::save(&dir, Settings { port }) {
                    log::error!("failed to save settings: {error}");
                }
            }
        }
    }
    ui.text_wrapped("The port must match the one in the plugin's own settings. A change here applies on the next (re)connection attempt.");

    ui.separator();
    if ui.collapsing_header("Recent alerts", TreeNodeFlags::empty()) {
        let history = shared.history_snapshot();
        if history.is_empty() {
            ui.text_disabled("(none yet)");
        }
        for entry in history {
            ui.text_colored(entry.kind.color(), format!("[{}] {}", entry.kind.label(), entry.content));
        }
    }
}

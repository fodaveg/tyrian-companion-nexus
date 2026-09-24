//! The ImGui surface: a small section appended to Nexus's own Options window
//! (`RenderType::OptionsRender`), showing connection status, the port and token settings, and
//! the last few alerts. The alerts themselves do not depend on it: the client calls
//! `nexus::alert::send_alert` on its own. The token field does: it is where the user pastes the
//! secret the plugin's "Copy token" button puts on the clipboard.
//!
//! `nexus`'s `render!` macro requires a plain `fn(&Ui)` (see `state.rs`'s module doc), so
//! this reads and writes only through `state::shared()` and the module-level `PENDING` below,
//! never through a captured closure environment.
//!
//! The token is never drawn: the field is a password field, and nothing here logs it.

use std::sync::{Mutex, OnceLock};

use nexus::imgui::{TreeNodeFlags, Ui};

use tyrian_companion_nexus_core::protocol::{is_usable_token, DEFAULT_PORT};
use tyrian_companion_nexus_core::settings::{self, Settings};
use tyrian_companion_nexus_core::state::{self, Status};

use crate::ADDON_DIR_NAME;

/// What the input boxes currently show, which may not be saved yet.
struct Pending {
    port: i32,
    token: String,
}

/// Seeded from the loaded settings once, at `load()`; see `init_pending`.
static PENDING: OnceLock<Mutex<Pending>> = OnceLock::new();

pub fn init_pending(settings: &Settings) {
    let _ = PENDING.set(Mutex::new(Pending { port: i32::from(settings.port), token: settings.token.clone() }));
}

fn pending() -> &'static Mutex<Pending> {
    PENDING.get_or_init(|| Mutex::new(Pending { port: i32::from(DEFAULT_PORT), token: String::new() }))
}

const GREEN: [f32; 4] = [0.45, 0.85, 0.45, 1.0];
const ORANGE: [f32; 4] = [0.90, 0.65, 0.30, 1.0];
const RED: [f32; 4] = [0.95, 0.35, 0.35, 1.0];
const GREY: [f32; 4] = [0.70, 0.70, 0.70, 1.0];

fn status_line(status: Status) -> ([f32; 4], &'static str) {
    match status {
        Status::Connected => (GREEN, "Status: connected"),
        Status::WaitingForPlugin => (ORANGE, "Status: waiting for the plugin..."),
        Status::MissingToken => (ORANGE, "Status: no token yet. Paste it below"),
        Status::TokenRejected => (RED, "Status: the plugin rejected the token. Copy it again from Obsidian"),
        Status::UpdateRequired => (RED, "Status: the plugin needs a newer version of this addon"),
        Status::GameExiting => (GREY, "Status: the game is closing"),
    }
}

pub fn options_render(ui: &Ui) {
    let shared = state::shared();

    ui.text("Tyrian Companion");
    ui.separator();

    let (color, text) = status_line(shared.status());
    ui.text_colored(color, text);

    {
        let mut pending = pending().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        ui.input_int("Port", &mut pending.port).build();
        ui.input_text("Token", &mut pending.token).password(true).build();
        ui.same_line();
        if ui.button("Paste") {
            if let Some(text) = ui.clipboard_text() {
                pending.token = text.trim().to_string();
            }
        }
        if ui.button("Save") {
            let port = pending.port.clamp(1, i32::from(u16::MAX)) as u16;
            pending.port = i32::from(port);
            // A copy from Obsidian can carry a trailing newline; the plugin compares exactly.
            let token = pending.token.trim().to_string();
            pending.token = token.clone();
            shared.apply_settings(port, &token);
            if let Ok(dir) = nexus::paths::get_addon_dir(ADDON_DIR_NAME) {
                if let Err(error) = settings::save(&dir, &Settings { port, token }) {
                    log::error!("failed to save settings: {error}");
                }
            }
        }
        if !pending.token.trim().is_empty() && !is_usable_token(pending.token.trim()) {
            ui.text_colored(ORANGE, "That does not look like a token: it has 32 to 128 characters and no spaces.");
        }
    }
    ui.text_wrapped(
        "In Obsidian, open Tyrian Companion's settings and press \"Copy token\", then paste it here \
         and save. The port must match the plugin's. Changes apply on the next connection attempt.",
    );

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

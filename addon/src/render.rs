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
//! The token is never drawn: the field is a password field, and nothing here logs it. What the
//! Save button may store is decided by `tyrian_companion_nexus_core::token::validate_for_save`: a
//! Guild Wars 2 API key or a value outside the plugin's format is refused with a message and never
//! reaches `settings.json` or the client.

use std::sync::{Mutex, OnceLock};

use nexus::imgui::{StyleColor, TreeNodeFlags, Ui};

use tyrian_companion_nexus_core::protocol::{DEFAULT_PORT, TOKEN_REJECTED_STATUS};
use tyrian_companion_nexus_core::settings::{self, Settings};
use tyrian_companion_nexus_core::state::{self, Status};
use tyrian_companion_nexus_core::token::{self, TokenRejection};

use crate::ADDON_DIR_NAME;

/// What the input boxes currently show, which may not be saved yet.
struct Pending {
    port: i32,
    token: String,
    /// Why the last paste or save was refused, until the next one that goes through.
    notice: Option<&'static str>,
}

/// Seeded from the loaded settings once, at `load()`; see `init_pending`.
static PENDING: OnceLock<Mutex<Pending>> = OnceLock::new();

/// `notice` is shown under the fields from the first frame: `load()` passes one when it removed
/// an API key from `settings.json`.
pub fn init_pending(settings: &Settings, notice: Option<&'static str>) {
    let _ = PENDING.set(Mutex::new(Pending { port: i32::from(settings.port), token: settings.token.clone(), notice }));
}

fn pending() -> &'static Mutex<Pending> {
    PENDING.get_or_init(|| Mutex::new(Pending { port: i32::from(DEFAULT_PORT), token: String::new(), notice: None }))
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
        Status::TokenRejected => (RED, TOKEN_REJECTED_STATUS),
        Status::UpdateRequired => (RED, "Status: the plugin needs a newer version of this addon"),
        Status::GameExiting => (GREY, "Status: the game is closing"),
    }
}

/// `text_colored` does not wrap, and the token messages run to a sentence or two.
fn text_colored_wrapped(ui: &Ui, color: [f32; 4], text: &str) {
    let color = ui.push_style_color(StyleColor::Text, color);
    ui.text_wrapped(text);
    color.pop();
}

pub fn options_render(ui: &Ui) {
    let shared = state::shared();

    ui.text("Tyrian Companion");
    ui.separator();

    let (color, text) = status_line(shared.status());
    text_colored_wrapped(ui, color, text);

    {
        let mut pending = pending().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        ui.input_int("Port", &mut pending.port).build();
        ui.input_text("Token", &mut pending.token).password(true).build();
        ui.same_line();
        if ui.button("Paste") {
            if let Some(text) = ui.clipboard_text() {
                if token::is_gw2_api_key(text.trim()) {
                    // Not even kept in the field: an API key has no reason to sit in this addon.
                    pending.token.clear();
                    pending.notice = Some(TokenRejection::Gw2ApiKey.message());
                } else {
                    pending.token = text.trim().to_string();
                    pending.notice = None;
                }
            }
        }
        if ui.button("Save") {
            let port = pending.port.clamp(1, i32::from(u16::MAX)) as u16;
            pending.port = i32::from(port);
            // Trimmed there: a copy from Obsidian can carry a trailing newline, and the plugin
            // compares exactly. A refused value saves nothing, port included.
            match token::validate_for_save(&pending.token) {
                Ok(token) => {
                    pending.token = token.clone();
                    pending.notice = None;
                    shared.apply_settings(port, &token);
                    if let Ok(dir) = nexus::paths::get_addon_dir(ADDON_DIR_NAME) {
                        if let Err(error) = settings::save(&dir, &Settings { port, token }) {
                            log::error!("failed to save settings: {error}");
                        }
                    }
                }
                Err(rejection) => {
                    if rejection == TokenRejection::Gw2ApiKey {
                        pending.token.clear();
                    }
                    pending.notice = Some(rejection.message());
                }
            }
        }
        // The last refusal, or what Save would say about what is typed now.
        let hint = pending.notice.or_else(|| token::validate_for_save(&pending.token).err().map(TokenRejection::message));
        if let Some(hint) = hint {
            text_colored_wrapped(ui, ORANGE, hint);
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

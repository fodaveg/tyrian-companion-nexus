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

use nexus::imgui::{Condition, ProgressBar, StyleColor, StyleVar, TreeNodeFlags, Ui, Window};

use tyrian_companion_nexus_core::farming::{FarmingError, Goal, MagicFindKind, Phase, Preparation, SlotSource};

use tyrian_companion_nexus_core::obsidian_launch::{LaunchApp, ObsidianLaunchOutcome};
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
    show_farming_panel: bool,
    farming_english: bool,
    reset_farming_position: bool,
}

/// Seeded from the loaded settings once, at `load()`; see `init_pending`.
static PENDING: OnceLock<Mutex<Pending>> = OnceLock::new();

/// `notice` is shown under the fields from the first frame: `load()` passes one when it removed
/// an API key from `settings.json`.
pub fn init_pending(settings: &Settings, notice: Option<&'static str>) {
    let _ = PENDING.set(Mutex::new(Pending { port: i32::from(settings.port), token: settings.token.clone(), notice,
        show_farming_panel: settings.show_farming_panel, farming_english: settings.farming_english,
        reset_farming_position: false }));
}

fn pending() -> &'static Mutex<Pending> {
    PENDING.get_or_init(|| Mutex::new(Pending { port: i32::from(DEFAULT_PORT), token: String::new(), notice: None,
        show_farming_panel: false, farming_english: false, reset_farming_position: false }))
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

/// What the panel shows about this load's one attempt to open the chosen app, Obsidian or Hebra,
/// automatically (see `tyrian_companion_nexus_core::obsidian_launch`). Native alerts are
/// reserved for the plugin's own content; this is housekeeping the player did not ask to be
/// interrupted for.
fn launch_line(app: LaunchApp, outcome: ObsidianLaunchOutcome) -> ([f32; 4], String) {
    let name = app.name();
    match outcome {
        ObsidianLaunchOutcome::Launched => {
            (GREEN, format!("{name}: launched automatically (the first connection attempt found nobody listening)"))
        }
        ObsidianLaunchOutcome::NoHandler => {
            (ORANGE, format!("{name}: not launched — no {} handler is registered in Windows", app.uri().trim_end_matches("open")))
        }
        ObsidianLaunchOutcome::Error(code) => (RED, format!("{name}: could not launch it (error code {code})")),
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
    ui.text_wrapped(inventory_status(shared.live_status(), true));
    ui.text_wrapped(wallet_status(shared.live_status(), shared.inventory_diagnostics().wallet, true));
    ui.text_wrapped("Verified Magic Find: no coverage");
    if ui.collapsing_header("Reader diagnostics", TreeNodeFlags::empty()) {
        ui.text_wrapped(format!("Profile: {}", tyrian_companion_nexus_core::inventory::PROFILE));
        ui.text_wrapped(format!("Supported build SHA-256: {}", tyrian_companion_nexus_core::inventory::BUILD_SHA256));
        let diagnostics = shared.inventory_diagnostics();
        ui.text_wrapped(if diagnostics.owner_verified { "Inventory owner: verified" } else { "Inventory owner: not verified" });
        ui.text_wrapped(format!("Own threads: {} / 128; positions: {} / 640", diagnostics.threads, diagnostics.positions));
        ui.text_wrapped(format!("Requested bytes: {} / 131072; reads: {} / 32768", diagnostics.bytes, diagnostics.reads));
        ui.text_wrapped(format!("Wallet requested bytes: {} / {}; reads: {}", diagnostics.wallet_bytes,
            tyrian_companion_nexus_core::wallet::MAX_BYTES, diagnostics.wallet_reads));
        if let Some(context) = shared.live_context() {
            ui.text_wrapped(format!("Character: {}; map: {}", context.character.as_deref().unwrap_or("unknown"), context.map_id.map_or_else(|| "unknown".into(), |id|id.to_string())));
        }
    }

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
                    let open_obsidian_on_start = shared.open_obsidian_on_start();
                    let launch_app = shared.launch_app();
                    shared.apply_settings(port, &token, open_obsidian_on_start, launch_app);
                    if let Ok(dir) = nexus::paths::get_addon_dir(ADDON_DIR_NAME) {
                        if let Err(error) =
                            settings::save(&dir, &Settings { port, token, open_obsidian_on_start, launch_app,
                                show_farming_panel: pending.show_farming_panel, farming_english: pending.farming_english })
                        {
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
    {
        // Applied right away, unlike port/token: a checkbox or a choice between two apps has
        // nothing to validate, so there is no reason to make it wait for "Save". Both are read
        // from and written straight to shared state, never through `pending`, so a token edited
        // but not yet saved is never read back out when these are the only things that changed.
        let mut open_on_start = shared.open_obsidian_on_start();
        let mut launch_app = shared.launch_app();
        let mut changed = false;
        if ui.checkbox("Open Obsidian or Hebra automatically when the game starts", &mut open_on_start) {
            shared.set_open_obsidian_on_start(open_on_start);
            changed = true;
        }
        ui.text("App to open:");
        for app in LaunchApp::ALL {
            ui.same_line();
            if ui.radio_button(app.name(), &mut launch_app, app) {
                shared.set_launch_app(launch_app);
                changed = true;
            }
        }
        if changed {
            let panel = pending().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Ok(dir) = nexus::paths::get_addon_dir(ADDON_DIR_NAME) {
                let settings = Settings {
                    port: shared.port(),
                    token: shared.token(),
                    open_obsidian_on_start: open_on_start,
                    launch_app,
                    show_farming_panel: panel.show_farming_panel,
                    farming_english: panel.farming_english,
                };
                if let Err(error) = settings::save(&dir, &settings) {
                    log::error!("failed to save settings: {error}");
                }
            }
        }
        if let Some(outcome) = shared.obsidian_launch_outcome() {
            let (color, text) = launch_line(launch_app, outcome);
            text_colored_wrapped(ui, color, &text);
        }
    }

    ui.separator();
    {
        let mut panel = pending().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut changed = ui.checkbox("Show Labyrinth farming panel / Mostrar panel de Laberinto", &mut panel.show_farming_panel);
        changed |= ui.checkbox("Farming panel in English / Panel en inglés", &mut panel.farming_english);
        if ui.button("Reset farming panel position / Restablecer posición") {
            panel.reset_farming_position = true;
        }
        if changed { save_panel_settings(&panel); }
        ui.text_wrapped("Read-only: session, goal and preparation are managed in Hebra or Obsidian.");
    }

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

/// Saves only applied settings, so toggling the panel cannot save an unfinished token paste.
fn save_panel_settings(panel: &Pending) {
    let shared = state::shared();
    if let Ok(dir) = nexus::paths::get_addon_dir(ADDON_DIR_NAME) {
        let settings = Settings {
            port: shared.port(), token: shared.token(),
            open_obsidian_on_start: shared.open_obsidian_on_start(), launch_app: shared.launch_app(),
            show_farming_panel: panel.show_farming_panel, farming_english: panel.farming_english,
        };
        if let Err(error) = settings::save(&dir, &settings) { log::error!("failed to save settings: {error}"); }
    }
}

fn translated<'a>(english: bool, spanish: &'a str, en: &'a str) -> &'a str {
    if english { en } else { spanish }
}

fn number(value: Option<i32>) -> String {
    value.map_or_else(|| "—".into(), |value| value.to_string())
}

fn duration(seconds: i32) -> String {
    let seconds = seconds as u32;
    format!("{}:{:02}:{:02}", seconds / 3600, seconds / 60 % 60, seconds % 60)
}

fn phase_label(phase: Phase, english: bool) -> &'static str {
    let (es, en) = match phase {
        Phase::Idle => ("Sin medición", "Not measuring"),
        Phase::Starting => ("Preparando medición", "Preparing measurement"),
        Phase::Active => ("Midiendo", "Measuring"),
        Phase::Stopping => ("Terminando sesión", "Finishing session"),
        Phase::Provisional => ("Cierre provisional", "Provisional close"),
        Phase::Complete => ("Sesión finalizada", "Session complete"),
        Phase::Abandoned => ("Sesión abandonada", "Session abandoned"),
        Phase::Error => ("Error de sesión", "Session error"),
    };
    translated(english, es, en)
}

/// Failures are independent of phase: an active session may fail an observation and a
/// completed session may fail to save. Both stay visible until the host clears them.
fn farming_error_label(error: FarmingError, english: bool) -> &'static str {
    let (es, en) = match error {
        FarmingError::Start => ("No se pudo empezar", "Could not start"),
        FarmingError::Observe => ("No se pudo actualizar", "Could not update"),
        FarmingError::Stop => ("No se pudo finalizar", "Could not finish"),
        FarmingError::Save => ("No se pudo guardar", "Could not save"),
        FarmingError::Other => ("Error de sesión", "Session error"),
    };
    translated(english, es, en)
}

/// Source status is separate from both host connectivity and persisted session phase.
fn inventory_status(status: tyrian_companion_nexus_core::live::LiveStatus, english: bool) -> &'static str {
    use tyrian_companion_nexus_core::live::LiveStatus;
    let (es, en) = match status {
        LiveStatus::NotNegotiated => ("Inventario: fuente no disponible en este host", "Inventory: source unavailable in this host"),
        LiveStatus::Waiting => ("Inventario: esperando confirmación", "Inventory: waiting for confirmation"),
        LiveStatus::Measuring => ("Inventario: observaciones guardadas", "Inventory: observations stored"),
        LiveStatus::Partial => ("Inventario: cantidades sin resolver", "Inventory: unresolved quantities"),
        LiveStatus::UnsupportedBuild => ("Inventario: versión del juego no compatible", "Inventory: unsupported game build"),
        LiveStatus::Unavailable => ("Inventario: lectura no disponible", "Inventory: reading unavailable"),
        LiveStatus::Conflict => ("Inventario: otra fuente vinculada a la sesión", "Inventory: another source owns the session"),
        LiveStatus::StorageUnavailable => ("Inventario: no se pudo guardar la lectura", "Inventory: observation could not be stored"),
    };
    translated(english, es, en)
}

/// Wallet coverage of the last capture: how many currencies it listed, or the closed reason it
/// listed none. Covered means those IDs only; an unlisted currency is unknown, never zero. With
/// no measurement in progress there is no capture to describe, whatever an older one found.
fn wallet_status(status: tyrian_companion_nexus_core::live::LiveStatus, coverage: tyrian_companion_nexus_core::wallet::WalletCoverage, english: bool) -> String {
    use tyrian_companion_nexus_core::wallet::{WalletCoverage, WalletError};
    let measuring = status.is_sampling();
    let (es, en) = match coverage {
        WalletCoverage::Listed(count) if measuring => return format!("{}: {count}", translated(english, "Monedas cubiertas", "Currencies covered")),
        WalletCoverage::Unavailable(error) if measuring => match error {
            WalletError::Guard => ("perfil de cartera no verificado", "wallet profile not verified"),
            WalletError::Profile => ("estructura desconocida", "unknown structure"),
            WalletError::Root => ("personaje no disponible", "character unavailable"),
            WalletError::Bounds => ("mapa fuera de límites", "map out of bounds"),
            WalletError::Empty => ("cartera vacía o ausente", "wallet empty or absent"),
            WalletError::Integrity => ("mapa incoherente", "inconsistent map"),
            WalletError::Range => ("valor fuera de rango", "value out of range"),
            WalletError::Changed => ("cambió durante la lectura", "changed while reading"),
            WalletError::ReadFailed => ("lectura fallida", "read failed"),
        },
        _ => ("sin lectura", "no reading"),
    };
    format!("{} ({})", translated(english, "Monedas: sin cobertura", "Currencies: no coverage"), translated(english, es, en))
}

/// Optional native window. It only renders validated host snapshots, never reads the API,
/// sends game input, starts/stops a session, or extrapolates a counter from stale data.
pub fn farming_render(ui: &Ui) {
    let (mut opened, english, reset) = {
        let mut panel = pending().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if !panel.show_farming_panel { return; }
        let reset = std::mem::take(&mut panel.reset_farming_position);
        (panel.show_farming_panel, panel.farming_english, reset)
    };
    let shared = state::shared();
    let view = shared.farming_view(std::time::Instant::now());
    let scale = (ui.current_font_size() / 13.0).max(1.0);
    let tr = |es, en| translated(english, es, en);
    let padding = ui.push_style_var(StyleVar::WindowPadding([12.0 * scale, 12.0 * scale]));
    let spacing = ui.push_style_var(StyleVar::ItemSpacing([8.0 * scale, 8.0 * scale]));
    Window::new(tr("Tyrian · Laberinto###TyrianFarming", "Tyrian · Labyrinth###TyrianFarming"))
        .opened(&mut opened)
        .position([80.0 * scale, 80.0 * scale], if reset { Condition::Always } else { Condition::FirstUseEver })
        .size([288.0 * scale, 0.0], Condition::FirstUseEver)
        .size_constraints([250.0 * scale, 0.0], [320.0 * scale, f32::MAX])
        .always_auto_resize(true)
        .bg_alpha(1.0)
        .build(ui, || {
            if let Some(reading) = &view.reading {
                ui.text_wrapped(phase_label(reading.phase, english));
                if let Some(error) = reading.err {
                    text_colored_wrapped(ui, RED, farming_error_label(error, english));
                    ui.text_wrapped(tr("Revisa la sesión en Hebra u Obsidian", "Check the session in Hebra or Obsidian"));
                }
                if !view.fresh {
                    text_colored_wrapped(ui, ORANGE, tr("Datos antiguos · última lectura", "Stale data · last reading"));
                }
                ui.separator();
                // Keep the labels readable at the host's font scale instead of importing fonts.
                ui.set_window_font_scale(1.35);
                ui.text_wrapped(number(reading.observed));
                ui.set_window_font_scale(1.0);
                ui.text_wrapped(tr("Bolsas observadas", "Observed bags"));
                let elapsed = reading.elapsed.map_or_else(|| "—".into(), duration);
                ui.text_wrapped(format!("{elapsed} · {}", tr("Duración", "Duration")));
                ui.separator();
                let rate = match (reading.lo, reading.hi) {
                    (Some(lo), Some(hi)) if hi != lo => format!("{lo}–{hi}"),
                    (Some(lo), None) => format!("≥{lo}"),
                    (Some(lo), Some(_)) => lo.to_string(),
                    _ => "—".into(),
                };
                ui.text_wrapped(format!("{rate} {}", tr("bolsas/h", "bags/h")));
                if reading.lo.is_none() { ui.text_wrapped(tr("Ritmo aún no disponible", "Rate not available yet")); }
                if !view.source_fresh() { ui.text_wrapped(tr("Último ritmo registrado", "Last recorded rate")); }
                ui.text_wrapped(view.age.map_or_else(
                    || tr("Sin lectura", "No reading").to_string(),
                    |age| format!("{} {age}s", tr("Última lectura hace", "Last reading ago:")),
                ));
                ui.separator();
                ui.text_wrapped(format!("{}: {} {}", tr("Huecos del personaje", "Character bag slots"),
                    number(reading.slots), tr("libres", "free")));
                if reading.slot_source == SlotSource::Recent {
                    ui.text_wrapped(tr("Personaje reciente", "Recent character"));
                }
                if let Some(age) = view.slot_age {
                    ui.text_wrapped(format!("{} {age}s", tr("Lectura de huecos hace", "Slot reading ago:")));
                } else { ui.text_wrapped(tr("Sin lectura de huecos", "No slot reading")); }

                if reading.goal != Goal::None {
                    ui.separator();
                    let (progress, target) = if reading.goal == Goal::Duration {
                        (reading.progress.map_or_else(|| "—".into(), duration),
                            reading.target.map_or_else(|| "—".into(), duration))
                    } else { (number(reading.progress), number(reading.target)) };
                    ui.text_wrapped(format!("{}: {progress} / {target} {}", tr("Objetivo", "Goal"),
                        if reading.goal == Goal::Bags { tr("bolsas", "bags") } else { "" }));
                    if let (Some(progress), Some(target)) = (reading.progress, reading.target) {
                        if target > 0 {
                            ProgressBar::new((progress as f32 / target as f32).clamp(0.0, 1.0))
                                .overlay_text("").build(ui);
                            if progress >= target { ui.text_wrapped(tr("Objetivo alcanzado", "Goal reached")); }
                        }
                    }
                    if let Some(eta) = view.eta() {
                        let label = if reading.goal == Goal::Duration { tr("Quedan", "Time left:") }
                            else { tr("Quedan aprox.", "Approx. time left:") };
                        ui.text_wrapped(format!("{label} {}", duration(eta)));
                    } else if reading.goal == Goal::Duration {
                        ui.text_wrapped(tr("Cuenta atrás no disponible", "Countdown unavailable"));
                    } else { ui.text_wrapped(tr("ETA aún no disponible", "ETA not available yet")); }
                }
                if let Some(net) = reading.net {
                    ui.separator();
                    ui.text_wrapped(format!("{net} · {}", tr("Bolsas netas al cierre", "Net bags at close")));
                    ui.text_wrapped(tr("El neto puede ser menor si abriste o gastaste bolsas.",
                        "Net bags may be lower if you opened or spent bags."));
                }
                if matches!(reading.phase, Phase::Stopping | Phase::Provisional) {
                    ui.text_wrapped(tr("Esperando la lectura final", "Waiting for the final reading"));
                }
                if reading.phase == Phase::Starting {
                    ui.text_wrapped(tr("Capturando el punto de partida", "Capturing the starting point"));
                }
                if reading.phase == Phase::Error && reading.err.is_none() {
                    ui.text_wrapped(tr("Revisa la sesión en Hebra u Obsidian", "Check the session in Hebra or Obsidian"));
                }
                if ui.collapsing_header(tr("Preparación opcional", "Optional preparation"), TreeNodeFlags::empty()) {
                    let mf = if reading.mf_kind == MagicFindKind::Partial { number(reading.mf) } else { "—".into() };
                    let label = if reading.mf_kind == MagicFindKind::Partial { tr("Parcial", "Partial") }
                        else { tr("Desconocido", "Unknown") };
                    ui.text_wrapped(format!("{}: {mf}% · {label}", tr("Hallazgo mágico", "Magic Find")));
                    ui.text_wrapped(match reading.prep {
                        Preparation::Attention => tr("Preparación: revisar en el host", "Preparation: check in the host"),
                        Preparation::Partial => tr("Preparación parcial", "Partial preparation"),
                        Preparation::Unknown => tr("Preparación desconocida", "Preparation unknown"),
                    });
                    ui.text_wrapped(tr("Buffs temporales sin verificar. Recordatorios manuales en el host.",
                        "Temporary buffs unverified. Manual reminders in the host."));
                }
            } else {
                ui.text_wrapped(if shared.connected() { tr("Sin medición", "Not measuring") }
                    else { tr("Sin conexión", "Offline") });
                ui.separator();
                ui.text_wrapped(tr("— · Bolsas observadas", "— · Observed bags"));
                ui.text_wrapped(tr("Sin lectura", "No reading"));
            }
            ui.separator();
            ui.text_wrapped(inventory_status(shared.live_status(), english));
            ui.text_wrapped(wallet_status(shared.live_status(), shared.inventory_diagnostics().wallet, english));
            ui.text_wrapped(tr("Hallazgo mágico verificado: sin cobertura", "Verified Magic Find: no coverage"));
            ui.text_wrapped(if shared.connected() { tr("Conexión al host: conectado", "Host connection: connected") }
                else { tr("Conexión al host: sin conexión", "Host connection: offline") });
            if shared.connected() && !view.capable {
                ui.text_wrapped(tr("Panel no disponible en este host", "Panel unavailable in this host"));
            } else if view.capable && view.reading.is_none() {
                ui.text_wrapped(tr("Esperando sesión", "Waiting for session"));
            }
        });
    spacing.pop();
    padding.pop();
    if !opened {
        let mut panel = pending().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        panel.show_farming_panel = false;
        save_panel_settings(&panel);
    }
}

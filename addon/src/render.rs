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
//!
//! The Labyrinth panel is painted here too, and only painted: which text, tone and tooltip each
//! of its cells has is decided by `tyrian_companion_nexus_core::panel`.

use std::sync::{Mutex, OnceLock};

use nexus::imgui::{Condition, DrawListMut, StyleColor, StyleVar, TreeNodeFlags, Ui, Window};

use tyrian_companion_nexus_core::quick_access::{PanelWindows, Shortcut};
use tyrian_companion_nexus_core::panel::{self, Cell, PanelInput, PanelMemory, Tone};

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
    /// Which of the addon's own windows are open and how the panel is shown; all but the
    /// options window are persisted settings.
    windows: PanelWindows,
    farming_english: bool,
    reset_farming_position: bool,
}

/// Seeded from the loaded settings once, at `load()`; see `init_pending`.
static PENDING: OnceLock<Mutex<Pending>> = OnceLock::new();

/// What the panel carries from one frame to the next (`panel::PanelMemory`).
static PANEL_MEMORY: Mutex<PanelMemory> = Mutex::new(PanelMemory::new());

/// `notice` is shown under the fields from the first frame: `load()` passes one when it removed
/// an API key from `settings.json`.
pub fn init_pending(settings: &Settings, notice: Option<&'static str>) {
    let _ = PENDING.set(Mutex::new(Pending { port: i32::from(settings.port), token: settings.token.clone(), notice,
        windows: PanelWindows::from_settings(settings), farming_english: settings.farming_english,
        reset_farming_position: false }));
}

fn pending() -> &'static Mutex<Pending> {
    PENDING.get_or_init(|| Mutex::new(Pending { port: i32::from(DEFAULT_PORT), token: String::new(), notice: None,
        windows: PanelWindows::default(), farming_english: false, reset_farming_position: false }))
}

const GREEN: [f32; 4] = [0.45, 0.85, 0.45, 1.0];
const ORANGE: [f32; 4] = [0.90, 0.65, 0.30, 1.0];
const RED: [f32; 4] = [0.95, 0.35, 0.35, 1.0];
const GREY: [f32; 4] = [0.70, 0.70, 0.70, 1.0];
/// Behind the text and the icons of the panel when it has no background.
const OUTLINE: [f32; 4] = [0.0, 0.0, 0.0, 0.90];

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
    ui.text_wrapped(panel::inventory_status(shared.live_status(), true));
    ui.text_wrapped(panel::wallet_status(shared.live_status(), shared.inventory_diagnostics().wallet, true));
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
                    save_panel_settings(&pending);
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
            save_panel_settings(&panel);
        }
        if let Some(outcome) = shared.obsidian_launch_outcome() {
            let (color, text) = launch_line(launch_app, outcome);
            text_colored_wrapped(ui, color, &text);
        }
    }

    ui.separator();
    {
        let mut panel = pending().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut changed = ui.checkbox("Show Labyrinth farming panel / Mostrar panel de Laberinto", &mut panel.windows.show_panel);
        if ui.checkbox("Farming panel in English / Panel en inglés", &mut panel.farming_english) {
            changed = true;
            crate::quick_access::refresh_tooltips(panel.farming_english);
        }
        // The same switch as the button on the panel's own title bar, for when the panel is
        // hard to hit against the game.
        changed |= ui.checkbox("Farming panel without background / Panel sin fondo", &mut panel.windows.transparent);
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

/// Saves only applied settings, so toggling the panel cannot save an unfinished token paste:
/// port, token and the launch choice come from the shared state, never from the input boxes.
fn save_panel_settings(panel: &Pending) {
    let shared = state::shared();
    if let Ok(dir) = nexus::paths::get_addon_dir(ADDON_DIR_NAME) {
        let settings = panel.windows.apply_to(Settings {
            port: shared.port(), token: shared.token(),
            open_obsidian_on_start: shared.open_obsidian_on_start(), launch_app: shared.launch_app(),
            farming_english: panel.farming_english, ..Settings::default()
        });
        if let Err(error) = settings::save(&dir, &settings) { log::error!("failed to save settings: {error}"); }
    }
}

fn translated<'a>(english: bool, spanish: &'a str, en: &'a str) -> &'a str {
    if english { en } else { spanish }
}

/// What a quick access icon (or the key the player assigned to it) does. Called from Nexus's
/// input thread; only flips a window flag and, for the panel, saves the shared setting.
pub fn activate_shortcut(shortcut: Shortcut) {
    let mut panel = pending().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    shortcut.activate(&mut panel.windows);
    if shortcut == Shortcut::Panel { save_panel_settings(&panel); }
}

/// Per-frame housekeeping and the addon's own Options window, opened from the quick access bar.
/// Nexus has no call to open its Options window on an addon's section, so this window paints
/// the very same content (`options_render`) and closes with its cross.
pub fn options_window_render(ui: &Ui) {
    let (english, mut opened) = {
        let panel = pending().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        (panel.farming_english, panel.windows.show_options)
    };
    crate::quick_access::register_pending(english);
    if !opened { return; }
    let scale = (ui.current_font_size() / 13.0).max(1.0);
    Window::new(translated(english, "Tyrian Companion · Opciones###TyrianOptions", "Tyrian Companion · Options###TyrianOptions"))
        .opened(&mut opened)
        .position([120.0 * scale, 120.0 * scale], Condition::FirstUseEver)
        .size([420.0 * scale, 480.0 * scale], Condition::FirstUseEver)
        .build(ui, || options_render(ui));
    if !opened {
        pending().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).windows.show_options = false;
    }
}

/// How much larger the observed bags are painted than the rest.
const LARGE: f32 = 1.6;

/// The colour of a tone: the host's text colour, or one of the four this file already had.
fn tone_color(ui: &Ui, tone: Tone) -> [f32; 4] {
    match tone {
        Tone::Normal => ui.style_color(StyleColor::Text),
        Tone::Muted => GREY,
        Tone::Good => GREEN,
        Tone::Warning => ORANGE,
        Tone::Error => RED,
    }
}

/// One line of text at the cursor. Without a window background it gets a dark outline, four
/// copies one pixel away, so it stays readable over the game.
fn paint_text(ui: &Ui, draw: &DrawListMut<'_>, text: &str, color: [f32; 4], outlined: bool) {
    if outlined {
        let [x, y] = ui.cursor_screen_pos();
        for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)] {
            draw.add_text([x + dx, y + dy], OUTLINE, text);
        }
    }
    ui.text_colored(color, text);
}

/// One cell of `panel::PanelView`: its text in its tone, and its tooltip under the mouse.
fn paint_cell(ui: &Ui, draw: &DrawListMut<'_>, cell: &Cell, outlined: bool) {
    paint_text(ui, draw, &cell.text, tone_color(ui, cell.tone), outlined);
    if ui.is_item_hovered() { ui.tooltip_text(cell.tooltip.join("\n")); }
}

/// The three buttons of the panel's own title bar. They are drawn, not written: the host's font
/// is not known to have a glyph for any of them.
#[derive(Clone, Copy)]
enum Icon {
    /// A triangle: down while the panel is open, right while it is folded.
    Fold { collapsed: bool },
    /// A square: filled while the panel has its background, hollow while it does not.
    Background { transparent: bool },
    /// A cross.
    Close,
}

/// A square button of the title bar with a drawn icon; true when clicked.
fn icon_button(ui: &Ui, draw: &DrawListMut<'_>, id: &str, size: f32, icon: Icon, tooltip: &str, outlined: bool) -> bool {
    let [x, y] = ui.cursor_screen_pos();
    let clicked = ui.invisible_button(id, [size, size]);
    let hovered = ui.is_item_hovered();
    if hovered {
        draw.add_rect([x, y], [x + size, y + size], ui.style_color(StyleColor::ButtonHovered)).filled(true).rounding(size * 0.15).build();
        ui.tooltip_text(tooltip);
    }
    let color = if hovered { ui.style_color(StyleColor::Text) } else { GREY };
    let (inset, thickness) = (size * 0.30, (size * 0.09).max(1.0));
    let (left, top, right, bottom) = (x + inset, y + inset, x + size - inset, y + size - inset);
    let shape = |dx: f32, dy: f32, color: [f32; 4]| {
        let (left, top, right, bottom) = (left + dx, top + dy, right + dx, bottom + dy);
        match icon {
            Icon::Fold { collapsed: false } => draw.add_triangle([left, top], [right, top], [(left + right) / 2.0, bottom], color).filled(true).build(),
            Icon::Fold { collapsed: true } => draw.add_triangle([left, top], [right, (top + bottom) / 2.0], [left, bottom], color).filled(true).build(),
            Icon::Background { transparent } => draw.add_rect([left, top], [right, bottom], color).filled(!transparent).thickness(thickness).build(),
            Icon::Close => {
                draw.add_line([left, top], [right, bottom], color).thickness(thickness).build();
                draw.add_line([left, bottom], [right, top], color).thickness(thickness).build();
            }
        }
    };
    if outlined { shape(1.0, 1.0, OUTLINE); }
    shape(0.0, 0.0, color);
    clicked
}

/// What the player did on the panel's title bar this frame.
#[derive(Default)]
struct BarClicks {
    fold: bool,
    background: bool,
    close: bool,
}

/// The Labyrinth panel. It only renders validated host snapshots, never reads the API, sends
/// game input, starts/stops a session, or extrapolates a counter from stale data.
///
/// The window has no native title bar: ImGui's cannot hold a button of ours, and the sketch asks
/// for one that removes the background. The bar painted here keeps what the native one had
/// (fold, title, close, and dragging the window by it or by any empty spot) and adds that
/// button. Its width, and so the window's, is reserved from `panel::width_samples`, not from
/// what the cells say now, so nothing moves when a text changes.
pub fn farming_render(ui: &Ui) {
    let (windows, english, reset) = {
        let mut panel = pending().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if !panel.windows.show_panel { return; }
        let reset = std::mem::take(&mut panel.reset_farming_position);
        (panel.windows, panel.farming_english, reset)
    };
    let shared = state::shared();
    let now = std::time::Instant::now();
    let (farming, price) = (shared.farming_view(now), shared.price_view(now));
    let view = {
        let mut memory = PANEL_MEMORY.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        panel::view(
            &PanelInput {
                connection: shared.status(),
                farming: &farming,
                price: &price,
                live: shared.live_status(),
                wallet: shared.inventory_diagnostics().wallet,
                // No validated reader for either yet: the lines say `—` until one exists.
                slots: None,
                magic_find: None,
            },
            &mut memory,
            english,
        )
    };
    let samples = panel::width_samples(english);
    let scale = (ui.current_font_size() / 13.0).max(1.0);
    let tr = |es, en| translated(english, es, en);
    let outlined = windows.transparent;
    let padding = ui.push_style_var(StyleVar::WindowPadding([12.0 * scale, 12.0 * scale]));
    let spacing = ui.push_style_var(StyleVar::ItemSpacing([8.0 * scale, 8.0 * scale]));
    let clicks = Window::new(tr("Tyrian · Laberinto###TyrianFarming", "Tyrian · Labyrinth###TyrianFarming"))
        .title_bar(false)
        .position([80.0 * scale, 80.0 * scale], if reset { Condition::Always } else { Condition::FirstUseEver })
        .always_auto_resize(true)
        .scroll_bar(false)
        .draw_background(!windows.transparent)
        .bg_alpha(1.0)
        .build(ui, || {
            let draw = ui.get_window_draw_list();
            let widest = |texts: &[String]| texts.iter().map(|text| ui.calc_text_size(text)[0]).fold(0.0, f32::max);
            let (gap, button, line) = (16.0 * scale, ui.frame_height(), ui.text_line_height());
            let title = tr("Tyrian · Laberinto", "Tyrian · Labyrinth");
            let left = widest(&samples.left).max(widest(&samples.left_large) * LARGE);
            let status_start = ui.calc_text_size(&view.status_label)[0] + 8.0 * scale + line + 8.0 * scale;
            let width = (left + gap + widest(&samples.right))
                .max(widest(&samples.lines))
                .max(status_start + widest(&samples.status))
                .max(button + 8.0 * scale + ui.calc_text_size(title)[0] + gap + 2.0 * button + 8.0 * scale);
            let origin = ui.cursor_pos()[0];

            // The title bar: fold, title, and on the right the background switch and the cross.
            let mut clicks = BarClicks::default();
            let [bar_x, bar_y] = ui.cursor_screen_pos();
            clicks.fold = icon_button(ui, &draw, "##fold", button, Icon::Fold { collapsed: windows.collapsed },
                if windows.collapsed { tr("Desplegar", "Unfold") } else { tr("Plegar", "Fold") }, outlined);
            let title_at = [bar_x + button + 8.0 * scale, bar_y + (button - line) / 2.0];
            if outlined {
                for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)] {
                    draw.add_text([title_at[0] + dx, title_at[1] + dy], OUTLINE, title);
                }
            }
            draw.add_text(title_at, ui.style_color(StyleColor::Text), title);
            ui.same_line_with_pos(origin + width - 2.0 * button - 8.0 * scale);
            clicks.background = icon_button(ui, &draw, "##background", button, Icon::Background { transparent: windows.transparent },
                if windows.transparent { tr("Poner el fondo del panel", "Give the panel its background") }
                else { tr("Quitar el fondo del panel", "Remove the panel's background") }, outlined);
            ui.same_line();
            clicks.close = icon_button(ui, &draw, "##close", button, Icon::Close, tr("Cerrar el panel", "Close the panel"), outlined);
            if windows.collapsed { return clicks; }

            // Two columns: bags and their rate, the stack and its two prices.
            let second = origin + left + gap;
            ui.separator();
            paint_cell(ui, &draw, &view.bags_label, outlined);
            ui.same_line_with_pos(second);
            paint_cell(ui, &draw, &view.stack_label, outlined);
            // Larger at the host's font scale instead of importing a font.
            ui.set_window_font_scale(LARGE);
            paint_cell(ui, &draw, &view.bags, outlined);
            ui.set_window_font_scale(1.0);
            ui.same_line_with_pos(second);
            paint_cell(ui, &draw, &view.buy, outlined);
            paint_cell(ui, &draw, &view.rate, outlined);
            ui.same_line_with_pos(second);
            paint_cell(ui, &draw, &view.sell, outlined);

            // Three full-width lines. The status is a label, a dot and a text with one tooltip.
            ui.separator();
            paint_cell(ui, &draw, &view.slots, outlined);
            paint_cell(ui, &draw, &view.magic_find, outlined);
            ui.group(|| {
                paint_text(ui, &draw, &view.status_label, ui.style_color(StyleColor::Text), outlined);
                ui.same_line();
                let [x, y] = ui.cursor_screen_pos();
                ui.dummy([line, line]);
                let centre = [x + line / 2.0, y + line / 2.0];
                if outlined { draw.add_circle(centre, line * 0.32 + 1.0, OUTLINE).filled(true).build(); }
                draw.add_circle(centre, line * 0.32, tone_color(ui, view.status_dot)).filled(true).build();
                ui.same_line();
                paint_text(ui, &draw, &view.status.text, tone_color(ui, view.status.tone), outlined);
            });
            if ui.is_item_hovered() { ui.tooltip_text(view.status.tooltip.join("\n")); }
            clicks
        })
        .unwrap_or_default();
    spacing.pop();
    padding.pop();
    if clicks.fold || clicks.background || clicks.close {
        let mut panel = pending().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if clicks.fold { panel.windows.collapsed = !panel.windows.collapsed; }
        if clicks.background { panel.windows.transparent = !panel.windows.transparent; }
        if clicks.close { panel.windows.show_panel = false; }
        save_panel_settings(&panel);
    }
}

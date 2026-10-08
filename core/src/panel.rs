//! The Labyrinth panel as data: what each cell says, in which tone, and what its tooltip adds.
//! The Windows-only `addon` crate paints exactly this; nothing about what to show is decided
//! there, which is what lets `cargo test` cover every state of the panel.
//!
//! The panel has a fixed shape (David's sketch of 8 Oct 2026): two columns, bags and their
//! rate on the left and the gross price of a stack of 250 on the right, then three lines for
//! the free slots, the Magic Find and the status. Every cell exists in every state and says
//! `—` without a figure, so the window never grows, shrinks or moves a line. What used to be
//! lines of their own (duration, goal and ETA, net bags at close, stale data, inventory and
//! wallet coverage, host connection) is in the tooltip of the cell it is about.
//!
//! Colour is never the only signal: the text states the status and the tooltip the detail.

use crate::farming::{FarmingError, FarmingView, Goal, Phase};
use crate::live::LiveStatus;
use crate::price::{format_coins, PriceStatus, PriceView};
use crate::state::Status;
use crate::wallet::{WalletCoverage, WalletError};

/// What a cell says when there is no figure.
pub const NO_DATA: &str = "—";

/// Free slots at or below this paint the line in the warning colour.
pub const SLOTS_WARNING: i32 = 10;
/// Free slots at or below this paint the line in the error colour.
pub const SLOTS_ERROR: i32 = 3;

/// The colour role of a cell. The addon maps each to one colour it already had.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// The host's own text colour.
    Normal,
    /// No figure, or nothing going on: grey.
    Muted,
    /// Connected and measuring: green. Only the status dot uses it.
    Good,
    /// Something worth reading in the tooltip: orange.
    Warning,
    /// An error, or no connection: red.
    Error,
}

/// One painted piece of text. `tooltip` has one entry per line and is never empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub text: String,
    pub tone: Tone,
    pub tooltip: Vec<String>,
}

/// What the panel remembers from one frame to the next.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PanelMemory {
    /// The rate is being shown as one averaged number instead of its range.
    rate_averaged: bool,
    /// Highest Magic Find seen in the current session.
    magic_find_peak: Option<i32>,
}

impl PanelMemory {
    /// Nothing remembered: a range is a range and no Magic Find has been seen.
    pub const fn new() -> Self {
        Self { rate_averaged: false, magic_find_peak: None }
    }
}

/// Everything the panel is painted from.
#[derive(Debug, Clone)]
pub struct PanelInput<'a> {
    pub connection: Status,
    pub farming: &'a FarmingView,
    pub price: &'a PriceView,
    pub live: LiveStatus,
    pub wallet: WalletCoverage,
    /// Free bag slots of the character from a validated reader. There is none yet: the addon
    /// passes `None` and the line says `—`.
    pub slots: Option<i32>,
    /// Magic Find percentage from a validated reader. None yet, as above.
    pub magic_find: Option<i32>,
}

/// The whole panel. Its shape is its type: there is no state in which a line is missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelView {
    /// Left column: its label, the observed bags (painted large) and the rate.
    pub bags_label: Cell,
    pub bags: Cell,
    pub rate: Cell,
    /// Right column: its label, the highest buy order and the lowest sell offer for 250 bags.
    pub stack_label: Cell,
    pub buy: Cell,
    pub sell: Cell,
    /// The three full-width lines.
    pub slots: Cell,
    pub magic_find: Cell,
    /// `Estado:`, then a dot in `status_dot`'s colour, then `status`.
    pub status_label: String,
    pub status_dot: Tone,
    pub status: Cell,
}

impl PanelView {
    /// Lines of the body in every state: label, figure and rate, then slots, Magic Find, status.
    pub const LINES: usize = 6;

    /// Every cell, for checks that hold for all of them.
    pub fn cells(&self) -> [&Cell; 9] {
        [
            &self.bags_label,
            &self.bags,
            &self.rate,
            &self.stack_label,
            &self.buy,
            &self.sell,
            &self.slots,
            &self.magic_find,
            &self.status,
        ]
    }
}

fn tr(english: bool, es: &'static str, en: &'static str) -> &'static str {
    if english {
        en
    } else {
        es
    }
}

fn cell(text: impl Into<String>, tone: Tone, tooltip: Vec<String>) -> Cell {
    Cell { text: text.into(), tone, tooltip }
}

/// `1:02:03`, as the panel has always written a duration.
pub fn duration(seconds: i32) -> String {
    let seconds = seconds.max(0) as u32;
    format!("{}:{:02}:{:02}", seconds / 3600, seconds / 60 % 60, seconds % 60)
}

pub fn phase_label(phase: Phase, english: bool) -> &'static str {
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
    tr(english, es, en)
}

/// Failures are independent of phase: an active session may fail an observation and a
/// completed session may fail to save. Both stay visible until the host clears them.
pub fn farming_error_label(error: FarmingError, english: bool) -> &'static str {
    let (es, en) = match error {
        FarmingError::Start => ("No se pudo empezar", "Could not start"),
        FarmingError::Observe => ("No se pudo actualizar", "Could not update"),
        FarmingError::Stop => ("No se pudo finalizar", "Could not finish"),
        FarmingError::Save => ("No se pudo guardar", "Could not save"),
        FarmingError::Other => ("Error de sesión", "Session error"),
    };
    tr(english, es, en)
}

/// Source status is separate from both host connectivity and persisted session phase.
pub fn inventory_status(status: LiveStatus, english: bool) -> &'static str {
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
    tr(english, es, en)
}

/// Wallet coverage of the last capture: how many currencies it listed, or the closed reason it
/// listed none. Covered means those IDs only; an unlisted currency is unknown, never zero. With
/// no measurement in progress there is no capture to describe, whatever an older one found.
pub fn wallet_status(status: LiveStatus, coverage: WalletCoverage, english: bool) -> String {
    let measuring = status.is_sampling();
    let (es, en) = match coverage {
        WalletCoverage::Listed(count) if measuring => {
            return format!("{}: {count}", tr(english, "Monedas cubiertas", "Currencies covered"))
        }
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
    format!("{} ({})", tr(english, "Monedas: sin cobertura", "Currencies: no coverage"), tr(english, es, en))
}

/// Whether a band this wide is shown as its average. Above 0.30 of its mean it is; below 0.20
/// it is a range again; in between it stays as it was, so a band hovering around one threshold
/// does not switch back and forth. Integer arithmetic: `(hi - lo) / ((lo + hi) / 2)`.
fn averaged(lo: i32, hi: i32, was: bool) -> bool {
    let (width, sum) = (i64::from(hi) - i64::from(lo), i64::from(lo) + i64::from(hi));
    if 20 * width > 3 * sum {
        true
    } else if 10 * width < sum {
        false
    } else {
        was
    }
}

/// The rate. A band is shown as `lo–hi`, or as `~mean` while it is wide (see [`averaged`]),
/// with the band itself in the tooltip. A band one unit wide is one number rounded down and
/// up, which is what a live session sends, and is shown as that number. Notes about the rate
/// ([`FarmingView::rate_line`]) go to the tooltip and paint it in the warning tone.
fn rate_cell(view: &FarmingView, memory: &mut PanelMemory, english: bool) -> Cell {
    let unit = "b/h";
    let mut tooltip = vec![tr(english, "Bolsas por hora", "Bags per hour").to_string()];
    let Some(reading) = view.reading.as_ref() else {
        memory.rate_averaged = false;
        tooltip.push(tr(english, "Sin lectura", "No reading").to_string());
        return cell(format!("{NO_DATA} {unit}"), Tone::Muted, tooltip);
    };
    let figure = match (reading.lo, reading.hi) {
        (Some(lo), Some(hi)) if i64::from(hi) - i64::from(lo) > 1 => {
            memory.rate_averaged = averaged(lo, hi, memory.rate_averaged);
            if memory.rate_averaged {
                let mean = (i64::from(lo) + i64::from(hi) + 1) / 2;
                tooltip.push(format!("{}: {lo}–{hi} {unit}", tr(english, "Rango", "Range")));
                format!("~{mean}")
            } else {
                format!("{lo}–{hi}")
            }
        }
        (Some(lo), hi) => {
            memory.rate_averaged = false;
            if hi.is_some() { lo.to_string() } else { format!("≥{lo}") }
        }
        (None, _) => {
            memory.rate_averaged = false;
            NO_DATA.to_string()
        }
    };
    let notes = view.rate_line(english).map(|line| line.notes).unwrap_or_default();
    let tone = if notes.is_empty() { Tone::Normal } else { Tone::Warning };
    tooltip.extend(notes);
    cell(format!("{figure} {unit}"), tone, tooltip)
}

/// The observed bags. Its tooltip keeps what left the panel: duration, goal and ETA, the net at
/// close and the notes of a phase. It turns to the warning tone when the figure is old or the
/// net at close came out lower than what was observed.
fn bags_cell(view: &FarmingView, english: bool) -> Cell {
    let mut tooltip = vec![tr(english, "Bolsas observadas", "Observed bags").to_string()];
    let Some(reading) = view.reading.as_ref() else {
        tooltip.push(tr(english, "Sin lectura", "No reading").to_string());
        return cell(NO_DATA, Tone::Muted, tooltip);
    };
    let mut tone = Tone::Normal;
    if !view.fresh {
        tooltip.push(tr(english, "Datos antiguos · última lectura", "Stale data · last reading").to_string());
        tone = Tone::Warning;
    }
    let elapsed = reading.elapsed.map_or_else(|| NO_DATA.to_string(), duration);
    tooltip.push(format!("{}: {elapsed}", tr(english, "Duración", "Duration")));
    if reading.goal != Goal::None {
        let (progress, target) = if reading.goal == Goal::Duration {
            (reading.progress.map_or_else(|| NO_DATA.to_string(), duration), reading.target.map_or_else(|| NO_DATA.to_string(), duration))
        } else {
            let number = |value: Option<i32>| value.map_or_else(|| NO_DATA.to_string(), |value| value.to_string());
            (number(reading.progress), number(reading.target))
        };
        let unit = if reading.goal == Goal::Bags { tr(english, " bolsas", " bags") } else { "" };
        tooltip.push(format!("{}: {progress} / {target}{unit}", tr(english, "Objetivo", "Goal")));
        if matches!((reading.progress, reading.target), (Some(progress), Some(target)) if target > 0 && progress >= target) {
            tooltip.push(tr(english, "Objetivo alcanzado", "Goal reached").to_string());
        }
        tooltip.push(match (view.eta(), reading.goal) {
            (Some(eta), Goal::Duration) => format!("{} {}", tr(english, "Quedan", "Time left:"), duration(eta)),
            (Some(eta), _) => format!("{} {}", tr(english, "Quedan aprox.", "Approx. time left:"), duration(eta)),
            (None, Goal::Duration) => tr(english, "Cuenta atrás no disponible", "Countdown unavailable").to_string(),
            (None, _) => tr(english, "ETA aún no disponible", "ETA not available yet").to_string(),
        });
    }
    if let Some(net) = reading.net {
        tooltip.push(format!("{}: {net}", tr(english, "Bolsas netas al cierre", "Net bags at close")));
        if reading.observed.is_some_and(|observed| net < observed) {
            tooltip.push(tr(english, "El neto es menor si abriste o gastaste bolsas.", "Net bags are lower if you opened or spent bags.").to_string());
            tone = Tone::Warning;
        }
    }
    match reading.phase {
        Phase::Stopping | Phase::Provisional => tooltip.push(tr(english, "Esperando la lectura final", "Waiting for the final reading").to_string()),
        Phase::Starting => tooltip.push(tr(english, "Capturando el punto de partida", "Capturing the starting point").to_string()),
        _ => {}
    }
    cell(reading.observed.map_or_else(|| NO_DATA.to_string(), |observed| observed.to_string()), tone, tooltip)
}

/// The right column: label, highest buy order and lowest sell offer for a stack of 250, gross.
/// Figures only from a fresh `ok` reading; every other state says `—` and why in the tooltip.
fn stack_cells(input: &PanelInput<'_>, english: bool) -> (Cell, Cell, Cell) {
    let view = input.price;
    let reading = view.reading.as_ref().filter(|_| view.capable && view.fresh);
    let figures = reading.filter(|reading| reading.st == PriceStatus::Ok);
    let idle = view.reading.as_ref().is_some_and(|reading| reading.st == PriceStatus::Idle);
    // Why there is no figure, or nothing when there is one.
    let (state, tone): (Option<String>, Tone) = if input.connection != Status::Connected {
        (Some(tr(english, "Sin conexión", "Offline").to_string()), Tone::Muted)
    } else if !view.capable {
        (Some(tr(english, "Precio no disponible en este host", "Price unavailable in this host").to_string()), Tone::Muted)
    } else if idle {
        (Some(tr(english, "El precio se lee durante una sesión", "The price is read during a session").to_string()), Tone::Muted)
    } else {
        match reading.map(|reading| reading.st) {
            None | Some(PriceStatus::Pending | PriceStatus::Idle) => {
                (Some(tr(english, "Precio aún sin leer", "Price not read yet").to_string()), Tone::Muted)
            }
            Some(PriceStatus::Stale) => {
                let text = match view.age.or(reading.and_then(|reading| reading.age).map(|age| age as u64)) {
                    Some(age) if english => format!("Price expired ({} min ago)", age / 60),
                    Some(age) => format!("Precio caducado (hace {} min)", age / 60),
                    None => tr(english, "Precio caducado", "Price expired").to_string(),
                };
                (Some(text), Tone::Warning)
            }
            Some(PriceStatus::Ok) if figures.is_some_and(|reading| reading.sell.is_none() && reading.list.is_none()) => {
                (Some(tr(english, "Sin cotización", "No quote").to_string()), Tone::Muted)
            }
            Some(PriceStatus::Ok) => (None, Tone::Normal),
        }
    };
    let side = |title: &'static str, unit: Option<i32>, stack: Option<i32>| {
        let mut tooltip = vec![format!("{title} × 250")];
        if let Some(unit) = unit {
            tooltip.push(format!("{}: {}", tr(english, "Unidad", "Unit"), format_coins(unit)));
        }
        tooltip.extend(state.clone());
        let side_tone = match stack {
            Some(_) => Tone::Normal,
            None if tone == Tone::Warning => Tone::Warning,
            None => Tone::Muted,
        };
        cell(stack.map_or_else(|| NO_DATA.to_string(), format_coins), side_tone, tooltip)
    };
    let mut label = vec![tr(english, "Stack de 250 sacos · precio bruto del bazar", "Stack of 250 bags · gross trading post price").to_string()];
    label.extend(state.clone());
    (
        cell("stack", if tone == Tone::Warning { Tone::Warning } else { Tone::Normal }, label),
        side(
            tr(english, "Pedido más alto", "Highest buy order"),
            figures.and_then(|reading| reading.sell),
            figures.and_then(|reading| reading.sell_stack),
        ),
        side(
            tr(english, "Oferta más baja", "Lowest sell offer"),
            figures.and_then(|reading| reading.list),
            figures.and_then(|reading| reading.list_stack),
        ),
    )
}

/// Free bag slots: `—` until a validated reader provides them; then the warning tone at
/// [`SLOTS_WARNING`] or fewer and the error tone at [`SLOTS_ERROR`] or fewer.
fn slots_cell(slots: Option<i32>, english: bool) -> Cell {
    let label = tr(english, "Huecos", "Slots");
    let mut tooltip = vec![tr(english, "Huecos libres en las bolsas del personaje", "Free bag slots of the character").to_string()];
    let Some(slots) = slots else {
        tooltip.push(tr(english, "Dato aún no disponible", "Not available yet").to_string());
        return cell(format!("{label}: {NO_DATA}"), Tone::Muted, tooltip);
    };
    let tone = if slots <= SLOTS_ERROR {
        Tone::Error
    } else if slots <= SLOTS_WARNING {
        Tone::Warning
    } else {
        Tone::Normal
    };
    if tone != Tone::Normal {
        tooltip.push(tr(english, "Quedan pocos huecos", "Few slots left").to_string());
    }
    cell(format!("{label}: {slots} {}", tr(english, "libres", "free")), tone, tooltip)
}

/// Magic Find: `—` until a validated reader provides it; then the warning tone while it is
/// below the highest value seen in the session, which the tooltip names.
fn magic_find_cell(magic_find: Option<i32>, view: &FarmingView, memory: &mut PanelMemory, english: bool) -> Cell {
    // A session that has not started, or none at all, has no peak to compare with.
    if view.reading.as_ref().is_none_or(|reading| matches!(reading.phase, Phase::Idle | Phase::Starting)) {
        memory.magic_find_peak = None;
    }
    let mut tooltip = vec![tr(english, "Hallazgo mágico", "Magic Find").to_string()];
    let Some(value) = magic_find else {
        tooltip.push(tr(english, "Hallazgo mágico verificado: sin cobertura", "Verified Magic Find: no coverage").to_string());
        return cell(format!("MF: {NO_DATA}"), Tone::Muted, tooltip);
    };
    let peak = memory.magic_find_peak.map_or(value, |peak| peak.max(value));
    memory.magic_find_peak = Some(peak);
    let tone = if value < peak { Tone::Warning } else { Tone::Normal };
    if tone == Tone::Warning {
        tooltip.push(format!("{}: {peak}%", tr(english, "Máximo de la sesión", "Session peak")));
    }
    cell(format!("MF: {value}%"), tone, tooltip)
}

/// What the status says when there is no connection, or `None` when there is one.
fn connection_text(connection: Status, english: bool) -> Option<(&'static str, Tone, &'static str)> {
    let (es, en, tone, detail_es, detail_en) = match connection {
        Status::Connected => return None,
        Status::WaitingForPlugin => ("Sin conexión", "Offline", Tone::Error, "Esperando a Tyrian Companion en Obsidian o Hebra", "Waiting for Tyrian Companion in Obsidian or Hebra"),
        Status::MissingToken => ("Falta el token", "Token missing", Tone::Error, "Pega el token en las opciones del addon", "Paste the token in the addon's options"),
        Status::TokenRejected => ("Token rechazado", "Token rejected", Tone::Error, "Copia el token de nuevo desde el plugin", "Copy the token again from the plugin"),
        Status::UpdateRequired => ("Actualiza el addon", "Update the addon", Tone::Error, "El plugin pide una versión más nueva de este addon", "The plugin needs a newer version of this addon"),
        Status::GameExiting => ("El juego se cierra", "The game is closing", Tone::Muted, "La conexión se ha cerrado con el juego", "The connection was closed with the game"),
    };
    Some((tr(english, es, en), tone, tr(english, detail_es, detail_en)))
}

/// The status line: what the dot's colour is, what the text says, and the detail in the tooltip.
///
/// - red, no connection or a session error: the text says which;
/// - orange, connected with something to read: old data, or an inventory source that cannot
///   measure while a session needs it;
/// - green, connected and a session under way;
/// - grey, connected with no session measuring, or the game closing.
///
/// The transient inventory states (`waiting for confirmation`, `unresolved quantities`) are in
/// the tooltip and do not change the colour: they come and go in normal measurement.
fn status_cell(input: &PanelInput<'_>, english: bool) -> (Tone, Cell) {
    let view = input.farming;
    let mut tooltip = Vec::new();
    let line = |es: &'static str, en: &'static str| tr(english, es, en).to_string();
    if let Some((text, tone, detail)) = connection_text(input.connection, english) {
        tooltip.push(line("Conexión al host: sin conexión", "Host connection: offline"));
        tooltip.push(detail.to_string());
        if view.reading.is_some() {
            tooltip.push(line("Datos antiguos · última lectura", "Stale data · last reading"));
        }
        return (tone, cell(text, if tone == Tone::Muted { Tone::Normal } else { tone }, tooltip));
    }
    tooltip.push(line("Conexión al host: conectado", "Host connection: connected"));
    let Some(reading) = view.reading.as_ref() else {
        let (text, tone) = if view.capable {
            (line("Esperando sesión", "Waiting for session"), Tone::Muted)
        } else {
            tooltip.push(line("Panel no disponible en este host", "Panel unavailable in this host"));
            (line("Sin panel", "No panel"), Tone::Warning)
        };
        tooltip.push(inventory_status(input.live, english).to_string());
        return (tone, cell(text, if tone == Tone::Muted { Tone::Normal } else { tone }, tooltip));
    };
    let measuring = matches!(reading.phase, Phase::Starting | Phase::Active | Phase::Stopping | Phase::Provisional);
    let mut tone = if measuring { Tone::Good } else { Tone::Muted };
    let mut text = phase_label(reading.phase, english);
    if !view.fresh {
        tooltip.push(line("Datos antiguos · última lectura", "Stale data · last reading"));
        tone = Tone::Warning;
    }
    let source_down = matches!(input.live, LiveStatus::NotNegotiated | LiveStatus::UnsupportedBuild | LiveStatus::Unavailable | LiveStatus::Conflict | LiveStatus::StorageUnavailable);
    if measuring && source_down {
        tone = Tone::Warning;
    }
    tooltip.push(inventory_status(input.live, english).to_string());
    tooltip.push(wallet_status(input.live, input.wallet, english));
    if input.live.is_sampling() && matches!(input.wallet, WalletCoverage::Unavailable(_)) && measuring && tone == Tone::Good {
        tone = Tone::Warning;
    }
    match reading.phase {
        Phase::Stopping | Phase::Provisional => tooltip.push(line("Esperando la lectura final", "Waiting for the final reading")),
        Phase::Starting => tooltip.push(line("Capturando el punto de partida", "Capturing the starting point")),
        _ => {}
    }
    if reading.err.is_some() || reading.phase == Phase::Error {
        if let Some(error) = reading.err {
            text = farming_error_label(error, english);
            tooltip.insert(1, format!("{}: {}", tr(english, "Fase", "Phase"), phase_label(reading.phase, english)));
        }
        tooltip.insert(1, line("Revisa la sesión en Hebra u Obsidian", "Check the session in Hebra or Obsidian"));
        tone = Tone::Error;
    }
    let text_tone = if matches!(tone, Tone::Warning | Tone::Error) { tone } else { Tone::Normal };
    (tone, cell(text, text_tone, tooltip))
}

/// The panel for this frame. `memory` carries the rate's range-or-average choice and the
/// session's Magic Find peak from the previous one.
pub fn view(input: &PanelInput<'_>, memory: &mut PanelMemory, english: bool) -> PanelView {
    let (stack_label, buy, sell) = stack_cells(input, english);
    let (status_dot, status) = status_cell(input, english);
    PanelView {
        bags_label: cell(
            tr(english, "bolsas", "bags"),
            Tone::Normal,
            vec![tr(english, "Bolsas observadas y su ritmo por hora", "Observed bags and their hourly rate").to_string()],
        ),
        bags: bags_cell(input.farming, english),
        rate: rate_cell(input.farming, memory, english),
        stack_label,
        buy,
        sell,
        slots: slots_cell(input.slots, english),
        magic_find: magic_find_cell(input.magic_find, input.farming, memory, english),
        status_label: tr(english, "Estado:", "Status:").to_string(),
        status_dot,
        status,
    }
}

/// The widest text each part of the panel is expected to hold, for the addon to reserve its
/// width once instead of following the content: a rate that turns from a range into an average,
/// or a status that changes, then moves nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidthSamples {
    /// Left column, at the normal size: the label and the longest rate.
    pub left: Vec<String>,
    /// Left column, at the large size: the observed bags.
    pub left_large: Vec<String>,
    /// Right column: the label and the longest price.
    pub right: Vec<String>,
    /// Full-width lines: slots and Magic Find.
    pub lines: Vec<String>,
    /// Every text the status can show, after its label and dot.
    pub status: Vec<String>,
}

pub fn width_samples(english: bool) -> WidthSamples {
    let mut status: Vec<String> = [Phase::Idle, Phase::Starting, Phase::Active, Phase::Stopping, Phase::Provisional, Phase::Complete, Phase::Abandoned, Phase::Error]
        .into_iter()
        .map(|phase| phase_label(phase, english).to_string())
        .collect();
    status.extend(
        [FarmingError::Start, FarmingError::Observe, FarmingError::Stop, FarmingError::Save, FarmingError::Other]
            .into_iter()
            .map(|error| farming_error_label(error, english).to_string()),
    );
    status.extend(
        [Status::WaitingForPlugin, Status::MissingToken, Status::TokenRejected, Status::UpdateRequired, Status::GameExiting]
            .into_iter()
            .filter_map(|connection| connection_text(connection, english).map(|(text, _, _)| text.to_string())),
    );
    status.push(tr(english, "Esperando sesión", "Waiting for session").to_string());
    status.push(tr(english, "Sin panel", "No panel").to_string());
    WidthSamples {
        left: vec![tr(english, "bolsas", "bags").to_string(), "9999–9999 b/h".to_string(), "~9999 b/h".to_string()],
        left_large: vec!["9999".to_string()],
        right: vec!["stack".to_string(), "999g 99s 99c".to_string()],
        lines: vec![
            slots_cell(Some(9999), english).text,
            magic_find_cell(Some(9999), &FarmingView { capable: false, reading: None, fresh: false, age: None, slot_age: None }, &mut PanelMemory::default(), english).text,
        ],
        status,
    }
}

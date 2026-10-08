//! The Labyrinth panel as data: what each cell says, in which tone, and what its tooltip adds.
//! The Windows-only `addon` crate paints exactly this; nothing about what to show is decided
//! there, which is what lets `cargo test` cover every state of the panel.
//!
//! The panel has a fixed shape (David's sketch of 8 Oct 2026): two columns, bags and their
//! rate on the left and the gross price of a stack of 250 on the right, then three lines for
//! the free slots, the Magic Find and the status. Every cell exists in every state and says
//! `—` without a figure, so the window never grows, shrinks or moves a line. What used to be
//! lines of their own (duration, goal and ETA, net bags at close, stale data, inventory and
//! wallet coverage, host connection, preparation) is in the tooltip of the cell it is about.
//!
//! Colour is never the only signal: the text states the status and the tooltip the detail.
//!
//! Free slots and Magic Find can come from two places, in this order: what the addon's own
//! passive reader read and verified in the last cycle ([`BagCoverage`], [`MagicFindCoverage`]),
//! and what the plugin sends in `farm1`. A Magic Find from the plugin is a value declared when
//! the session started, or a partial one: it does not follow the game, so it is written and
//! coloured apart and never warns about a drop. When the reader has no coverage the cell falls
//! back to the plugin's figure, or to `—`, and its tooltip says why the addon has none.

use std::time::{Duration, Instant};

use crate::bags::{BagCoverage, BagSlots};
use crate::farming::{FarmingError, FarmingView, Goal, MagicFindKind, Phase, Preparation, SlotSource};
use crate::live::LiveStatus;
use crate::magic_find::{MagicFind, MagicFindCoverage};
use crate::passive::Uncovered;
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
    /// No figure, a figure that is not a live reading, or nothing going on: grey.
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

/// How long the last verified reading of a line is still painted while the reader's cycles
/// come back without one. A single cycle that fails (`Changed`, `Deadline`, `ReadFailed`) would
/// otherwise swap the figure for the plugin's, or for `—`, for a second and back.
pub const READING_HOLD: Duration = Duration::from_secs(5);

/// A reading younger than this is the reader's current one. A cycle takes a second, so an older
/// one means a cycle is missing, and the tooltip then says how old the reading is.
const READING_CURRENT: Duration = Duration::from_secs(2);

/// The declared duration of a session falling by more than this many seconds, or back under
/// them, is another session and not a correction of the same one.
const SESSION_RESTART: i32 = 60;

/// What the panel remembers from one frame to the next. It has to see every frame, also the
/// ones in which the panel is closed ([`observe`]): a session that ends and another that starts
/// while it is closed would otherwise leave the first one's highest Magic Find in here.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct PanelMemory {
    /// The rate is being shown as one averaged number instead of its range.
    rate_averaged: bool,
    /// The highest verified Magic Find of the current session, with its addends. Only readings
    /// the reader returned while the session measures feed it, and it only goes up.
    magic_find_peak: Option<MagicFind>,
    /// From when a reading can be that highest: the frame in which this session, with this
    /// character, was first seen measuring. The reader's output stays as it is until another
    /// cycle replaces it, so without this a reading of up to [`READING_HOLD`] before the session
    /// started, or of the character before this one, would still be taken.
    peak_from: Option<Instant>,
    /// The last character the game context named, and since when the readings are its own: the
    /// frame in which a different one was first seen. Another character is another baseline.
    character: Option<String>,
    character_since: Option<Instant>,
    /// The last verified reading of each line and the instant of the reader's cycle that
    /// returned it, kept for [`READING_HOLD`].
    bags: Option<(BagSlots, Instant)>,
    magic_find: Option<(MagicFind, Instant)>,
    /// A session was under way in the previous frame, and how long it said it had lasted: a
    /// session that starts, or a duration that goes back, is another session.
    session_running: bool,
    elapsed: Option<i32>,
    /// The instant of the last capture of this connection that left the reader sampling. A
    /// capture that fails is a lasting problem only once this is older than [`READING_HOLD`].
    good_capture: Option<Instant>,
    /// The same for the last capture that listed the wallet.
    good_wallet: Option<Instant>,
}

impl PanelMemory {
    /// Nothing remembered: a range is a range and no reading has been seen.
    pub const fn new() -> Self {
        Self {
            rate_averaged: false,
            magic_find_peak: None,
            peak_from: None,
            character: None,
            character_since: None,
            bags: None,
            magic_find: None,
            session_running: false,
            elapsed: None,
            good_capture: None,
            good_wallet: None,
        }
    }

    /// The highest verified Magic Find total of the current session, if any reading fed it.
    pub fn magic_find_peak(&self) -> Option<f32> {
        self.magic_find_peak.map(|peak| peak.total)
    }
}

/// Why a live source has no capture of this second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gap {
    /// The last capture failed as a whole.
    CaptureFailed,
    /// There is no character in a map, so no capture was tried: character select, or a loading
    /// screen. Nothing failed.
    NoCharacter,
}

/// Whether the reader's output can describe the game now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// Cycles are running, or there is none of this second and `gap` says why: a reading counts
    /// for [`READING_HOLD`] from the instant of its own cycle.
    Live { gap: Option<Gap> },
    /// No connection, no negotiated source, a source conflict, an unsupported build, storage
    /// down or the game closing: nothing is read, so nothing counts and nothing is kept.
    Stopped,
}

/// One whole capture that fails (the inventory copy changed under it, could not be read, or
/// ran out of time) leaves `Unavailable` for the second until the next one. That is a failed
/// cycle, not a reader that stopped, and what was read just before is held through it. The
/// same status after the connection is lost is the reader stopped.
///
/// `Unavailable` is also what the source says while there is no character in a map. That is
/// held through in the same way, a loading screen being a matter of seconds, but it is told
/// apart ([`Gap`]), so that the panel does not call it a capture that failed.
fn source(input: &PanelInput<'_>) -> Source {
    if input.connection != Status::Connected {
        return Source::Stopped;
    }
    match input.live {
        live if live.is_sampling() => Source::Live { gap: None },
        LiveStatus::Unavailable if input.character_in_map => Source::Live { gap: Some(Gap::CaptureFailed) },
        LiveStatus::Unavailable => Source::Live { gap: Some(Gap::NoCharacter) },
        _ => Source::Stopped,
    }
}

/// Why a reading that is painted is not from the reader's current cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Why {
    /// The reader ran and said why it has none.
    Reader(Uncovered),
    /// No capture of this second, for this reason.
    Gap(Gap),
    /// No new cycle has run: the plugin is slow to confirm the last one.
    NoCycle,
}

/// A verified reading as a line takes it in this frame.
#[derive(Debug, Clone, Copy)]
struct Taken<T> {
    value: T,
    /// The reader's output of this frame is this reading, not one kept from before.
    read: bool,
    /// Set when it is not the current one: how many seconds ago it was read, and why.
    held: Option<(u64, Why)>,
}

/// The addon's reading of one line for this frame. A reading is as old as the cycle that read
/// it (`read_at`), not as the frame that paints it: the reader's diagnostics stay as they are
/// until another cycle replaces them, across a slow plugin and across a reconnection. A reading
/// counts for [`READING_HOLD`] from its own instant and then is dropped, whatever the
/// diagnostics still say. While the source is stopped nothing counts and nothing is kept.
fn take<T: Copy>(
    slot: &mut Option<(T, Instant)>,
    source: Source,
    read: Option<T>,
    reason: Option<Uncovered>,
    read_at: Option<Instant>,
    now: Instant,
) -> Option<Taken<T>> {
    let Source::Live { gap } = source else {
        *slot = None;
        return None;
    };
    let fresh = read.zip(read_at);
    if let Some(reading) = fresh {
        *slot = Some(reading);
    }
    let (value, at) = (*slot)?;
    let age = now.saturating_duration_since(at);
    if age > READING_HOLD {
        *slot = None;
        return None;
    }
    let held = if fresh.is_some() && age < READING_CURRENT {
        None
    } else {
        let why = match (reason, gap) {
            (Some(reason), _) => Why::Reader(reason),
            (None, Some(gap)) => Why::Gap(gap),
            (None, None) => Why::NoCycle,
        };
        Some((age.as_secs(), why))
    };
    Some(Taken { value, read: fresh.is_some(), held })
}

/// Why the addon's reader has nothing of this second, for the tooltip of a line it feeds.
fn no_capture(gap: Gap, english: bool) -> String {
    let (es, en) = match gap {
        Gap::CaptureFailed => ("Lectura del addon: la última captura falló", "Addon reading: the last capture failed"),
        Gap::NoCharacter => ("Lectura del addon: no hay personaje en un mapa", "Addon reading: no character in a map"),
    };
    tr(english, es, en).to_string()
}

/// The same for the status, whose tooltip already names the inventory source above it.
fn no_capture_status(gap: Gap, english: bool) -> String {
    let (es, en) = match gap {
        Gap::CaptureFailed => ("La última captura falló", "The last capture failed"),
        Gap::NoCharacter => (
            "No hay personaje en un mapa (selección de personaje o pantalla de carga)",
            "No character in a map (character select or loading screen)",
        ),
    };
    tr(english, es, en).to_string()
}

/// The tooltip lines of a reading that is being held: how old it is and why there is no new one.
fn held_lines(held: Option<(u64, Why)>, english: bool) -> Vec<String> {
    let Some((age, why)) = held else { return Vec::new() };
    let mut lines = vec![if english { format!("Last reading {age} s ago") } else { format!("Última lectura hace {age} s") }];
    match why {
        Why::Reader(reason) => lines.push(no_coverage(reason, english)),
        Why::Gap(gap) => lines.push(no_capture(gap, english)),
        Why::NoCycle => {}
    }
    lines
}

/// What the two lines take from the addon's reader in this frame.
struct Readings {
    source: Source,
    bags: Option<Taken<BagSlots>>,
    magic_find: Option<Taken<MagicFind>>,
    /// There is no capture of this second, and the last one that worked is no older than
    /// [`READING_HOLD`]: one failed cycle or a short loading screen, which the status does not
    /// change colour for.
    capture_failed_briefly: bool,
    /// The wallet was read well no more than [`READING_HOLD`] ago: if it has no coverage now,
    /// that is one failed read of it, not a wallet without coverage.
    wallet_failed_briefly: bool,
}

/// Moves the memory one frame on: whose session it is, what is held, and the session's highest
/// Magic Find. Everything that changes the memory about the readings is here, so that a frame
/// that paints and a frame that only observes leave it the same.
fn advance(input: &PanelInput<'_>, memory: &mut PanelMemory) -> Readings {
    let reading = input.farming.reading.as_ref();
    let phase = reading.map(|reading| reading.phase);
    let running = matches!(phase, Some(Phase::Starting | Phase::Active | Phase::Stopping | Phase::Provisional));
    let elapsed = reading.and_then(|reading| reading.elapsed);
    // `farm1` has no session id. Another session shows as one that starts running, or as a
    // declared duration that goes back: the host sends a frame every 5 s, so a short `starting`
    // can pass unseen and `complete` can be followed by `active`, or `active` by `active`.
    let restarted = matches!(
        (memory.elapsed, elapsed),
        (Some(before), Some(now)) if now < before && (before - now > SESSION_RESTART || now < SESSION_RESTART)
    );
    let another = (running && !memory.session_running) || restarted;
    memory.session_running = running;
    memory.elapsed = elapsed;
    // Another character, as another session: nothing read of the one before is this one's. A
    // context that names nobody (character select, or no link yet) is not a change, so the same
    // character coming back keeps what it had.
    let another_character = matches!((memory.character.as_deref(), input.character), (Some(before), Some(now)) if before != now);
    if let Some(name) = input.character.filter(|name| memory.character.as_deref() != Some(*name)) {
        memory.character = Some(name.to_string());
    }
    if another_character {
        memory.character_since = Some(input.now);
    }
    if another || another_character {
        memory.bags = None;
        memory.magic_find = None;
    }
    // No highest without a session that has started measuring, nor across a lost connection:
    // what happened while the addon was not being told is not this session's as far as it knows.
    if another || another_character || input.connection != Status::Connected || matches!(phase, None | Some(Phase::Idle | Phase::Starting)) {
        memory.magic_find_peak = None;
        memory.peak_from = None;
    }
    // The highest counts from the first frame in which this session is seen measuring.
    let measuring = matches!(phase, Some(Phase::Active | Phase::Stopping | Phase::Provisional));
    if measuring && memory.peak_from.is_none() {
        memory.peak_from = Some(input.now);
    }
    let source = source(input);
    // When the reader last captured well, by the instant of that capture. A reader that has
    // stopped has nothing to hold on to, and neither has a connection that never captured.
    // The same for the wallet, which is read in the same capture and can fail on its own.
    match source {
        Source::Live { gap: None } => {
            memory.good_capture = input.read_at.or(memory.good_capture);
            if matches!(input.wallet, WalletCoverage::Listed(_)) {
                memory.good_wallet = input.read_at.or(memory.good_wallet);
            }
        }
        Source::Live { gap: Some(_) } => {}
        Source::Stopped => {
            memory.good_capture = None;
            memory.good_wallet = None;
        }
    }
    let recent = |at: Option<Instant>| at.is_some_and(|at| input.now.saturating_duration_since(at) <= READING_HOLD);
    let capture_failed_briefly = matches!(source, Source::Live { gap: Some(_) }) && recent(memory.good_capture);
    let wallet_failed_briefly = recent(memory.good_wallet);
    let (bags, bags_reason) = match input.bags {
        BagCoverage::Read(slots) => (Some(slots), None),
        BagCoverage::Unavailable(reason) => (None, Some(reason)),
        BagCoverage::NotRead => (None, None),
    };
    let (magic_find, magic_find_reason) = match input.magic_find {
        MagicFindCoverage::Read(read) => (Some(read), None),
        MagicFindCoverage::Unavailable(reason) => (None, Some(reason)),
        MagicFindCoverage::NotRead => (None, None),
    };
    // A cycle that ran before this character was the one in the game read the one before: its
    // output is still there until the next cycle, and is not a reading of this one.
    let since = |from: Option<Instant>| input.read_at.is_some_and(|at| from.is_none_or(|from| at >= from));
    let own = since(memory.character_since);
    let bags = take(&mut memory.bags, source, bags.filter(|_| own), bags_reason, input.read_at, input.now);
    let magic_find = take(&mut memory.magic_find, source, magic_find.filter(|_| own), magic_find_reason, input.read_at, input.now);
    // Only a reading the reader has just returned, while the session measures, can be its
    // highest; and the highest only goes up. A held reading is compared with it and never
    // moves it, and neither does one read after the session is complete, nor one whose cycle
    // ran before this session was measuring: `read` only says the reader's output is a figure.
    if let Some(Taken { value, read: true, .. }) = magic_find {
        let of_this_session = memory.peak_from.is_some() && since(memory.peak_from);
        if measuring && of_this_session && memory.magic_find_peak.is_none_or(|peak| value.total > peak.total) {
            memory.magic_find_peak = Some(value);
        }
    }
    Readings { source, bags, magic_find, capture_failed_briefly, wallet_failed_briefly }
}

/// Keeps `memory` up to date in a frame that paints nothing, which is every frame while the
/// panel is closed. Without it a session that ends and another that starts in the meantime
/// would be one session to the memory, and the first one's highest Magic Find would make the
/// second one's look like a fall.
pub fn observe(input: &PanelInput<'_>, memory: &mut PanelMemory) {
    advance(input, memory);
}

/// Everything the panel is painted from.
#[derive(Debug, Clone)]
pub struct PanelInput<'a> {
    /// The frame's instant on a monotonic clock, for [`READING_HOLD`]. The caller owns the
    /// clock; nothing here reads one.
    pub now: Instant,
    /// When the reader ran the cycle that `bags` and `magic_find` come from, on the same clock
    /// (`SharedState::inventory_reading`); `None` before the first cycle. The readings are as
    /// old as this says, however long the diagnostics have stayed the same.
    pub read_at: Option<Instant>,
    pub connection: Status,
    pub farming: &'a FarmingView,
    pub price: &'a PriceView,
    pub live: LiveStatus,
    /// A character is in a map: the game context the client reports is `gameplay`
    /// (`SharedState::character_in_map`, from `NexusLink` and the Mumble Link, not from the
    /// memory readers). Without one the source says `Unavailable` because there is nothing to
    /// capture, which is not a capture that failed.
    pub character_in_map: bool,
    /// The character the same game context names, `None` when it names none (character select,
    /// or nothing read from the link yet). Only compared with the one before: a different one
    /// starts the session's highest Magic Find over, and nothing read of the other is kept.
    pub character: Option<&'a str>,
    pub wallet: WalletCoverage,
    /// What the addon's own reader says about the bags in its last cycle
    /// (`inventory::Diagnostics::bags`). Only `Read` is a verified figure; without it the line
    /// falls back to the plugin's `slots`.
    pub bags: BagCoverage,
    /// The same for Magic Find (`inventory::Diagnostics::magic_find`); without a verified
    /// figure the line falls back to the plugin's `mf`, marked as not live.
    pub magic_find: MagicFindCoverage,
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

const RATE_UNIT: &str = "b/h";

/// The rate. A band is shown as `lo–hi`, or as `~mean` while it is wide (see [`averaged`]),
/// with the band itself in the tooltip. A band one unit wide is one number rounded down and
/// up, which is what a live session sends, and is shown as that number. Notes about the rate
/// ([`FarmingView::rate_notes`]) go to the tooltip and paint it in the warning tone.
fn rate_cell(view: &FarmingView, memory: &mut PanelMemory, english: bool) -> Cell {
    let mut tooltip = vec![tr(english, "Bolsas por hora", "Bags per hour").to_string()];
    let Some(reading) = view.reading.as_ref() else {
        memory.rate_averaged = false;
        tooltip.push(tr(english, "Sin lectura", "No reading").to_string());
        return cell(format!("{NO_DATA} {RATE_UNIT}"), Tone::Muted, tooltip);
    };
    let figure = match (reading.lo, reading.hi) {
        (Some(lo), Some(hi)) if i64::from(hi) - i64::from(lo) > 1 => {
            memory.rate_averaged = averaged(lo, hi, memory.rate_averaged);
            if memory.rate_averaged {
                let mean = (i64::from(lo) + i64::from(hi) + 1) / 2;
                tooltip.push(format!("{}: {lo}–{hi} {RATE_UNIT}", tr(english, "Rango", "Range")));
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
    let notes = view.rate_notes(english);
    let tone = if notes.is_empty() { Tone::Normal } else { Tone::Warning };
    tooltip.extend(notes);
    cell(format!("{figure} {RATE_UNIT}"), tone, tooltip)
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
/// Figures only from a fresh `ok` reading; every other state says `—` and why in the tooltip,
/// and so does one side alone without a quote.
fn stack_cells(input: &PanelInput<'_>, english: bool) -> (Cell, Cell, Cell) {
    let view = input.price;
    let reading = view.reading.as_ref().filter(|_| view.capable && view.fresh);
    let figures = reading.filter(|reading| reading.st == PriceStatus::Ok);
    let idle = view.reading.as_ref().is_some_and(|reading| reading.st == PriceStatus::Idle);
    // Why there is no figure at all, or nothing when the reading has figures.
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
        // The reading has figures and this side has none: say which of the two reasons it is.
        if state.is_none() && stack.is_none() {
            tooltip.push(match unit {
                None => tr(english, "Sin cotización en este lado", "No quote on this side").to_string(),
                Some(_) => tr(english, "El stack no cabe en la trama", "The stack does not fit the frame").to_string(),
            });
        }
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

fn slots_text(free: Option<i32>, english: bool) -> String {
    let label = tr(english, "Huecos", "Slots");
    match free {
        Some(free) => format!("{label}: {free} {}", tr(english, "libres", "free")),
        None => format!("{label}: {NO_DATA}"),
    }
}

/// Why the addon's own reader has no figure, in a few words. The Options window shows the
/// English ones next to the reader's counters.
pub fn uncovered_reason(reason: Uncovered, english: bool) -> &'static str {
    let (es, en) = match reason {
        Uncovered::Guard => ("esta versión del juego no es la auditada", "this game build is not the audited one"),
        Uncovered::Profile => ("estructura no reconocida", "structure not recognised"),
        Uncovered::Root => ("personaje no disponible", "character unavailable"),
        Uncovered::Bounds => ("fuera de los límites de lectura", "outside the read limits"),
        Uncovered::Alignment => ("puntero mal alineado", "misaligned pointer"),
        Uncovered::Integrity => ("datos incoherentes", "inconsistent data"),
        Uncovered::Unsupported => ("un efecto necesita estado en vivo", "an effect needs live state"),
        Uncovered::Changed => ("cambió durante la lectura", "changed while reading"),
        Uncovered::ReadFailed => ("lectura fallida", "read failed"),
        Uncovered::Deadline => ("se acabó el tiempo de lectura", "the read ran out of time"),
    };
    tr(english, es, en)
}

/// The bag reader's last pass for the Options window: what it got, or the exact reason it got
/// nothing, and what it cost. English, like the rest of that window; it is what a screenshot
/// has to show when a reading is missing in the game.
pub fn bags_diagnostic(coverage: BagCoverage, bytes: u32, reads: u32) -> String {
    let outcome = match coverage {
        BagCoverage::NotRead => "not read".to_string(),
        BagCoverage::Read(slots) => format!(
            "read, {} used of {}, {} free ({} bags in {} bag slots)",
            slots.occupied, slots.capacity, slots.free, slots.bags, slots.bag_slots
        ),
        BagCoverage::Unavailable(reason) => format!("no coverage, {reason:?} ({})", uncovered_reason(reason, true)),
    };
    format!("Bags: {outcome}; bytes: {bytes} / {}; reads: {reads}", crate::bags::MAX_BYTES)
}

/// The same for the Magic Find reader, with its three addends and whether the boon-only
/// modifiers counted.
pub fn magic_find_diagnostic(coverage: MagicFindCoverage, bytes: u32, reads: u32) -> String {
    let outcome = match coverage {
        MagicFindCoverage::NotRead => "not read".to_string(),
        MagicFindCoverage::Read(read) => format!(
            "read, {}% = luck {} + server {} + effects {} (boon modifiers counted: {})",
            points(read.total),
            read.luck,
            points(read.pushed),
            points(read.buffs),
            if read.boon { "yes" } else { "no" }
        ),
        MagicFindCoverage::Unavailable(reason) => format!("no coverage, {reason:?} ({})", uncovered_reason(reason, true)),
    };
    format!("Magic Find: {outcome}; bytes: {bytes} / {}; reads: {reads}", crate::magic_find::MAX_BYTES)
}

/// The tooltip line of a cell whose figure is not the addon's because its reader had none.
/// Not having coverage is not the player's doing: it is said, and nothing turns red for it.
fn no_coverage(reason: Uncovered, english: bool) -> String {
    format!("{} ({})", tr(english, "Lectura del addon: sin cobertura", "Addon reading: no coverage"), uncovered_reason(reason, english))
}

/// Why a line's figure is not the addon's own, for the branches that paint the plugin's or `—`:
/// the reader is stopped, it said why it has none, the whole capture failed, or there is no
/// character in a map. `None` when the reader simply has not produced one.
fn fallback(source: Source, reason: Option<Uncovered>, english: bool) -> Option<String> {
    match (source, reason) {
        (Source::Stopped, _) => Some(not_sampling(english)),
        (_, Some(reason)) => Some(no_coverage(reason, english)),
        (Source::Live { gap: Some(gap) }, None) => Some(no_capture(gap, english)),
        (Source::Live { gap: None }, None) => None,
    }
}

fn not_sampling(english: bool) -> String {
    tr(english, "Lectura del addon: sin muestreo en curso", "Addon reading: not sampling now").to_string()
}

/// Free bag slots: the addon's own verified reading if there is one, else what the plugin
/// sends, else `—`. The warning tone at [`SLOTS_WARNING`] or fewer and the error tone at
/// [`SLOTS_ERROR`] or fewer. The verified figure names the counter of the inventory window in
/// its tooltip (used of total); the plugin's says whose it is and how old, and is also in the
/// warning tone when it is old. A verified figure is held for [`READING_HOLD`] through cycles
/// that return none: the cell stays as it was and the tooltip says how old the reading is.
fn slots_cell(input: &PanelInput<'_>, source: Source, taken: Option<Taken<BagSlots>>, english: bool) -> Cell {
    let view = input.farming;
    let line = |es: &'static str, en: &'static str| tr(english, es, en).to_string();
    let mut tooltip = vec![line("Huecos libres en las bolsas del personaje", "Free bag slots of the character")];
    let mut old = false;
    // Why the figure is not the addon's, for the two branches that do not paint it.
    let reason = match input.bags {
        BagCoverage::Unavailable(reason) => Some(reason),
        _ => None,
    };
    let uncovered = fallback(source, reason, english);
    let free = if let Some(Taken { value: slots, held, .. }) = taken {
        tooltip.push(line("Leído y verificado por el addon", "Read and verified by the addon"));
        tooltip.push(if english {
            format!("Inventory: {} used of {}", slots.occupied, slots.capacity)
        } else {
            format!("Inventario: {} usados de {}", slots.occupied, slots.capacity)
        });
        tooltip.push(if english {
            format!("Bags: {} in {} bag slots", slots.bags, slots.bag_slots)
        } else {
            format!("Bolsas: {} en {} ranuras", slots.bags, slots.bag_slots)
        });
        tooltip.extend(held_lines(held, english));
        i32::try_from(slots.free).unwrap_or(i32::MAX)
    } else if let Some((reading, free)) = view.reading.as_ref().and_then(|reading| reading.slots.map(|free| (reading, free))) {
        tooltip.push(line("Dato del plugin", "From the plugin"));
        if reading.slot_source == SlotSource::Recent {
            tooltip.push(line("Personaje reciente", "Recent character"));
        }
        tooltip.push(match view.slot_age {
            Some(age) => format!("{} {age}s", tr(english, "Lectura de huecos hace", "Slot reading ago:")),
            None => line("Sin lectura de huecos", "No slot reading"),
        });
        if !view.fresh {
            tooltip.push(line("Datos antiguos · última lectura", "Stale data · last reading"));
        }
        tooltip.extend(uncovered);
        old = !view.fresh || view.show_reading_age();
        free
    } else {
        tooltip.push(line("Sin lectura de huecos", "No slot reading"));
        tooltip.extend(uncovered);
        return cell(slots_text(None, english), Tone::Muted, tooltip);
    };
    let tone = if free <= SLOTS_ERROR {
        Tone::Error
    } else if free <= SLOTS_WARNING || old {
        Tone::Warning
    } else {
        Tone::Normal
    };
    if free <= SLOTS_WARNING {
        tooltip.push(line("Quedan pocos huecos", "Few slots left"));
    }
    cell(slots_text(Some(free), english), tone, tooltip)
}

/// How a Magic Find is written: a verified one bare, the plugin's with a word after it that
/// says it is not a live reading, and `—` without one.
fn magic_find_text(figure: Option<&str>, from_plugin: bool, english: bool) -> String {
    match figure {
        Some(figure) if from_plugin => format!("MF: {figure}% {}", tr(english, "parcial", "partial")),
        Some(figure) => format!("MF: {figure}%"),
        None => format!("MF: {NO_DATA}"),
    }
}

/// Percentage points as the panel writes them: whole when they are, else with one decimal.
///
/// Rounded to tenths with a cast, on purpose: `f64::round` makes the Windows DLL import a
/// rounding function from the C runtime, and its import table is meant not to grow.
pub fn points(value: f32) -> String {
    let scaled = f64::from(value) * 10.0;
    let tenths = if scaled >= 0.0 { (scaled + 0.5) as i64 } else { (scaled - 0.5) as i64 };
    // `unsigned_abs`: the cast saturates, and the lowest i64 has no positive of its own.
    let (sign, tenths) = (if tenths < 0 { "-" } else { "" }, tenths.unsigned_abs());
    if tenths % 10 == 0 {
        format!("{sign}{}", tenths / 10)
    } else {
        format!("{sign}{}.{}", tenths / 10, tenths % 10)
    }
}

/// Two amounts of percentage points differ by something the panel would write.
fn lower(now: f32, before: f32) -> bool {
    before - now >= 0.05
}

/// Magic Find.
///
/// - Read and verified by the addon: the bare figure, its three addends in the tooltip (luck,
///   what the server pushed, the applied effects), and the warning tone while it is below the
///   highest total of the session, with how much it fell and which addends. The reader's
///   `boon` flag is not an addend: what it lets in is already inside the other two.
/// - From the plugin (`mf` with `mfKind: partial`): a value declared when the session started,
///   or a partial one. It does not follow the game, so it is written `MF: 333% parcial`, in the
///   muted tone, never warns about a drop and never feeds the session's peak.
/// - Neither: `—`.
///
/// When the reader tried and has no coverage, the tooltip says why. What the old "optional
/// preparation" block said is at the end of the tooltip.
fn magic_find_cell(input: &PanelInput<'_>, source: Source, taken: Option<Taken<MagicFind>>, peak: Option<MagicFind>, english: bool) -> Cell {
    let view = input.farming;
    let line = |es: &'static str, en: &'static str| tr(english, es, en).to_string();
    let mut tooltip = vec![line("Hallazgo mágico", "Magic Find")];
    // Why the figure is not the addon's own: the reader's reason, or that there is no reader
    // output at all.
    let reason = match input.magic_find {
        MagicFindCoverage::Unavailable(reason) => Some(reason),
        _ => None,
    };
    let verified = taken.is_some();
    let uncovered = fallback(source, reason, english)
        .unwrap_or_else(|| line("Hallazgo mágico verificado: sin cobertura", "Verified Magic Find: no coverage"));
    let plugin = view.reading.as_ref().filter(|reading| reading.mf_kind == MagicFindKind::Partial).and_then(|reading| reading.mf);
    let (text, tone) = if let Some(Taken { value: read, held, .. }) = taken {
        tooltip.push(line("Leído y verificado por el addon", "Read and verified by the addon"));
        let addends: [(&str, f32, fn(&MagicFind) -> f32); 3] = [
            (tr(english, "Suerte", "Luck"), read.luck as f32, |value| value.luck as f32),
            (tr(english, "Servidor", "Server"), read.pushed, |value| value.pushed),
            (tr(english, "Efectos", "Effects"), read.buffs, |value| value.buffs),
        ];
        for (name, value, _) in &addends {
            tooltip.push(format!("{name}: {}%", points(*value)));
        }
        // The session's highest is kept by `advance`; here it is only compared with.
        let peak = peak.as_ref().filter(|peak| lower(read.total, peak.total));
        if let Some(peak) = peak {
            let fall = points(peak.total - read.total);
            tooltip.push(if english {
                format!("Down {fall} points from the session peak ({}%)", points(peak.total))
            } else {
                format!("Bajó {fall} puntos respecto al máximo de la sesión ({}%)", points(peak.total))
            });
            for (name, now, of) in &addends {
                let before = of(peak);
                if lower(*now, before) {
                    tooltip.push(if english {
                        format!("{name}: from {}% to {}%", points(before), points(*now))
                    } else {
                        format!("{name}: de {}% a {}%", points(before), points(*now))
                    });
                }
            }
        }
        let tone = if peak.is_some() { Tone::Warning } else { Tone::Normal };
        tooltip.extend(held_lines(held, english));
        (magic_find_text(Some(&points(read.total)), false, english), tone)
    } else if let Some(value) = plugin {
        tooltip.push(line(
            "Dato del plugin: declarado al empezar la sesión, o parcial. No es una lectura en vivo.",
            "From the plugin: declared when the session started, or partial. Not a live reading.",
        ));
        tooltip.push(uncovered);
        (magic_find_text(Some(&value.to_string()), true, english), Tone::Muted)
    } else {
        tooltip.push(uncovered);
        (magic_find_text(None, false, english), Tone::Muted)
    };
    if let Some(reading) = view.reading.as_ref() {
        tooltip.push(match reading.prep {
            Preparation::Attention => line("Preparación: revisar en el host", "Preparation: check in the host"),
            Preparation::Partial => line("Preparación parcial", "Partial preparation"),
            Preparation::Unknown => line("Preparación desconocida", "Preparation unknown"),
        });
        // The host cannot see the effects on the character; the addon's own reading counts them.
        if !verified {
            tooltip.push(line(
                "Buffs temporales sin verificar. Recordatorios manuales en el host.",
                "Temporary buffs unverified. Manual reminders in the host.",
            ));
        }
    }
    cell(text, tone, tooltip)
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
/// the tooltip and do not change the colour: they come and go in normal measurement. So does
/// one capture that fails, and one read of the wallet that fails: `readings` says whether the
/// reader captured well, and listed the wallet, no more than [`READING_HOLD`] ago, and until
/// that runs out the dot and the text stay as they were and only the tooltip says what failed.
/// A reader that has stopped changes them at once.
/// The inventory status and the wallet coverage are in the tooltip in every branch, and so is
/// an error the host had reported before the connection was lost.
fn status_cell(input: &PanelInput<'_>, readings: &Readings, english: bool) -> (Tone, Cell) {
    let (capture_failed_briefly, wallet_failed_briefly) = (readings.capture_failed_briefly, readings.wallet_failed_briefly);
    let view = input.farming;
    let mut tooltip = Vec::new();
    let line = |es: &'static str, en: &'static str| tr(english, es, en).to_string();
    let source = |tooltip: &mut Vec<String>| {
        tooltip.push(inventory_status(input.live, english).to_string());
        tooltip.push(wallet_status(input.live, input.wallet, english));
    };
    let check = || line("Revisa la sesión en Hebra u Obsidian", "Check the session in Hebra or Obsidian");
    if let Some((text, tone, detail)) = connection_text(input.connection, english) {
        tooltip.push(line("Conexión al host: sin conexión", "Host connection: offline"));
        tooltip.push(detail.to_string());
        if let Some(reading) = view.reading.as_ref() {
            tooltip.push(line("Datos antiguos · última lectura", "Stale data · last reading"));
            if let Some(error) = reading.err {
                tooltip.push(format!("{}: {}", tr(english, "Error pendiente", "Pending error"), farming_error_label(error, english)));
                tooltip.push(check());
            }
        }
        source(&mut tooltip);
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
        source(&mut tooltip);
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
    if measuring && source_down && !capture_failed_briefly {
        tone = Tone::Warning;
    }
    source(&mut tooltip);
    // Why the source has no capture now: one that failed, or no character in a map to read.
    if let Source::Live { gap: Some(gap) } = readings.source {
        tooltip.push(no_capture_status(gap, english));
    }
    // A wallet that has no coverage is a lasting problem; one read of it that fails, after one
    // that worked, is not, and says so only in the tooltip line above.
    if input.live.is_sampling() && matches!(input.wallet, WalletCoverage::Unavailable(_)) && measuring && tone == Tone::Good && !wallet_failed_briefly {
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
        tooltip.insert(1, check());
        tone = Tone::Error;
    }
    let text_tone = if matches!(tone, Tone::Warning | Tone::Error) { tone } else { Tone::Normal };
    (tone, cell(text, text_tone, tooltip))
}

/// The panel for this frame. `memory` carries the rate's range-or-average choice and the
/// session's verified Magic Find peak from the previous one.
pub fn view(input: &PanelInput<'_>, memory: &mut PanelMemory, english: bool) -> PanelView {
    let readings = advance(input, memory);
    let (stack_label, buy, sell) = stack_cells(input, english);
    let (status_dot, status) = status_cell(input, &readings, english);
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
        slots: slots_cell(input, readings.source, readings.bags, english),
        magic_find: magic_find_cell(input, readings.source, readings.magic_find, memory.magic_find_peak, english),
        status_label: tr(english, "Estado:", "Status:").to_string(),
        status_dot,
        status,
    }
}

/// The widest text each part of the panel is expected to hold, for the addon to reserve its
/// width once instead of following the content: a rate that turns from a range into an average,
/// or a status that changes, then moves nothing.
///
/// Every shape a cell can take is here once, with its figures at their limit and written with
/// 9s: a text of that cell is no wider than its sample if it is the sample with characters
/// taken out and digits changed. The addon measures the samples with the widest digit of the
/// host's font in place of the 9s, so that holds whatever the font. Beyond the limits (10 000
/// bags an hour, a stack of 100 000 g) the window widens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidthSamples {
    /// Left column, at the normal size: the label and the rate.
    pub left: Vec<String>,
    /// Left column, at the large size: the observed bags.
    pub left_large: Vec<String>,
    /// Right column: the label and a price.
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
        left: vec![
            tr(english, "bolsas", "bags").to_string(),
            format!("9999–9999 {RATE_UNIT}"),
            format!("~9999 {RATE_UNIT}"),
            format!("≥9999 {RATE_UNIT}"),
            format!("{NO_DATA} {RATE_UNIT}"),
        ],
        left_large: vec!["9999".to_string(), NO_DATA.to_string()],
        right: vec!["stack".to_string(), "99999g 99s 99c".to_string(), NO_DATA.to_string()],
        lines: vec![
            slots_text(Some(9999), english),
            slots_text(None, english),
            magic_find_text(Some("9999.9"), false, english),
            magic_find_text(Some("9999"), true, english),
            magic_find_text(None, false, english),
        ],
        status,
    }
}

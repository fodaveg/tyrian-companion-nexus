//! The Labyrinth panel as data: every cell in every state, the rate's range-or-average choice,
//! where slots and Magic Find come from, and the texts at their limits. The addon paints
//! exactly what `panel::view` returns.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tyrian_companion_nexus_core::bags::{BagCoverage, BagSlots};
use tyrian_companion_nexus_core::farming::FarmingView;
use tyrian_companion_nexus_core::live::LiveStatus;
use tyrian_companion_nexus_core::magic_find::{MagicFind, MagicFindCoverage};
use tyrian_companion_nexus_core::panel::{self, Cell, PanelInput, PanelMemory, PanelView, Tone, NO_DATA};
use tyrian_companion_nexus_core::passive::Uncovered;
use tyrian_companion_nexus_core::price::PriceView;
use tyrian_companion_nexus_core::protocol::{parse_server_line, ServerLine};
use tyrian_companion_nexus_core::state::{SharedState, Status};
use tyrian_companion_nexus_core::wallet::{WalletCoverage, WalletError};

const NONCE: &str = "Zk3m1Qw9Lr0aT7yUc2Vb5g";

/// A live session as the plugin sends it today: no slots (the reader sends none) and no
/// Magic Find (the session was started without one).
fn farming_frame() -> Value {
    json!({ "v":3, "type":"farming_state", "tag":"farm1", "nonce":NONCE, "seq":1, "ttl":15,
        "phase":"active", "err":null, "elapsed":1716, "observed":143, "net":null,
        "lo":37, "hi":37, "age":0, "slots":null, "slotSrc":"unknown", "slotAge":null,
        "goal":"none", "target":null, "progress":null, "eta":null, "mf":null,
        "mfKind":"unknown", "prep":"unknown" })
}

/// The same session with the slots and the Magic Find of David's sketch in the frame.
fn sketch_frame() -> Value {
    let mut frame = farming_frame();
    frame["slots"] = json!(63);
    frame["slotSrc"] = json!("ingame");
    frame["slotAge"] = json!(2);
    frame["mf"] = json!(333);
    frame["mfKind"] = json!("partial");
    frame["prep"] = json!("partial");
    frame
}

fn price_frame(st: &str) -> Value {
    let figures = st == "ok";
    json!({ "v":3, "type":"price_state", "tag":"price2", "nonce":NONCE, "seq":1, "ttl":15, "st":st,
        "sell": if figures { json!(283) } else { Value::Null }, "sellStack": if figures { json!(70762) } else { Value::Null },
        "list": if figures { json!(356) } else { Value::Null }, "listStack": if figures { json!(89037) } else { Value::Null },
        "age": if st == "pending" || st == "idle" { Value::Null } else { json!(1260) } })
}

/// A farming view `local` after `frame` arrived, or one with no reading at all.
fn farming(frame: Option<&Value>, local: Duration) -> FarmingView {
    let state = SharedState::new();
    let now = Instant::now();
    state.begin_farming_connection(NONCE);
    state.enable_farming(NONCE);
    if let Some(frame) = frame {
        let ServerLine::FarmingState(reading) = parse_server_line(&frame.to_string()) else { panic!("{frame}") };
        assert!(state.accept_farming(reading, now));
    }
    state.farming_view(now + local)
}

/// A price view `local` after `frame` arrived; `None` is a capability with no frame yet.
fn price(frame: Option<&Value>, local: Duration) -> PriceView {
    let state = SharedState::new();
    let now = Instant::now();
    state.begin_price_connection(NONCE);
    state.enable_price(NONCE);
    if let Some(frame) = frame {
        let ServerLine::PriceState(reading) = parse_server_line(&frame.to_string()) else { panic!("{frame}") };
        assert!(state.accept_price(reading, now));
    }
    state.price_view(now + local)
}

fn no_price() -> PriceView {
    SharedState::new().price_view(Instant::now())
}

/// A Magic Find the addon's reader read: luck, what the server pushed, and the effects.
fn verified(luck: u32, server: f32, effects: f32) -> MagicFindCoverage {
    MagicFindCoverage::Read(MagicFind { total: luck as f32 + server + effects, luck, pushed: server, buffs: effects, boon: false })
}

/// Bags the addon's reader read: `free` of `capacity`, in eight bags.
fn bags(free: u32, capacity: u32) -> BagCoverage {
    BagCoverage::Read(BagSlots { capacity, occupied: capacity - free, free, bag_slots: 8, bags: 8 })
}

const REASONS: [Uncovered; 10] = [
    Uncovered::Guard,
    Uncovered::Profile,
    Uncovered::Root,
    Uncovered::Bounds,
    Uncovered::Alignment,
    Uncovered::Integrity,
    Uncovered::Unsupported,
    Uncovered::Changed,
    Uncovered::ReadFailed,
    Uncovered::Deadline,
];

struct Case {
    connection: Status,
    farming: FarmingView,
    price: PriceView,
    live: LiveStatus,
    wallet: WalletCoverage,
    bags: BagCoverage,
    magic_find: MagicFindCoverage,
}

impl Case {
    /// Connected, measuring, a fresh price, and the frame of a live session of today.
    fn measuring() -> Self {
        Self {
            connection: Status::Connected,
            farming: farming(Some(&farming_frame()), Duration::ZERO),
            price: price(Some(&price_frame("ok")), Duration::ZERO),
            live: LiveStatus::Measuring,
            wallet: WalletCoverage::Listed(55),
            bags: BagCoverage::NotRead,
            magic_find: MagicFindCoverage::NotRead,
        }
    }

    /// The same with the sketch's slots and Magic Find in the plugin's frame.
    fn sketch() -> Self {
        Self::measuring().with_frame(sketch_frame(), |_| {})
    }

    fn with_frame(mut self, mut frame: Value, change: impl FnOnce(&mut Value)) -> Self {
        change(&mut frame);
        self.farming = farming(Some(&frame), Duration::ZERO);
        self
    }

    fn with_farming(self, change: impl FnOnce(&mut Value)) -> Self {
        self.with_frame(farming_frame(), change)
    }

    fn view_with(&self, memory: &mut PanelMemory, english: bool) -> PanelView {
        panel::view(
            &PanelInput {
                connection: self.connection,
                farming: &self.farming,
                price: &self.price,
                live: self.live,
                wallet: self.wallet,
                bags: self.bags,
                magic_find: self.magic_find,
            },
            memory,
            english,
        )
    }

    fn view(&self, english: bool) -> PanelView {
        self.view_with(&mut PanelMemory::default(), english)
    }
}

fn has(cell: &Cell, text: &str) -> bool {
    cell.tooltip.iter().any(|line| line.contains(text))
}

#[test]
fn the_sketch_itself() {
    let view = Case::sketch().view(false);
    assert_eq!((view.bags_label.text.as_str(), view.bags.text.as_str(), view.rate.text.as_str()), ("bolsas", "143", "37 b/h"));
    assert_eq!((view.stack_label.text.as_str(), view.buy.text.as_str(), view.sell.text.as_str()), ("stack", "7g 7s 62c", "8g 90s 37c"));
    assert_eq!(view.slots.text, "Huecos: 63 libres");
    // The plugin's Magic Find is not a live reading, and says so in its own text.
    assert_eq!(view.magic_find.text, "MF: 333% parcial");
    assert_eq!((view.status_label.as_str(), view.status_dot, view.status.text.as_str()), ("Estado:", Tone::Good, "Midiendo"));
    for cell in view.cells() {
        let expected = if std::ptr::eq(cell, &view.magic_find) { Tone::Muted } else { Tone::Normal };
        assert_eq!(cell.tone, expected, "{cell:?}");
    }
    let english = Case::sketch().view(true);
    assert_eq!((english.bags_label.text.as_str(), english.stack_label.text.as_str()), ("bags", "stack"));
    assert_eq!((english.slots.text.as_str(), english.magic_find.text.as_str()), ("Slots: 63 free", "MF: 333% partial"));
    assert_eq!((english.status_label.as_str(), english.status.text.as_str()), ("Status:", "Measuring"));
    // With a verified reading the line is the sketch's, bare.
    let mut read = Case::sketch();
    read.magic_find = verified(300, 0.0, 33.0);
    assert_eq!(read.view(false).magic_find.text, "MF: 333%");
    // The corn is not in the panel, and neither is any net-of-fee wording.
    for view in [&view, &english] {
        for cell in view.cells() {
            for text in cell.tooltip.iter().chain([&cell.text]) {
                let lower = text.to_lowercase();
                assert!(!lower.contains("maíz") && !lower.contains("corn") && !lower.contains("neto de") && !lower.contains("net of"), "{text}");
            }
        }
    }
}

#[test]
fn slots_come_from_the_plugin_frame_and_say_whose_and_how_old() {
    let view = Case::sketch().view(false);
    assert_eq!((view.slots.text.as_str(), view.slots.tone), ("Huecos: 63 libres", Tone::Normal));
    assert_eq!(view.slots.tooltip, ["Huecos libres en las bolsas del personaje", "Dato del plugin", "Lectura de huecos hace 2s"]);
    assert_eq!(Case::sketch().view(true).slots.tooltip, ["Free bag slots of the character", "From the plugin", "Slot reading ago: 2s"]);
    // The slots of the most recent character, and a reading without an age.
    let recent = Case::measuring().with_frame(sketch_frame(), |frame| { frame["slotSrc"] = json!("recent"); frame["slotAge"] = Value::Null; }).view(false).slots;
    assert_eq!(recent.text, "Huecos: 63 libres");
    assert!(has(&recent, "Personaje reciente") && has(&recent, "Sin lectura de huecos"), "{recent:?}");
    // An old reading in a session under way: the figure stays, in the warning tone.
    let old = Case::measuring().with_frame(sketch_frame(), |frame| { frame["age"] = json!(20); frame["slotAge"] = json!(20); }).view(false).slots;
    assert_eq!((old.text.as_str(), old.tone), ("Huecos: 63 libres", Tone::Warning));
    assert!(has(&old, "Lectura de huecos hace 20s"));
    let mut stale = Case::sketch();
    stale.farming = farming(Some(&sketch_frame()), Duration::from_secs(15));
    let stale = stale.view(false).slots;
    assert_eq!(stale.tone, Tone::Warning);
    assert!(has(&stale, "Datos antiguos") && has(&stale, "Lectura de huecos hace 17s"), "{stale:?}");
    // No figure in the frame, which is what a live session sends today: a dash.
    for english in [false, true] {
        let none = Case::measuring().view(english).slots;
        assert_eq!((none.text.as_str(), none.tone), (if english { "Slots: —" } else { "Huecos: —" }, Tone::Muted));
        assert!(has(&none, if english { "No slot reading" } else { "Sin lectura de huecos" }));
    }
    let mut offline = Case::measuring();
    offline.farming = farming(None, Duration::ZERO);
    assert_eq!(offline.view(false).slots.text, "Huecos: —");
}

#[test]
fn slots_warn_at_ten_and_turn_to_error_at_three_whatever_their_source() {
    for (slots, tone) in [(250, Tone::Normal), (11, Tone::Normal), (10, Tone::Warning), (4, Tone::Warning), (3, Tone::Error), (0, Tone::Error)] {
        let plugin = Case::measuring().with_frame(sketch_frame(), |frame| frame["slots"] = json!(slots)).view(false).slots;
        let mut case = Case::measuring();
        case.bags = bags(slots as u32, 250);
        let read = case.view(false).slots;
        for cell in [&plugin, &read] {
            assert_eq!(cell.text, format!("Huecos: {slots} libres"));
            assert_eq!(cell.tone, tone, "{slots}");
            assert_eq!(has(cell, "Quedan pocos huecos"), tone != Tone::Normal, "{slots}");
        }
    }
}

#[test]
fn a_verified_reading_of_the_slots_comes_before_the_plugin_one() {
    let mut case = Case::sketch();
    case.bags = bags(41, 160);
    let view = case.view(false);
    assert_eq!((view.slots.text.as_str(), view.slots.tone), ("Huecos: 41 libres", Tone::Normal));
    // The tooltip names the counter of the inventory window: used of total.
    assert_eq!(
        view.slots.tooltip,
        ["Huecos libres en las bolsas del personaje", "Leído y verificado por el addon", "Inventario: 119 usados de 160", "Bolsas: 8 en 8 ranuras"]
    );
    assert!(!has(&view.slots, "plugin"));
    // It does not depend on the plugin's frame being there, or fresh.
    case.farming = farming(None, Duration::ZERO);
    let view = case.view(true);
    assert_eq!(view.slots.text, "Slots: 41 free");
    assert_eq!(
        view.slots.tooltip,
        ["Free bag slots of the character", "Read and verified by the addon", "Inventory: 119 used of 160", "Bags: 8 in 8 bag slots"]
    );
}

/// The reader tried and has no figure. The line falls back to the plugin's figure, or to a
/// dash, says in the tooltip that the addon has no coverage and why, and never turns red for
/// it: no coverage is not something the player did.
#[test]
fn bags_without_coverage_fall_back_to_the_plugin_or_a_dash_and_say_why() {
    for reason in REASONS {
        for english in [false, true] {
            let why = format!(
                "{} ({})",
                if english { "Addon reading: no coverage" } else { "Lectura del addon: sin cobertura" },
                panel::uncovered_reason(reason, english)
            );
            // With the plugin's figure.
            let mut with_plugin = Case::sketch();
            with_plugin.bags = BagCoverage::Unavailable(reason);
            let cell = with_plugin.view(english).slots;
            assert_eq!((cell.text.as_str(), cell.tone), (if english { "Slots: 63 free" } else { "Huecos: 63 libres" }, Tone::Normal), "{reason:?}");
            assert!(has(&cell, if english { "From the plugin" } else { "Dato del plugin" }) && has(&cell, &why), "{reason:?}: {cell:?}");
            // Without any figure.
            let mut without = Case::measuring();
            without.bags = BagCoverage::Unavailable(reason);
            let cell = without.view(english).slots;
            assert_eq!((cell.text.as_str(), cell.tone), (if english { "Slots: —" } else { "Huecos: —" }, Tone::Muted), "{reason:?}");
            assert!(has(&cell, &why), "{reason:?}: {cell:?}");
        }
    }
    // Every reason has its own short text, in both languages.
    for english in [false, true] {
        let mut texts: Vec<&str> = REASONS.iter().map(|reason| panel::uncovered_reason(*reason, english)).collect();
        assert!(texts.iter().all(|text| !text.is_empty() && text.chars().count() <= 42), "{texts:?}");
        texts.sort_unstable();
        texts.dedup();
        assert_eq!(texts.len(), REASONS.len());
    }
    // Not read at all: as before there was a reader.
    let mut not_read = Case::sketch();
    not_read.bags = BagCoverage::NotRead;
    assert_eq!(not_read.view(false).slots.tooltip, ["Huecos libres en las bolsas del personaje", "Dato del plugin", "Lectura de huecos hace 2s"]);
}

/// The reader's diagnostics keep the last cycle's figures after the cycles stop. A figure of
/// then is not a reading of now, so outside sampling it is not painted as verified.
#[test]
fn a_reading_of_the_addon_only_counts_while_its_reader_is_sampling() {
    for live in [LiveStatus::Waiting, LiveStatus::Measuring, LiveStatus::Partial] {
        let mut case = Case::sketch();
        case.live = live;
        case.bags = bags(41, 160);
        case.magic_find = verified(300, 30.0, 3.0);
        let view = case.view(false);
        assert_eq!((view.slots.text.as_str(), view.magic_find.text.as_str()), ("Huecos: 41 libres", "MF: 333%"), "{live:?}");
    }
    for live in [LiveStatus::NotNegotiated, LiveStatus::UnsupportedBuild, LiveStatus::Unavailable, LiveStatus::Conflict, LiveStatus::StorageUnavailable] {
        let mut case = Case::sketch();
        case.live = live;
        case.bags = bags(41, 160);
        case.magic_find = verified(300, 30.0, 53.0);
        let mut memory = PanelMemory::default();
        let view = case.view_with(&mut memory, false);
        // Back to the plugin's figures, and the tooltip says the reader is not sampling.
        assert_eq!((view.slots.text.as_str(), view.magic_find.text.as_str()), ("Huecos: 63 libres", "MF: 333% parcial"), "{live:?}");
        assert_eq!(view.magic_find.tone, Tone::Muted);
        for cell in [&view.slots, &view.magic_find] {
            assert!(has(cell, "Lectura del addon: sin muestreo en curso") && !has(cell, "verificado por el addon"), "{live:?}: {cell:?}");
        }
        assert_eq!(memory, PanelMemory::default(), "a figure that is not painted is not a peak either");
        assert!(has(&case.view(true).slots, "Addon reading: not sampling now"));
    }
}

/// The plugin's Magic Find is a value declared when the session started, or a partial one. It
/// is written apart, in the muted tone, and never as if it followed the game.
#[test]
fn the_plugin_magic_find_is_marked_as_not_live_and_never_warns() {
    let mut memory = PanelMemory::default();
    let step = |memory: &mut PanelMemory, mf: i32, english: bool| {
        Case::measuring().with_frame(sketch_frame(), |frame| frame["mf"] = json!(mf)).view_with(memory, english).magic_find
    };
    let first = step(&mut memory, 333, false);
    assert_eq!((first.text.as_str(), first.tone), ("MF: 333% parcial", Tone::Muted));
    assert_eq!(
        first.tooltip,
        [
            "Hallazgo mágico",
            "Dato del plugin: declarado al empezar la sesión, o parcial. No es una lectura en vivo.",
            "Hallazgo mágico verificado: sin cobertura",
            "Preparación parcial",
            "Buffs temporales sin verificar. Recordatorios manuales en el host.",
        ]
    );
    // A lower value later is not a drop: there is no peak for a value that is not read.
    let lower = step(&mut memory, 250, false);
    assert_eq!((lower.text.as_str(), lower.tone), ("MF: 250% parcial", Tone::Muted));
    assert!(!has(&lower, "Bajó") && !has(&lower, "máximo"), "{lower:?}");
    assert_eq!(memory, PanelMemory::default(), "it never feeds the session's peak");
    let english = step(&mut memory, 333, true);
    assert_eq!(english.text, "MF: 333% partial");
    assert!(has(&english, "Not a live reading") && has(&english, "Verified Magic Find: no coverage"));
    // Without a value in the frame: a dash, and the same two notes of the old block.
    for english in [false, true] {
        let none = Case::measuring().view(english).magic_find;
        assert_eq!((none.text.as_str(), none.tone), ("MF: —", Tone::Muted));
        assert!(has(&none, if english { "Verified Magic Find: no coverage" } else { "Hallazgo mágico verificado: sin cobertura" }));
        assert!(has(&none, if english { "Preparation unknown" } else { "Preparación desconocida" }));
        assert!(has(&none, if english { "Temporary buffs unverified" } else { "Buffs temporales sin verificar" }));
    }
    let attention = Case::measuring().with_frame(sketch_frame(), |frame| frame["prep"] = json!("attention")).view(false).magic_find;
    assert!(has(&attention, "Preparación: revisar en el host"));
    // `mfKind: unknown` with a number is not a value to show.
    let unknown = Case::measuring().with_frame(sketch_frame(), |frame| frame["mfKind"] = json!("unknown")).view(false).magic_find;
    assert_eq!(unknown.text, "MF: —");
    // No reading at all: no preparation to tell either.
    let mut offline = Case::measuring();
    offline.farming = farming(None, Duration::ZERO);
    assert_eq!(offline.view(false).magic_find.tooltip, ["Hallazgo mágico", "Hallazgo mágico verificado: sin cobertura"]);
}

/// The reading of the addon's own reader: the total, its three addends, and a warning while it
/// is below the highest total of the session, saying how much it fell and which addend.
#[test]
fn a_verified_magic_find_shows_its_parts_and_warns_when_it_falls_from_the_session_peak() {
    let mut memory = PanelMemory::default();
    let mut case = Case::sketch();
    let mut step = |memory: &mut PanelMemory, reading: MagicFindCoverage, english: bool| {
        case.magic_find = reading;
        case.view_with(memory, english).magic_find
    };
    let first = step(&mut memory, verified(300, 30.0, 3.0), false);
    assert_eq!((first.text.as_str(), first.tone), ("MF: 333%", Tone::Normal));
    // The host's "temporary buffs unverified" is not said next to a reading that counts them.
    assert_eq!(
        first.tooltip,
        ["Hallazgo mágico", "Leído y verificado por el addon", "Suerte: 300%", "Servidor: 30%", "Efectos: 3%", "Preparación parcial"]
    );
    assert!(!has(&first, "plugin") && !has(&first, "sin cobertura"), "a verified reading comes before the plugin's 333: {first:?}");
    assert_eq!(step(&mut memory, verified(300, 30.0, 53.0), false).tone, Tone::Normal, "a rise is the new peak");
    let fallen = step(&mut memory, verified(300, 30.0, 3.0), false);
    assert_eq!((fallen.text.as_str(), fallen.tone), ("MF: 333%", Tone::Warning));
    assert!(has(&fallen, "Bajó 50 puntos respecto al máximo de la sesión (383%)"), "{fallen:?}");
    assert!(has(&fallen, "Efectos: de 53% a 3%"));
    assert!(!has(&fallen, "Suerte: de") && !has(&fallen, "Servidor: de"), "only the part that fell: {fallen:?}");
    let english = step(&mut memory, verified(300, 0.0, 3.0), true);
    assert_eq!((english.text.as_str(), english.tone), ("MF: 303%", Tone::Warning));
    assert!(has(&english, "Down 80 points from the session peak (383%)") && has(&english, "Server: from 30% to 0%") && has(&english, "Effects: from 53% to 3%"));
    assert_eq!(english.tooltip[2..5], ["Luck: 300%", "Server: 0%", "Effects: 3%"]);
    assert_eq!(step(&mut memory, verified(300, 30.0, 53.0), false).tone, Tone::Normal, "back at the peak");
    // A reading that goes missing falls back to the plugin's value and does not forget the peak.
    let fallback = step(&mut memory, MagicFindCoverage::NotRead, false);
    assert_eq!((fallback.text.as_str(), fallback.tone), ("MF: 333% parcial", Tone::Muted));
    assert_eq!(step(&mut memory, MagicFindCoverage::Unavailable(Uncovered::Changed), false).text, "MF: 333% parcial");
    assert_eq!(step(&mut memory, verified(300, 30.0, 3.0), false).tone, Tone::Warning);
    // A new session has its own peak.
    let mut starting = Case::measuring().with_frame(sketch_frame(), |frame| frame["phase"] = json!("starting"));
    starting.magic_find = verified(250, 0.0, 0.0);
    assert_eq!(starting.view_with(&mut memory, false).magic_find.tone, Tone::Normal);
    assert_eq!(step(&mut memory, verified(260, 0.0, 0.0), false).tone, Tone::Normal);
    assert_eq!(step(&mut memory, verified(250, 0.0, 0.0), false).tone, Tone::Warning);
    // Fractions are written with one decimal, and a difference too small to write is no drop.
    let mut memory = PanelMemory::default();
    let half = step(&mut memory, verified(300, 12.5, 20.0), false);
    assert_eq!(half.text, "MF: 332.5%");
    assert!(has(&half, "Servidor: 12.5%"));
    assert_eq!(step(&mut memory, verified(300, 12.49, 20.0), false).tone, Tone::Normal);
    let small = step(&mut memory, verified(300, 12.0, 20.0), false);
    assert_eq!((small.text.as_str(), small.tone), ("MF: 332%", Tone::Warning));
    assert!(has(&small, "Bajó 0.5 puntos") && has(&small, "Servidor: de 12.5% a 12%"), "{small:?}");
    // The boon flag is not an addend: it changes nothing the panel writes.
    let mut memory = PanelMemory::default();
    let with_boon = MagicFindCoverage::Read(MagicFind { total: 333.0, luck: 300, pushed: 30.0, buffs: 3.0, boon: true });
    assert_eq!(step(&mut memory, with_boon, false).tooltip, step(&mut PanelMemory::default(), verified(300, 30.0, 3.0), false).tooltip);
}

/// The Magic Find reader tried and has no figure: the plugin's value if there is one, marked
/// as not live as always, or a dash; the tooltip says the addon has no coverage and why; grey.
#[test]
fn magic_find_without_coverage_falls_back_to_the_plugin_or_a_dash_and_says_why() {
    for reason in REASONS {
        for english in [false, true] {
            let why = format!(
                "{} ({})",
                if english { "Addon reading: no coverage" } else { "Lectura del addon: sin cobertura" },
                panel::uncovered_reason(reason, english)
            );
            let mut with_plugin = Case::sketch();
            with_plugin.magic_find = MagicFindCoverage::Unavailable(reason);
            let cell = with_plugin.view(english).magic_find;
            assert_eq!((cell.text.as_str(), cell.tone), (if english { "MF: 333% partial" } else { "MF: 333% parcial" }, Tone::Muted), "{reason:?}");
            assert!(has(&cell, &why) && has(&cell, if english { "Not a live reading" } else { "No es una lectura en vivo" }), "{reason:?}: {cell:?}");
            let mut without = Case::measuring();
            without.magic_find = MagicFindCoverage::Unavailable(reason);
            let cell = without.view(english).magic_find;
            assert_eq!((cell.text.as_str(), cell.tone), ("MF: —", Tone::Muted), "{reason:?}");
            assert!(has(&cell, &why), "{reason:?}: {cell:?}");
            // The reason replaces the generic line, it does not add to it.
            assert!(!has(&cell, "Verified Magic Find: no coverage") && !has(&cell, "Hallazgo mágico verificado: sin cobertura"));
        }
    }
    let mut live_state = Case::sketch();
    live_state.magic_find = MagicFindCoverage::Unavailable(Uncovered::Unsupported);
    assert!(has(&live_state.view(false).magic_find, "Lectura del addon: sin cobertura (un efecto necesita estado en vivo)"));
}

#[test]
fn points_are_whole_when_they_are_and_have_one_decimal_when_not() {
    for (value, text) in [
        (0.0, "0"),
        (333.0, "333"),
        (332.5, "332.5"),
        (12.49, "12.5"),
        (12.44, "12.4"),
        (12.04, "12"),
        (12.96, "13"),
        (0.05, "0.1"),
        (-5.0, "-5"),
        (-0.5, "-0.5"),
        (-12.26, "-12.3"),
        (9999.9, "9999.9"),
    ] {
        assert_eq!(panel::points(value), text, "{value}");
    }
}

/// What the Options window shows of each reader's last pass, for a screenshot.
#[test]
fn the_reader_diagnostics_name_the_outcome_the_exact_reason_and_the_cost() {
    // The budgets are the readers' own constants, whatever they are.
    let (bag_budget, magic_find_budget) = (tyrian_companion_nexus_core::bags::MAX_BYTES, tyrian_companion_nexus_core::magic_find::MAX_BYTES);
    assert_eq!(panel::bags_diagnostic(BagCoverage::NotRead, 0, 0), format!("Bags: not read; bytes: 0 / {bag_budget}; reads: 0"));
    assert_eq!(
        panel::bags_diagnostic(bags(63, 160), 1480, 37),
        format!("Bags: read, 97 used of 160, 63 free (8 bags in 8 bag slots); bytes: 1480 / {bag_budget}; reads: 37")
    );
    assert_eq!(
        panel::bags_diagnostic(BagCoverage::Unavailable(Uncovered::Alignment), 312, 9),
        format!("Bags: no coverage, Alignment (misaligned pointer); bytes: 312 / {bag_budget}; reads: 9")
    );
    assert_eq!(
        panel::magic_find_diagnostic(MagicFindCoverage::NotRead, 0, 0),
        format!("Magic Find: not read; bytes: 0 / {magic_find_budget}; reads: 0")
    );
    assert_eq!(
        panel::magic_find_diagnostic(verified(300, 30.0, 33.5), 28706, 410),
        format!("Magic Find: read, 363.5% = luck 300 + server 30 + effects 33.5 (boon modifiers counted: no); bytes: 28706 / {magic_find_budget}; reads: 410")
    );
    // A pass cut by the readers' own deadline names it; it is no longer a plain read failure.
    assert_eq!(
        panel::magic_find_diagnostic(MagicFindCoverage::Unavailable(Uncovered::Deadline), 41200, 603),
        format!("Magic Find: no coverage, Deadline (the read ran out of time); bytes: 41200 / {magic_find_budget}; reads: 603")
    );
    for reason in REASONS {
        for line in [panel::bags_diagnostic(BagCoverage::Unavailable(reason), 1, 1), panel::magic_find_diagnostic(MagicFindCoverage::Unavailable(reason), 1, 1)] {
            assert!(line.contains(&format!("{reason:?}")) && line.contains(panel::uncovered_reason(reason, true)), "{line}");
        }
    }
}

fn rate_after(memory: &mut PanelMemory, lo: Value, hi: Value, english: bool) -> Cell {
    Case::measuring()
        .with_farming(|frame| {
            frame["lo"] = lo;
            frame["hi"] = hi;
        })
        .view_with(memory, english)
        .rate
}

#[test]
fn the_rate_shows_what_it_has() {
    let fresh = |lo: Value, hi: Value| rate_after(&mut PanelMemory::default(), lo, hi, false);
    assert_eq!(fresh(json!(480), json!(560)).text, "480–560 b/h");
    assert_eq!(fresh(json!(37), json!(37)).text, "37 b/h");
    // A band one unit wide is one number rounded down and up: what a live session sends.
    assert_eq!(fresh(json!(37), json!(38)).text, "37 b/h");
    assert_eq!(fresh(json!(2), json!(3)).text, "2 b/h");
    assert_eq!(fresh(json!(480), Value::Null).text, "≥480 b/h");
    let none = fresh(Value::Null, Value::Null);
    assert_eq!((none.text.as_str(), none.tone), ("— b/h", Tone::Warning));
    assert!(has(&none, "Ritmo aún no disponible"));
    let normal = fresh(json!(480), json!(560));
    assert_eq!(normal.tone, Tone::Normal);
    assert_eq!(normal.tooltip, ["Bolsas por hora"]);
    assert_eq!(rate_after(&mut PanelMemory::default(), json!(480), json!(560), true).tooltip, ["Bags per hour"]);
}

/// The two thresholds, exactly: above 0.30 of the mean the band becomes its average, below 0.20
/// it is a band again, and from 0.20 to 0.30 it stays whatever it was.
#[test]
fn the_rate_averages_above_030_and_is_a_range_again_below_020() {
    let mut memory = PanelMemory::default();
    let mut step = |lo: i32, hi: i32| rate_after(&mut memory, json!(lo), json!(hi), false);
    // Mean 1000. Width 300 is exactly 0.30: not above it.
    assert_eq!(step(850, 1150).text, "850–1150 b/h");
    let averaged = step(849, 1151);
    assert_eq!(averaged.text, "~1000 b/h");
    assert!(has(&averaged, "Rango: 849–1151 b/h"), "the band stays in the tooltip: {averaged:?}");
    assert_eq!(averaged.tone, Tone::Normal);
    // Inside 0.20..=0.30 it stays averaged, however long it hovers there.
    for (lo, hi) in [(851, 1149), (880, 1120), (870, 1130), (899, 1101), (900, 1100)] {
        assert_eq!(step(lo, hi).text, "~1000 b/h", "{lo}–{hi}");
    }
    // Width 198 is below 0.20: a band again.
    assert_eq!(step(901, 1099).text, "901–1099 b/h");
    // And inside 0.20..=0.30 it now stays a band.
    for (lo, hi) in [(900, 1100), (880, 1120), (851, 1149), (850, 1150), (870, 1130)] {
        assert_eq!(step(lo, hi).text, format!("{lo}–{hi} b/h"));
    }
    assert_eq!(step(400, 1200).text, "~800 b/h");
    assert_eq!(step(401, 1200).text, "~801 b/h", "the mean rounds half up");
}

#[test]
fn the_rate_does_not_alternate_between_the_thresholds() {
    for start_averaged in [false, true] {
        let mut memory = PanelMemory::default();
        let first = if start_averaged { (600, 1400) } else { (990, 1010) };
        let mut texts = vec![rate_after(&mut memory, json!(first.0), json!(first.1), false).text];
        // A band whose width swings from 0.21 to 0.29 of its mean on every frame.
        for frame in 0..200 {
            let half = if frame % 2 == 0 { 105 } else { 145 };
            texts.push(rate_after(&mut memory, json!(1000 - half), json!(1000 + half), false).text);
        }
        let averaged = texts.iter().skip(1).filter(|text| text.starts_with('~')).count();
        assert_eq!(averaged, if start_averaged { 200 } else { 0 }, "{start_averaged}");
    }
    // Without a band there is nothing to remember: the next wide band starts from a range.
    let mut memory = PanelMemory::default();
    assert_eq!(rate_after(&mut memory, json!(600), json!(1400), false).text, "~1000 b/h");
    assert_eq!(rate_after(&mut memory, json!(700), Value::Null, false).text, "≥700 b/h");
    assert_eq!(rate_after(&mut memory, json!(880), json!(1120), false).text, "880–1120 b/h");
}

#[test]
fn the_rate_notes_are_its_tooltip_and_its_warning() {
    let old = Case::measuring().with_farming(|frame| frame["age"] = json!(20)).view(false).rate;
    assert_eq!((old.text.as_str(), old.tone), ("37 b/h", Tone::Warning));
    assert!(has(&old, "Último ritmo registrado") && has(&old, "Última lectura hace 20s"));
    let mut offline = Case::measuring();
    offline.farming = farming(None, Duration::ZERO);
    let none = offline.view(true).rate;
    assert_eq!((none.text.as_str(), none.tone), ("— b/h", Tone::Muted));
    assert!(has(&none, "No reading"));
}

#[test]
fn the_bags_tooltip_keeps_what_left_the_panel() {
    let plain = Case::measuring().view(false).bags;
    assert_eq!(plain.tooltip, ["Bolsas observadas", "Duración: 0:28:36"]);
    let goal = Case::measuring()
        .with_farming(|frame| {
            frame["goal"] = json!("bags");
            frame["target"] = json!(1000);
            frame["progress"] = json!(143);
            frame["eta"] = json!(4834);
        })
        .view(false)
        .bags;
    assert!(has(&goal, "Objetivo: 143 / 1000 bolsas") && has(&goal, "Quedan aprox. 1:20:34"), "{goal:?}");
    assert_eq!(goal.tone, Tone::Normal);
    let reached = Case::measuring()
        .with_farming(|frame| {
            frame["goal"] = json!("duration");
            frame["target"] = json!(1700);
            frame["progress"] = json!(1716);
        })
        .view(true)
        .bags;
    assert!(has(&reached, "Goal: 0:28:36 / 0:28:20") && has(&reached, "Goal reached") && has(&reached, "Countdown unavailable"), "{reached:?}");
    // The net at close: named always, a warning only when it came out lower than observed.
    let same = Case::measuring().with_farming(|frame| { frame["phase"] = json!("complete"); frame["net"] = json!(143); }).view(false).bags;
    assert!(has(&same, "Bolsas netas al cierre: 143"));
    assert_eq!(same.tone, Tone::Normal);
    let lower = Case::measuring().with_farming(|frame| { frame["phase"] = json!("complete"); frame["net"] = json!(120); }).view(false).bags;
    assert!(has(&lower, "Bolsas netas al cierre: 120") && has(&lower, "abriste o gastaste"));
    assert_eq!((lower.text.as_str(), lower.tone), ("143", Tone::Warning));
    let stopping = Case::measuring().with_farming(|frame| frame["phase"] = json!("stopping")).view(false).bags;
    assert!(has(&stopping, "Esperando la lectura final"));
    let starting = Case::measuring().with_farming(|frame| { frame["phase"] = json!("starting"); frame["observed"] = Value::Null; }).view(false).bags;
    assert_eq!(starting.text, NO_DATA);
    assert!(has(&starting, "Capturando el punto de partida"));
    // Old figures stay, in the warning tone, and say they are old.
    let mut stale = Case::measuring();
    stale.farming = farming(Some(&farming_frame()), Duration::from_secs(15));
    let stale = stale.view(false).bags;
    assert_eq!((stale.text.as_str(), stale.tone), ("143", Tone::Warning));
    assert!(has(&stale, "Datos antiguos"));
}

#[test]
fn the_stack_says_a_dash_and_why_in_every_state_without_a_price() {
    // (price view, connection, tone of the two prices, tone of the label, what the tooltip says)
    let rows: Vec<(PriceView, Status, Tone, Tone, &str)> = vec![
        (no_price(), Status::WaitingForPlugin, Tone::Muted, Tone::Normal, "Sin conexión"),
        (no_price(), Status::Connected, Tone::Muted, Tone::Normal, "Precio no disponible en este host"),
        (price(None, Duration::ZERO), Status::Connected, Tone::Muted, Tone::Normal, "Precio aún sin leer"),
        (price(Some(&price_frame("pending")), Duration::ZERO), Status::Connected, Tone::Muted, Tone::Normal, "Precio aún sin leer"),
        (price(Some(&price_frame("idle")), Duration::ZERO), Status::Connected, Tone::Muted, Tone::Normal, "El precio se lee durante una sesión"),
        (price(Some(&price_frame("idle")), Duration::from_secs(40)), Status::Connected, Tone::Muted, Tone::Normal, "El precio se lee durante una sesión"),
        (price(Some(&price_frame("stale")), Duration::ZERO), Status::Connected, Tone::Warning, Tone::Warning, "Precio caducado (hace 21 min)"),
        (price(Some(&price_frame("ok")), Duration::from_secs(15)), Status::Connected, Tone::Muted, Tone::Normal, "Precio aún sin leer"),
    ];
    for (price, connection, tone, label_tone, why) in rows {
        let mut case = Case::measuring();
        case.price = price;
        case.connection = connection;
        let view = case.view(false);
        assert_eq!((view.buy.text.as_str(), view.sell.text.as_str()), (NO_DATA, NO_DATA), "{why}");
        assert_eq!((view.buy.tone, view.sell.tone, view.stack_label.tone), (tone, tone, label_tone), "{why}");
        for cell in [&view.stack_label, &view.buy, &view.sell] {
            assert!(has(cell, why), "{why}: {cell:?}");
        }
    }
    let mut no_quote = price_frame("ok");
    for key in ["sell", "sellStack", "list", "listStack"] {
        no_quote[key] = Value::Null;
    }
    let mut case = Case::measuring();
    case.price = price(Some(&no_quote), Duration::ZERO);
    let view = case.view(true);
    assert_eq!((view.buy.text.as_str(), view.buy.tone), (NO_DATA, Tone::Muted));
    assert!(has(&view.buy, "No quote") && has(&view.stack_label, "No quote"));
}

#[test]
fn the_stack_prices_are_the_gross_ones_and_name_themselves_in_the_tooltip() {
    let view = Case::measuring().view(false);
    assert_eq!(view.buy.tooltip, ["Pedido más alto × 250", "Unidad: 2s 83c"]);
    assert_eq!(view.sell.tooltip, ["Oferta más baja × 250", "Unidad: 3s 56c"]);
    assert_eq!(view.stack_label.tooltip, ["Stack de 250 sacos · precio bruto del bazar"]);
    let english = Case::measuring().view(true);
    assert_eq!(english.buy.tooltip, ["Highest buy order × 250", "Unit: 2s 83c"]);
    assert_eq!(english.sell.tooltip, ["Lowest sell offer × 250", "Unit: 3s 56c"]);
}

/// One side quoted and the other not: the side without a figure says a dash and why, and the
/// label does not, because the reading itself is fine.
#[test]
fn one_side_without_a_quote_says_why_in_its_own_tooltip() {
    for (missing, present) in [("list", "sell"), ("sell", "list")] {
        let mut one_side = price_frame("ok");
        one_side[missing] = Value::Null;
        one_side[format!("{missing}Stack")] = Value::Null;
        let mut case = Case::measuring();
        case.price = price(Some(&one_side), Duration::ZERO);
        for english in [false, true] {
            let view = case.view(english);
            let (empty, full) = if missing == "list" { (&view.sell, &view.buy) } else { (&view.buy, &view.sell) };
            assert_eq!((empty.text.as_str(), empty.tone), (NO_DATA, Tone::Muted), "{missing}");
            assert!(has(empty, if english { "No quote on this side" } else { "Sin cotización en este lado" }), "{empty:?}");
            assert_eq!(full.tone, Tone::Normal, "{present}");
            assert_eq!(full.tooltip.len(), 2, "the quoted side has nothing to explain: {full:?}");
            assert_eq!((view.stack_label.tone, view.stack_label.tooltip.len()), (Tone::Normal, 1));
        }
    }
    // A unit price whose stack does not fit an int32 arrives without the stack.
    let mut no_stack = price_frame("ok");
    no_stack["listStack"] = Value::Null;
    let mut case = Case::measuring();
    case.price = price(Some(&no_stack), Duration::ZERO);
    let view = case.view(false);
    assert_eq!(view.sell.text, NO_DATA);
    assert_eq!(view.sell.tooltip, ["Oferta más baja × 250", "Unidad: 3s 56c", "El stack no cabe en la trama"]);
}

#[test]
fn the_status_dot_and_text_by_connection() {
    // (connection, dot, Spanish, English)
    for (connection, dot, es, en) in [
        (Status::WaitingForPlugin, Tone::Error, "Sin conexión", "Offline"),
        (Status::MissingToken, Tone::Error, "Falta el token", "Token missing"),
        (Status::TokenRejected, Tone::Error, "Token rechazado", "Token rejected"),
        (Status::UpdateRequired, Tone::Error, "Actualiza el addon", "Update the addon"),
        (Status::GameExiting, Tone::Muted, "El juego se cierra", "The game is closing"),
    ] {
        let mut case = Case::measuring();
        case.connection = connection;
        let view = case.view(false);
        assert_eq!((view.status_dot, view.status.text.as_str()), (dot, es), "{connection:?}");
        // The text carries the tone too, never the dot alone; grey text would read as disabled.
        assert_eq!(view.status.tone, if dot == Tone::Error { Tone::Error } else { Tone::Normal });
        assert!(has(&view.status, "Conexión al host: sin conexión") && has(&view.status, "Datos antiguos"));
        assert_eq!(case.view(true).status.text, en);
    }
    // Connected, nothing measuring.
    let mut waiting = Case::measuring();
    waiting.farming = farming(None, Duration::ZERO);
    let view = waiting.view(false);
    assert_eq!((view.status_dot, view.status.text.as_str(), view.status.tone), (Tone::Muted, "Esperando sesión", Tone::Normal));
    assert!(has(&view.status, "Conexión al host: conectado"));
    // Connected to a host that has no panel feed.
    waiting.farming = SharedState::new().farming_view(Instant::now());
    let view = waiting.view(false);
    assert_eq!((view.status_dot, view.status.text.as_str(), view.status.tone), (Tone::Warning, "Sin panel", Tone::Warning));
    assert!(has(&view.status, "Panel no disponible en este host"));
}

/// Inventory status and wallet coverage are in the status tooltip with or without a connection
/// and with or without a reading, and an error reported before the connection was lost stays.
#[test]
fn no_notice_is_lost_without_a_connection_or_without_a_reading() {
    let mut offline = Case::measuring().with_farming(|frame| frame["err"] = json!("observe"));
    offline.connection = Status::WaitingForPlugin;
    offline.live = LiveStatus::Partial;
    offline.wallet = WalletCoverage::Unavailable(WalletError::Changed);
    let status = offline.view(false).status;
    assert_eq!(
        status.tooltip,
        [
            "Conexión al host: sin conexión",
            "Esperando a Tyrian Companion en Obsidian o Hebra",
            "Datos antiguos · última lectura",
            "Error pendiente: No se pudo actualizar",
            "Revisa la sesión en Hebra u Obsidian",
            "Inventario: cantidades sin resolver",
            "Monedas: sin cobertura (cambió durante la lectura)",
        ]
    );
    let english = offline.view(true).status;
    assert!(has(&english, "Pending error: Could not update") && has(&english, "Currencies: no coverage (changed while reading)"));
    // Without a pending error there is none to name.
    let mut quiet = Case::measuring();
    quiet.connection = Status::MissingToken;
    let status = quiet.view(false).status;
    assert!(!has(&status, "Error pendiente") && !has(&status, "Revisa la sesión"));
    assert!(has(&status, "Inventario: observaciones guardadas") && has(&status, "Monedas cubiertas: 55"));
    // No reading at all, offline and connected.
    for connection in [Status::WaitingForPlugin, Status::Connected] {
        let mut empty = Case::measuring();
        empty.connection = connection;
        empty.farming = farming(None, Duration::ZERO);
        empty.live = LiveStatus::Unavailable;
        let status = empty.view(false).status;
        assert!(has(&status, "Inventario: lectura no disponible"), "{connection:?}: {status:?}");
        assert!(has(&status, "Monedas: sin cobertura (sin lectura)"), "{connection:?}: {status:?}");
        assert!(!has(&status, "Datos antiguos"), "{connection:?}");
    }
    // And a host without the panel feed.
    let mut no_panel = Case::measuring();
    no_panel.farming = SharedState::new().farming_view(Instant::now());
    let status = no_panel.view(false).status;
    assert!(has(&status, "Inventario: observaciones guardadas") && has(&status, "Monedas cubiertas: 55"), "{status:?}");
}

#[test]
fn the_status_dot_and_text_by_phase_and_problem() {
    for (phase, dot, es) in [
        ("idle", Tone::Muted, "Sin medición"),
        ("starting", Tone::Good, "Preparando medición"),
        ("active", Tone::Good, "Midiendo"),
        ("stopping", Tone::Good, "Terminando sesión"),
        ("provisional", Tone::Good, "Cierre provisional"),
        ("complete", Tone::Muted, "Sesión finalizada"),
        ("abandoned", Tone::Muted, "Sesión abandonada"),
        ("error", Tone::Error, "Error de sesión"),
    ] {
        let view = Case::measuring().with_farming(|frame| frame["phase"] = json!(phase)).view(false);
        assert_eq!((view.status_dot, view.status.text.as_str()), (dot, es), "{phase}");
        assert_eq!(view.status.tone, if dot == Tone::Error { Tone::Error } else { Tone::Normal }, "{phase}");
    }
    // An error names itself, keeps the phase in the tooltip and says where to look.
    let failed = Case::measuring().with_farming(|frame| frame["err"] = json!("observe")).view(false);
    assert_eq!((failed.status_dot, failed.status.text.as_str(), failed.status.tone), (Tone::Error, "No se pudo actualizar", Tone::Error));
    assert!(has(&failed.status, "Fase: Midiendo") && has(&failed.status, "Revisa la sesión en Hebra u Obsidian"));
    // Old data: orange, and the text still says the phase.
    let mut stale = Case::measuring();
    stale.farming = farming(Some(&farming_frame()), Duration::from_secs(15));
    let view = stale.view(false);
    assert_eq!((view.status_dot, view.status.text.as_str(), view.status.tone), (Tone::Warning, "Midiendo", Tone::Warning));
    assert!(has(&view.status, "Datos antiguos · última lectura"));
}

#[test]
fn the_status_tooltip_keeps_inventory_and_wallet_and_only_lasting_problems_change_the_colour() {
    let normal = Case::measuring().view(false).status;
    assert_eq!(normal.tooltip, ["Conexión al host: conectado", "Inventario: observaciones guardadas", "Monedas cubiertas: 55"]);
    // These two come and go in normal measurement: tooltip only.
    for live in [LiveStatus::Waiting, LiveStatus::Partial] {
        let mut case = Case::measuring();
        case.live = live;
        let view = case.view(false);
        assert_eq!((view.status_dot, view.status.tone), (Tone::Good, Tone::Normal), "{live:?}");
        assert!(has(&view.status, panel::inventory_status(live, false)));
    }
    // A source that cannot measure while a session needs it: orange.
    for live in [LiveStatus::NotNegotiated, LiveStatus::UnsupportedBuild, LiveStatus::Unavailable, LiveStatus::Conflict, LiveStatus::StorageUnavailable] {
        let mut case = Case::measuring();
        case.live = live;
        let view = case.view(false);
        assert_eq!((view.status_dot, view.status.text.as_str(), view.status.tone), (Tone::Warning, "Midiendo", Tone::Warning), "{live:?}");
        assert!(has(&view.status, panel::inventory_status(live, false)));
        // With no session measuring it is not a problem of the session.
        let idle = Case { live, ..Case::measuring().with_farming(|frame| frame["phase"] = json!("complete")) }.view(false);
        assert_eq!(idle.status_dot, Tone::Muted, "{live:?}");
    }
    let mut wallet = Case::measuring();
    wallet.wallet = WalletCoverage::Unavailable(WalletError::Changed);
    let view = wallet.view(true);
    assert_eq!(view.status_dot, Tone::Warning);
    assert!(has(&view.status, "Currencies: no coverage (changed while reading)"));
}

/// A text is covered by a sample when it is the sample with characters taken out and digits
/// changed: then it cannot be wider, whatever the font, as long as the sample is measured with
/// the widest digit, which is what the addon does.
fn covered(text: &str, samples: &[String]) -> bool {
    let shape = |text: &str| text.chars().map(|character| if character.is_ascii_digit() { '9' } else { character }).collect::<Vec<_>>();
    let text = shape(text);
    samples.iter().any(|sample| {
        let mut rest = text.iter().peekable();
        for character in shape(sample) {
            if rest.peek() == Some(&&character) {
                rest.next();
            }
        }
        rest.peek().is_none()
    })
}

#[test]
fn covered_means_no_wider_than_a_sample() {
    let samples = panel::width_samples(false);
    for text in ["7g 7s 62c", "45c", "2s 83c", "1000g 0s 0c", "99999g 99s 99c", NO_DATA, "stack"] {
        assert!(covered(text, &samples.right), "{text}");
    }
    for text in ["100000g 0s 0c", "214748g 36s 47c", "stacks", "7g 7s 62c ·"] {
        assert!(!covered(text, &samples.right), "{text}");
    }
    for text in ["37 b/h", "480–560 b/h", "9000–9999 b/h", "~1234 b/h", "≥480 b/h", "— b/h", "bolsas"] {
        assert!(covered(text, &samples.left), "{text}");
    }
    for text in ["10000–12000 b/h", "~10000 b/h", "37 bolsas/h"] {
        assert!(!covered(text, &samples.left), "{text}");
    }
}

/// Every state the panel can be in: the same nine cells, each one line of text with a tooltip,
/// and each text covered by a width the addon reserved for its own cell. There is no state in
/// which a line is missing, grows a second one or pushes the window wider.
#[test]
fn every_text_the_panel_produces_is_one_line_covered_by_a_reserved_sample() {
    let mut frames = vec![None];
    for phase in ["idle", "starting", "active", "stopping", "provisional", "complete", "error", "abandoned"] {
        for err in [Value::Null, json!("start"), json!("observe"), json!("stop"), json!("save"), json!("other")] {
            for (lo, hi, observed) in [
                (Value::Null, Value::Null, Value::Null),
                (json!(480), Value::Null, json!(0)),
                (json!(400), json!(1200), json!(143)),
                (json!(9000), json!(9999), json!(9999)),
                (json!(37), json!(38), json!(1234)),
            ] {
                for (index, (slots, mf)) in [(Value::Null, Value::Null), (json!(9999), json!(9999)), (json!(0), json!(0)), (json!(250), json!(333))].into_iter().enumerate() {
                    // The middle figures add nothing outside one phase; the limits go everywhere.
                    if index > 1 && phase != "active" {
                        continue;
                    }
                    let mut frame = farming_frame();
                    frame["phase"] = json!(phase);
                    frame["err"] = err.clone();
                    frame["lo"] = lo.clone();
                    frame["hi"] = hi.clone();
                    frame["observed"] = observed.clone();
                    frame["slots"] = slots;
                    frame["mfKind"] = if mf.is_null() { json!("unknown") } else { json!("partial") };
                    frame["mf"] = mf;
                    frames.push(Some(frame));
                }
            }
        }
    }
    // A stack of exactly 1000 g, the dearest that is reserved for, one side alone, and the rest.
    let mut thousand = price_frame("ok");
    thousand["sellStack"] = json!(10_000_000);
    thousand["listStack"] = json!(999_999_999);
    let mut one_side = price_frame("ok");
    one_side["sell"] = Value::Null;
    one_side["sellStack"] = Value::Null;
    let prices = [None, Some(price_frame("ok")), Some(thousand), Some(one_side), Some(price_frame("pending")), Some(price_frame("stale")), Some(price_frame("idle"))];
    let mut states = 0;
    for english in [false, true] {
        let samples = panel::width_samples(english);
        for frame in &frames {
            for local in [0, 15] {
                for (index, price_frame) in prices.iter().enumerate() {
                    for connection in [Status::Connected, Status::WaitingForPlugin, Status::MissingToken, Status::TokenRejected, Status::UpdateRequired, Status::GameExiting] {
                        for read in 0..3 {
                            // The addon's reader only runs with a connection.
                            if read > 0 && connection != Status::Connected {
                                continue;
                            }
                            let case = Case {
                                connection,
                                farming: farming(frame.as_ref(), Duration::from_secs(local)),
                                price: if index == 0 { no_price() } else { price(price_frame.as_ref(), Duration::from_secs(local)) },
                                live: if local == 0 { LiveStatus::Measuring } else { LiveStatus::Unavailable },
                                wallet: WalletCoverage::Listed(55),
                                bags: match read { 0 => BagCoverage::NotRead, 1 => bags(2, 160), _ => BagCoverage::Unavailable(Uncovered::Changed) },
                                magic_find: match read { 0 => MagicFindCoverage::NotRead, 1 => verified(300, 30.5, 3.0), _ => MagicFindCoverage::Unavailable(Uncovered::Unsupported) },
                            };
                            let view = case.view(english);
                            for cell in view.cells() {
                                assert!(!cell.text.is_empty() && !cell.text.contains('\n'), "{cell:?}");
                                assert!(!cell.tooltip.is_empty() && cell.tooltip.iter().all(|line| !line.is_empty() && !line.contains('\n')), "{cell:?}");
                            }
                            assert!(view.status_label.ends_with(':'));
                            for (cell, group) in [
                                (&view.bags_label, &samples.left), (&view.rate, &samples.left), (&view.bags, &samples.left_large),
                                (&view.stack_label, &samples.right), (&view.buy, &samples.right), (&view.sell, &samples.right),
                                (&view.slots, &samples.lines), (&view.magic_find, &samples.lines),
                            ] {
                                assert!(covered(&cell.text, group), "{:?} is not covered by {group:?}", cell.text);
                            }
                            // The status is one of the reserved texts, whole.
                            assert!(samples.status.contains(&view.status.text), "{} is not reserved", view.status.text);
                            states += 1;
                        }
                    }
                }
            }
        }
    }
    assert!(states > 100_000, "{states}");
}

/// The figures of the request, by name: 9999 bags, a long range, an average of four digits,
/// stacks over 100 g and of 1000 g, three digits of Magic Find.
#[test]
fn content_at_its_limits() {
    let mut memory = PanelMemory::default();
    let mut limit = Case::measuring().with_frame(sketch_frame(), |frame| {
        frame["observed"] = json!(9999);
        frame["lo"] = json!(1234);
        frame["hi"] = json!(1456);
        frame["slots"] = json!(250);
    });
    let mut dear = price_frame("ok");
    dear["sellStack"] = json!(1_234_567);
    dear["listStack"] = json!(10_000_000);
    limit.price = price(Some(&dear), Duration::ZERO);
    let view = limit.view_with(&mut memory, false);
    assert_eq!((view.bags.text.as_str(), view.rate.text.as_str()), ("9999", "1234–1456 b/h"));
    assert_eq!((view.buy.text.as_str(), view.sell.text.as_str()), ("123g 45s 67c", "1000g 0s 0c"));
    assert_eq!((view.slots.text.as_str(), view.magic_find.text.as_str()), ("Huecos: 250 libres", "MF: 333% parcial"));
    let wide = limit.with_frame(sketch_frame(), |frame| { frame["lo"] = json!(900); frame["hi"] = json!(1568); }).view_with(&mut memory, false).rate;
    assert_eq!(wide.text, "~1234 b/h");
    assert_eq!(rate_after(&mut PanelMemory::default(), json!(9000), json!(9999), true).text, "9000–9999 b/h");
}

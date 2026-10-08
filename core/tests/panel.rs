//! The Labyrinth panel as data: every cell in every state, the rate's range-or-average choice,
//! and the texts at their limits. The addon paints exactly what `panel::view` returns.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tyrian_companion_nexus_core::farming::FarmingView;
use tyrian_companion_nexus_core::live::LiveStatus;
use tyrian_companion_nexus_core::panel::{self, Cell, PanelInput, PanelMemory, PanelView, Tone, NO_DATA};
use tyrian_companion_nexus_core::price::PriceView;
use tyrian_companion_nexus_core::protocol::{parse_server_line, ServerLine};
use tyrian_companion_nexus_core::state::{SharedState, Status};
use tyrian_companion_nexus_core::wallet::{WalletCoverage, WalletError};

const NONCE: &str = "Zk3m1Qw9Lr0aT7yUc2Vb5g";

fn farming_frame() -> Value {
    json!({ "v":3, "type":"farming_state", "tag":"farm1", "nonce":NONCE, "seq":1, "ttl":15,
        "phase":"active", "err":null, "elapsed":1716, "observed":143, "net":null,
        "lo":37, "hi":37, "age":0, "slots":null, "slotSrc":"unknown", "slotAge":null,
        "goal":"none", "target":null, "progress":null, "eta":null, "mf":null,
        "mfKind":"unknown", "prep":"unknown" })
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

struct Case {
    connection: Status,
    farming: FarmingView,
    price: PriceView,
    live: LiveStatus,
    wallet: WalletCoverage,
    slots: Option<i32>,
    magic_find: Option<i32>,
}

impl Case {
    /// Connected, measuring, a fresh price: the panel of the sketch.
    fn measuring() -> Self {
        Self {
            connection: Status::Connected,
            farming: farming(Some(&farming_frame()), Duration::ZERO),
            price: price(Some(&price_frame("ok")), Duration::ZERO),
            live: LiveStatus::Measuring,
            wallet: WalletCoverage::Listed(55),
            slots: None,
            magic_find: None,
        }
    }

    fn with_farming(mut self, change: impl FnOnce(&mut Value)) -> Self {
        let mut frame = farming_frame();
        change(&mut frame);
        self.farming = farming(Some(&frame), Duration::ZERO);
        self
    }

    fn view_with(&self, memory: &mut PanelMemory, english: bool) -> PanelView {
        panel::view(
            &PanelInput {
                connection: self.connection,
                farming: &self.farming,
                price: &self.price,
                live: self.live,
                wallet: self.wallet,
                slots: self.slots,
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
    let mut case = Case::measuring();
    case.slots = Some(63);
    case.magic_find = Some(333);
    let view = case.view(false);
    assert_eq!((view.bags_label.text.as_str(), view.bags.text.as_str(), view.rate.text.as_str()), ("bolsas", "143", "37 b/h"));
    assert_eq!((view.stack_label.text.as_str(), view.buy.text.as_str(), view.sell.text.as_str()), ("stack", "7g 7s 62c", "8g 90s 37c"));
    assert_eq!((view.slots.text.as_str(), view.magic_find.text.as_str()), ("Huecos: 63 libres", "MF: 333%"));
    assert_eq!((view.status_label.as_str(), view.status_dot, view.status.text.as_str()), ("Estado:", Tone::Good, "Midiendo"));
    for cell in view.cells() {
        assert_eq!(cell.tone, Tone::Normal, "{cell:?}");
    }
    let english = case.view(true);
    assert_eq!((english.bags_label.text.as_str(), english.stack_label.text.as_str()), ("bags", "stack"));
    assert_eq!((english.slots.text.as_str(), english.magic_find.text.as_str()), ("Slots: 63 free", "MF: 333%"));
    assert_eq!((english.status_label.as_str(), english.status.text.as_str()), ("Status:", "Measuring"));
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

/// What the addon ships today: no validated reader for slots or Magic Find, so both say `—`
/// in the muted tone and their tooltips say why.
#[test]
fn slots_and_magic_find_say_a_dash_until_a_reader_provides_them() {
    for english in [false, true] {
        let view = Case::measuring().view(english);
        assert_eq!(view.slots.text, if english { "Slots: —" } else { "Huecos: —" });
        assert_eq!(view.magic_find.text, "MF: —");
        assert_eq!((view.slots.tone, view.magic_find.tone), (Tone::Muted, Tone::Muted));
        assert!(has(&view.slots, if english { "Not available yet" } else { "Dato aún no disponible" }));
        assert!(has(&view.magic_find, if english { "Verified Magic Find: no coverage" } else { "Hallazgo mágico verificado: sin cobertura" }));
    }
}

#[test]
fn slots_warn_at_ten_and_turn_to_error_at_three() {
    for (slots, tone) in [(250, Tone::Normal), (11, Tone::Normal), (10, Tone::Warning), (4, Tone::Warning), (3, Tone::Error), (0, Tone::Error)] {
        let mut case = Case::measuring();
        case.slots = Some(slots);
        let view = case.view(false);
        assert_eq!(view.slots.text, format!("Huecos: {slots} libres"));
        assert_eq!(view.slots.tone, tone, "{slots}");
        assert_eq!(has(&view.slots, "Quedan pocos huecos"), tone != Tone::Normal, "{slots}");
    }
}

#[test]
fn magic_find_warns_while_it_is_below_the_peak_of_the_session() {
    let mut memory = PanelMemory::default();
    let mut case = Case::measuring();
    let step = |case: &mut Case, memory: &mut PanelMemory, value: Option<i32>| {
        case.magic_find = value;
        case.view_with(memory, false).magic_find
    };
    assert_eq!(step(&mut case, &mut memory, Some(320)).tone, Tone::Normal);
    assert_eq!(step(&mut case, &mut memory, Some(333)).tone, Tone::Normal);
    let dropped = step(&mut case, &mut memory, Some(301));
    assert_eq!((dropped.text.as_str(), dropped.tone), ("MF: 301%", Tone::Warning));
    assert!(has(&dropped, "Máximo de la sesión: 333%"));
    assert_eq!(step(&mut case, &mut memory, Some(333)).tone, Tone::Normal, "back at the peak");
    // A reading that goes missing does not forget the peak; a new session does.
    assert_eq!(step(&mut case, &mut memory, None).tone, Tone::Muted);
    assert_eq!(step(&mut case, &mut memory, Some(320)).tone, Tone::Warning);
    let mut starting = Case::measuring().with_farming(|frame| frame["phase"] = json!("starting"));
    assert_eq!(step(&mut starting, &mut memory, Some(300)).tone, Tone::Normal, "a new session has its own peak");
    assert_eq!(step(&mut case, &mut memory, Some(310)).tone, Tone::Normal);
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
    // One side without a quote: a dash on that side only.
    let mut one_side = price_frame("ok");
    one_side["list"] = Value::Null;
    one_side["listStack"] = Value::Null;
    let mut case = Case::measuring();
    case.price = price(Some(&one_side), Duration::ZERO);
    let view = case.view(false);
    assert_eq!((view.buy.text.as_str(), view.buy.tone), ("7g 7s 62c", Tone::Normal));
    assert_eq!((view.sell.text.as_str(), view.sell.tone), (NO_DATA, Tone::Muted));
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

/// Every combination the panel can be in: the same nine cells, each one line of text with a
/// tooltip. There is no state in which a line is missing or grows a second one.
#[test]
fn the_panel_has_the_same_cells_in_every_state() {
    assert_eq!(PanelView::LINES, 6);
    let mut frames = vec![None];
    for phase in ["idle", "starting", "active", "stopping", "provisional", "complete", "error", "abandoned"] {
        for err in [Value::Null, json!("observe"), json!("save")] {
            for (lo, hi) in [(Value::Null, Value::Null), (json!(480), Value::Null), (json!(400), json!(1200))] {
                let mut frame = farming_frame();
                frame["phase"] = json!(phase);
                frame["err"] = err.clone();
                frame["lo"] = lo;
                frame["hi"] = hi;
                frames.push(Some(frame));
            }
        }
    }
    let prices = [None, Some(price_frame("ok")), Some(price_frame("pending")), Some(price_frame("stale")), Some(price_frame("idle"))];
    let mut states = 0;
    for english in [false, true] {
        for frame in &frames {
            for local in [0, 5, 15, 60] {
                for (index, price_frame) in prices.iter().enumerate() {
                    for connection in [Status::Connected, Status::WaitingForPlugin, Status::MissingToken, Status::GameExiting] {
                        for (slots, magic_find) in [(None, None), (Some(2), Some(333))] {
                            let case = Case {
                                connection,
                                farming: farming(frame.as_ref(), Duration::from_secs(local)),
                                price: if index == 0 { no_price() } else { price(price_frame.as_ref(), Duration::from_secs(local)) },
                                live: if local == 0 { LiveStatus::Measuring } else { LiveStatus::Unavailable },
                                wallet: WalletCoverage::Listed(55),
                                slots,
                                magic_find,
                            };
                            let view = case.view(english);
                            for cell in view.cells() {
                                assert!(!cell.text.is_empty() && !cell.text.contains('\n'), "{cell:?}");
                                assert!(!cell.tooltip.is_empty() && cell.tooltip.iter().all(|line| !line.is_empty() && !line.contains('\n')), "{cell:?}");
                            }
                            assert!(view.status_label.ends_with(':'));
                            states += 1;
                        }
                    }
                }
            }
        }
    }
    assert!(states > 20_000, "{states}");
}

fn widest(texts: &[String]) -> usize {
    texts.iter().map(|text| text.chars().count()).max().unwrap()
}

/// The addon reserves the width of each part from `width_samples`. These are the contents at
/// their limits; none is longer than what was reserved for its part.
#[test]
fn content_at_its_limits_fits_the_reserved_widths() {
    for english in [false, true] {
        let samples = panel::width_samples(english);
        let mut memory = PanelMemory::default();
        // 9999 bags, a long range, a wide one averaged to four digits, a stack over 100 g.
        let mut limit = Case::measuring().with_farming(|frame| {
            frame["observed"] = json!(9999);
            frame["lo"] = json!(1234);
            frame["hi"] = json!(1456);
        });
        let mut dear = price_frame("ok");
        dear["sellStack"] = json!(1_234_567);
        dear["listStack"] = json!(9_999_999);
        limit.price = price(Some(&dear), Duration::ZERO);
        limit.slots = Some(250);
        limit.magic_find = Some(333);
        let view = limit.view_with(&mut memory, english);
        assert_eq!((view.bags.text.as_str(), view.rate.text.as_str()), ("9999", "1234–1456 b/h"));
        assert_eq!((view.buy.text.as_str(), view.sell.text.as_str()), ("123g 45s 67c", "999g 99s 99c"));
        assert_eq!(view.magic_find.text, "MF: 333%");
        assert!(view.bags.text.chars().count() <= widest(&samples.left_large));
        assert!(view.rate.text.chars().count() <= widest(&samples.left));
        assert!(view.buy.text.chars().count() <= widest(&samples.right) && view.sell.text.chars().count() <= widest(&samples.right));
        assert!(view.slots.text.chars().count() <= widest(&samples.lines) && view.magic_find.text.chars().count() <= widest(&samples.lines));
        let wide = limit.with_farming(|frame| { frame["lo"] = json!(900); frame["hi"] = json!(1568); }).view_with(&mut memory, english).rate;
        assert_eq!(wide.text, "~1234 b/h");
        assert!(wide.text.chars().count() <= widest(&samples.left));
        let extreme = rate_after(&mut PanelMemory::default(), json!(9000), json!(9999), english);
        assert_eq!(extreme.text, "9000–9999 b/h");
        assert!(extreme.text.chars().count() <= widest(&samples.left));
        // Every text the status can show is one of the reserved ones, in both languages.
        let mut texts = Vec::new();
        for connection in [Status::Connected, Status::WaitingForPlugin, Status::MissingToken, Status::TokenRejected, Status::UpdateRequired, Status::GameExiting] {
            for phase in ["idle", "starting", "active", "stopping", "provisional", "complete", "error", "abandoned"] {
                for err in [Value::Null, json!("start"), json!("observe"), json!("stop"), json!("save"), json!("other")] {
                    let mut case = Case::measuring().with_farming(|frame| { frame["phase"] = json!(phase); frame["err"] = err.clone(); });
                    case.connection = connection;
                    texts.push(case.view(english).status.text);
                }
            }
            let mut empty = Case::measuring();
            empty.connection = connection;
            empty.farming = farming(None, Duration::ZERO);
            texts.push(empty.view(english).status.text);
            empty.farming = SharedState::new().farming_view(Instant::now());
            texts.push(empty.view(english).status.text);
        }
        for text in texts {
            assert!(samples.status.contains(&text), "{text} is not reserved");
        }
        // The longest status in each language, so a longer one added later is noticed here.
        assert_eq!(widest(&samples.status), 21, "{:?}", samples.status);
    }
}

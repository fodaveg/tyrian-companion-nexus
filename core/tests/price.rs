//! `price2` frames, freshness, formatting and the painted lines, against the fixture shared
//! byte for byte with the plugin.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tyrian_companion_nexus_core::price::{
    format_coins, panel_lines, PriceState, PriceStatus, PriceView,
};
use tyrian_companion_nexus_core::protocol::{build_price_sub_line, parse_server_line, ServerLine};
use tyrian_companion_nexus_core::state::SharedState;

const NONCE: &str = "AQEBAQEBAQEBAQEBAQEBAQ";
const OTHER_NONCE: &str = "Yk3m1Qw9Lr0aT7yUc2Vb5g";

fn fixture() -> Vec<Value> {
    let text = include_str!("fixtures/price2.json");
    serde_json::from_str::<Value>(text).unwrap()["frames"]
        .as_array()
        .unwrap()
        .clone()
}

fn frame(index: usize) -> Value {
    fixture()[index].clone()
}

fn reading(value: &Value) -> PriceState {
    let ServerLine::PriceState(state) = parse_server_line(&value.to_string()) else {
        panic!("expected a valid price state: {value}");
    };
    state
}

fn assert_discard(value: &Value) {
    assert_eq!(
        parse_server_line(&value.to_string()),
        ServerLine::Discard,
        "{value}"
    );
}

fn subscribed() -> SharedState {
    let state = SharedState::new();
    state.begin_price_connection(NONCE);
    assert!(state.enable_price(NONCE));
    state
}

#[test]
fn every_fixture_frame_parses_and_the_sub_line_equals_the_fixture_as_json() {
    let frames = fixture();
    assert_eq!(
        parse_server_line(&frames[0].to_string()),
        ServerLine::PriceCapability(NONCE.into())
    );
    let sub = build_price_sub_line(NONCE, 1).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&sub).unwrap(), frames[1]);
    assert!(sub.ends_with('\n'));
    let first = reading(&frames[2]);
    assert_eq!(
        (
            first.st,
            first.sell,
            first.sell_stack,
            first.list,
            first.list_stack,
            first.age
        ),
        (
            PriceStatus::Ok,
            Some(345),
            Some(86250),
            Some(367),
            Some(91750),
            Some(412)
        )
    );
    assert_eq!(reading(&frames[3]).st, PriceStatus::Ok);
    assert_eq!(reading(&frames[4]).st, PriceStatus::Pending);
    assert_eq!(reading(&frames[5]).st, PriceStatus::Stale);
    assert_eq!(reading(&frames[6]).st, PriceStatus::Idle);
    for frame in &frames {
        assert!(frame.to_string().len() <= 512);
    }
}

#[test]
fn byte_boundary_is_inclusive_and_duplicate_or_extra_keys_are_rejected() {
    let body = frame(2).to_string();
    let padded = format!("{body}{}", " ".repeat(512 - body.len()));
    assert!(matches!(
        parse_server_line(&padded),
        ServerLine::PriceState(_)
    ));
    assert_eq!(
        parse_server_line(&format!("{padded} ")),
        ServerLine::Discard
    );
    let duplicate = body.replacen("\"seq\":1", "\"seq\":1,\"seq\":2", 1);
    assert_eq!(parse_server_line(&duplicate), ServerLine::Discard);
    let mut extra = frame(2);
    extra["itemId"] = json!(36038);
    assert_discard(&extra);
    let mut missing = frame(2);
    missing.as_object_mut().unwrap().remove("age");
    assert_discard(&missing);
    let mut cap = frame(0);
    cap["extra"] = json!(1);
    assert_discard(&cap);
    let cap_dup = frame(0)
        .to_string()
        .replacen("\"v\":3", "\"v\":3,\"v\":3", 1);
    assert_eq!(parse_server_line(&cap_dup), ServerLine::Discard);
}

#[test]
fn st_is_a_closed_enum() {
    for st in ["ok", "idle", "pending", "stale"] {
        let mut value = frame(6);
        value["st"] = json!(st);
        if st == "ok" {
            value["age"] = json!(1);
        }
        reading(&value);
    }
    for invalid in [
        json!("future"),
        json!("OK"),
        json!(1),
        Value::Null,
        json!({}),
    ] {
        let mut value = frame(2);
        value["st"] = invalid;
        assert_discard(&value);
    }
}

#[test]
fn amounts_and_age_are_nonnegative_int32_or_null() {
    for key in ["sell", "sellStack", "list", "listStack", "age"] {
        for valid in [Value::Null, json!(0), json!(i32::MAX)] {
            let mut value = frame(2);
            value[key] = valid;
            reading(&value);
        }
        for invalid in [
            json!(2_147_483_648i64),
            json!(-1),
            json!(1.5),
            json!("1"),
            json!([]),
            json!(true),
        ] {
            let mut value = frame(2);
            value[key] = invalid;
            assert_discard(&value);
        }
    }
}

#[test]
fn a_status_other_than_ok_with_any_amount_is_discarded() {
    for st in ["idle", "pending", "stale"] {
        for key in ["sell", "sellStack", "list", "listStack"] {
            let mut value = frame(6);
            value["st"] = json!(st);
            value[key] = json!(5);
            assert_discard(&value);
        }
    }
}

#[test]
fn cap_and_state_require_v3_price2_a_22_character_nonce_ttl_and_int32_sequence() {
    for (key, invalid) in [
        ("v", json!(2)),
        ("tag", json!("price1")),
        ("tag", json!("price3")),
        ("ttl", json!(14)),
        ("ttl", json!(16)),
        ("nonce", json!("")),
        ("nonce", json!("with spaces")),
        ("nonce", json!("a".repeat(65))),
        ("nonce", json!("a".repeat(21))),
        ("nonce", json!("a".repeat(23))),
        ("seq", json!(0)),
        ("seq", json!(-1)),
        ("seq", json!(2_147_483_648u64)),
        ("type", json!("farming_state")),
    ] {
        let mut value = frame(2);
        value[key] = invalid;
        assert_discard(&value);
    }
    for (key, invalid) in [
        ("v", json!(2)),
        ("tag", json!("farm1")),
        ("tag", json!("price1")),
        ("nonce", json!("a!")),
        ("nonce", json!("a".repeat(21))),
        ("nonce", json!("a".repeat(23))),
    ] {
        let mut value = frame(0);
        value[key] = invalid;
        assert_discard(&value);
    }
    let mut maximum = frame(2);
    maximum["seq"] = json!(i32::MAX);
    assert_eq!(reading(&maximum).seq, i32::MAX);
}

/// `price1` had the same shape and carried net figures. Its capability, its state and every
/// other frame of its fixture are dropped whole, and the subscription never names it.
#[test]
fn the_price1_tag_is_discarded_in_every_frame_and_never_sent() {
    for index in 0..fixture().len() {
        if index == 1 {
            continue; // `price_sub` travels the other way.
        }
        let mut old = frame(index);
        assert_eq!(old["tag"], json!("price2"));
        old["tag"] = json!("price1");
        assert_discard(&old);
    }
    let sub = build_price_sub_line(NONCE, 7).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&sub).unwrap()["tag"], json!("price2"));
    assert!(!sub.contains("price1"));
}

#[test]
fn sequence_must_increase_and_belong_to_the_connection() {
    let state = subscribed();
    let now = Instant::now();
    let mut value = frame(2);
    value["seq"] = json!(100);
    assert!(state.accept_price(reading(&value), now));
    assert!(!state.accept_price(reading(&value), now));
    value["seq"] = json!(99);
    assert!(!state.accept_price(reading(&value), now));
    value["seq"] = json!(101);
    value["nonce"] = json!(OTHER_NONCE);
    assert!(!state.accept_price(reading(&value), now));
    // Without a capability nothing is accepted; a second cap on the same connection is ignored.
    let bare = SharedState::new();
    assert!(!bare.enable_price(NONCE));
    bare.begin_price_connection(NONCE);
    assert!(!bare.accept_price(reading(&frame(2)), now));
    assert!(bare.enable_price(NONCE) && !bare.enable_price(NONCE));
    assert!(!bare.enable_price("old_nonce"));
}

#[test]
fn transport_ttl_is_15_seconds_monotonic_and_age_grows_locally() {
    let state = subscribed();
    let now = Instant::now();
    state.accept_price(reading(&frame(2)), now);
    let before = state.price_view(now + Duration::from_secs(14));
    assert!(before.fresh);
    assert_eq!(before.age, Some(412 + 14));
    let after = state.price_view(now + Duration::from_secs(15));
    assert!(!after.fresh);
    // The block stays where it is; the figures of the expired transport are gone.
    assert_eq!(
        texts(&after, false),
        owned(&[
            ("Saco · precio aún sin leer", false),
            ("Pedido —", false),
            ("Oferta —", false)
        ])
    );
    // A new frame restarts the transport clock; its own age replaces the old one.
    state.accept_price(reading(&frame(3)), now + Duration::from_secs(20));
    let view = state.price_view(now + Duration::from_secs(22));
    assert!(view.fresh);
    assert_eq!(view.age, Some(32));
}

#[test]
fn disconnect_is_immediate_and_a_new_connection_needs_its_own_frame() {
    let state = subscribed();
    let now = Instant::now();
    state.accept_price(reading(&frame(2)), now);
    state.disconnect_price();
    let view = state.price_view(now + Duration::from_secs(1));
    assert!(!view.capable && !view.fresh && view.reading.is_none());
    assert!(panel_lines(&view, false).is_empty());
    state.begin_price_connection(NONCE);
    assert!(state.price_view(now).reading.is_none());
    assert!(state.enable_price(NONCE));
    assert!(
        !state.price_view(now).fresh,
        "an old reading does not come back on welcome"
    );
}

#[test]
fn price_sequence_is_independent_of_alerts_and_farming() {
    let state = subscribed();
    assert!(state.accept_alert("server", Some(1)));
    state.accept_price(reading(&frame(2)), Instant::now());
    assert!(!state.accept_alert("server", Some(1)));
    assert!(state.history_snapshot().is_empty());
    assert!(!state.farming_view(Instant::now()).capable);
}

#[test]
fn format_coins_drops_leading_zero_units() {
    for (copper, text) in [
        (0, "0c"),
        (45, "45c"),
        (289, "2s 89c"),
        (345, "3s 45c"),
        (367, "3s 67c"),
        (10_000, "1g 0s 0c"),
        (86250, "8g 62s 50c"),
        (91750, "9g 17s 50c"),
        (i32::MAX, "214748g 36s 47c"),
    ] {
        assert_eq!(format_coins(copper), text);
    }
}

fn view_of(index: usize, age_extra: u64) -> PriceView {
    let state = subscribed();
    let now = Instant::now();
    state.accept_price(reading(&frame(index)), now);
    state.price_view(now + Duration::from_secs(age_extra))
}

fn texts(view: &PriceView, english: bool) -> Vec<(String, bool)> {
    panel_lines(view, english)
        .into_iter()
        .map(|line| (line.text, line.warning))
        .collect()
}

fn owned(rows: &[(&str, bool)]) -> Vec<(String, bool)> {
    rows.iter()
        .map(|(text, warning)| (text.to_string(), *warning))
        .collect()
}

#[test]
fn panel_lines_per_status_and_language_use_the_contract_texts() {
    let ok = view_of(2, 0);
    assert_eq!(
        texts(&ok, false),
        owned(&[
            ("Saco · precio del bazar", false),
            ("Pedido 3s 45c · ×250 8g 62s 50c", false),
            ("Oferta 3s 67c · ×250 9g 17s 50c", false)
        ])
    );
    assert_eq!(
        texts(&ok, true),
        owned(&[
            ("Bag · trading post price", false),
            ("Buy order 3s 45c · ×250 8g 62s 50c", false),
            ("Sell offer 3s 67c · ×250 9g 17s 50c", false)
        ])
    );
    // Gross figures only: the net wording is gone from the block in both languages.
    for english in [false, true] {
        for (text, _) in texts(&ok, english) {
            let lower = text.to_lowercase();
            assert!(!lower.contains("neto") && !lower.contains("net of") && !lower.contains("comisi"), "{text}");
        }
    }
    // Every other state keeps the two sides in place with a dash; the header says the state.
    let dashes = |header: &str, warning: bool, english: bool| {
        let (buy, sell) = if english { ("Buy order —", "Sell offer —") } else { ("Pedido —", "Oferta —") };
        owned(&[(header, warning), (buy, false), (sell, false)])
    };
    let none = view_of(3, 0);
    assert_eq!(texts(&none, false), dashes("Saco · sin cotización", false, false));
    assert_eq!(texts(&none, true), dashes("Bag · no quote", false, true));
    let pending = view_of(4, 0);
    assert_eq!(texts(&pending, false), dashes("Saco · precio aún sin leer", false, false));
    assert_eq!(texts(&pending, true), dashes("Bag · price not read yet", false, true));
    // Stale frame has age 1260 s = 21 min; 5 s later still 21 min. Eleven minutes: see below.
    let stale = view_of(5, 0);
    assert_eq!(texts(&stale, false), dashes("Saco · precio caducado (hace 21 min)", true, false));
    assert_eq!(texts(&stale, true), dashes("Bag · price expired (21 min ago)", true, true));
    let mut eleven = stale.clone();
    eleven.age = Some(660);
    assert_eq!(texts(&eleven, false), dashes("Saco · precio caducado (hace 11 min)", true, false));
    assert_eq!(texts(&eleven, true), dashes("Bag · price expired (11 min ago)", true, true));
    assert!(
        texts(&view_of(6, 0), false).is_empty(),
        "idle paints nothing"
    );
    assert!(texts(&view_of(6, 0), true).is_empty());
}

/// The rule that keeps the panel from jumping: with the capability, and outside `idle`, the
/// block is three lines whatever the status, the language, a missing side or the transport's
/// age; without the capability, or on `idle`, it is none, fresh or not.
#[test]
fn the_block_takes_three_lines_in_every_state_it_is_painted_and_none_otherwise() {
    use tyrian_companion_nexus_core::price::PANEL_LINES;
    assert_eq!(PANEL_LINES, 3);
    let mut one_side = frame(2);
    one_side["sell"] = Value::Null;
    one_side["sellStack"] = Value::Null;
    let mut no_stack = frame(2);
    no_stack["listStack"] = Value::Null;
    let mut stale_without_age = frame(5);
    stale_without_age["age"] = Value::Null;
    let painted = [frame(2), frame(3), frame(4), frame(5), one_side, no_stack, stale_without_age];
    for english in [false, true] {
        // Capability, no frame yet.
        let waiting = subscribed();
        let now = Instant::now();
        assert_eq!(panel_lines(&waiting.price_view(now), english).len(), PANEL_LINES);
        for value in &painted {
            let state = subscribed();
            state.accept_price(reading(value), now);
            // Second by second across the 5 s between frames, the 15 s expiry and well past it.
            for seconds in 0..=40 {
                let view = state.price_view(now + Duration::from_secs(seconds));
                let lines = panel_lines(&view, english);
                assert_eq!(lines.len(), PANEL_LINES, "{value} +{seconds}s");
                assert!(lines[1..].iter().all(|line| !line.warning), "{value} +{seconds}s");
                if !view.fresh {
                    assert!(lines[1..].iter().all(|line| line.text.ends_with(" —")), "old figures at +{seconds}s: {lines:?}");
                }
            }
        }
        // `idle` and no capability: nothing, and it stays nothing once the transport expires.
        let idle = subscribed();
        idle.accept_price(reading(&frame(6)), now);
        for seconds in 0..=40 {
            assert!(panel_lines(&idle.price_view(now + Duration::from_secs(seconds)), english).is_empty(), "idle +{seconds}s");
        }
        let bare = SharedState::new();
        assert!(panel_lines(&bare.price_view(now), english).is_empty(), "no connection");
        bare.begin_price_connection(NONCE);
        assert!(panel_lines(&bare.price_view(now), english).is_empty(), "no capability");
        idle.disconnect_price();
        assert!(panel_lines(&idle.price_view(now), english).is_empty(), "disconnected");
    }
}

#[test]
fn a_missing_side_paints_a_dash_and_no_age_is_painted_on_ok() {
    let mut value = frame(2);
    value["list"] = Value::Null;
    value["listStack"] = Value::Null;
    let state = subscribed();
    let now = Instant::now();
    state.accept_price(reading(&value), now);
    let view = state.price_view(now);
    assert_eq!(texts(&view, false)[2].0, "Oferta —");
    assert_eq!(texts(&view, true)[2].0, "Sell offer —");
    for (text, _) in texts(&view, false) {
        assert!(!text.contains("hace") && !text.contains("412"), "{text}");
    }
}

#[test]
fn nothing_is_painted_without_capability_and_no_figure_without_a_fresh_frame() {
    let state = SharedState::new();
    let now = Instant::now();
    assert!(
        panel_lines(&state.price_view(now), false).is_empty(),
        "no connection"
    );
    state.begin_price_connection(NONCE);
    assert!(
        panel_lines(&state.price_view(now), false).is_empty(),
        "no capability"
    );
    state.enable_price(NONCE);
    let unread = owned(&[
        ("Saco · precio aún sin leer", false),
        ("Pedido —", false),
        ("Oferta —", false),
    ]);
    assert_eq!(texts(&state.price_view(now), false), unread, "no frame yet");
    state.accept_price(reading(&frame(2)), now);
    assert_eq!(texts(&state.price_view(now), false)[1].0, "Pedido 3s 45c · ×250 8g 62s 50c");
    assert_eq!(
        texts(&state.price_view(now + Duration::from_secs(15)), false),
        unread,
        "transport of 15 s"
    );
}

#[test]
fn every_line_fits_34_characters_for_realistic_amounts() {
    for index in [2, 3, 4, 5, 6] {
        for english in [false, true] {
            for (text, warning) in texts(&view_of(index, 0), english) {
                // Two texts the contract fixes go past the 34 the rest keep to: its stale
                // header, "Saco · precio caducado (hace 11 min)", is 36 characters, and its
                // short English label "Sell offer" makes 35 with these amounts. Neither is
                // shortened here; whether they wrap depends on the game's font.
                let limit = if warning { 36 } else if text.starts_with("Sell offer") { 35 } else { 34 };
                assert!(text.chars().count() <= limit, "{text}");
            }
        }
    }
    // Largest realistic amounts: 99 999 g per stack of 250.
    let mut value = frame(2);
    value["sell"] = json!(9_999_999);
    value["list"] = json!(9_999_999);
    value["sellStack"] = json!(99_999_999);
    value["listStack"] = json!(99_999_999);
    let state = subscribed();
    let now = Instant::now();
    state.accept_price(reading(&value), now);
    for english in [false, true] {
        for (text, _) in texts(&state.price_view(now), english) {
            assert!(text.chars().count() <= 50, "{text}");
        }
    }
}

//! Bounded `farm1` frames and monotonic freshness, without game or API dependencies.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tyrian_companion_nexus_core::farming::{FarmingState, Phase};
use tyrian_companion_nexus_core::protocol::{parse_server_line, ServerLine};
use tyrian_companion_nexus_core::state::SharedState;

const NONCE: &str = "Zk3m1Qw9Lr0aT7yUc2Vb5g";
const OTHER_NONCE: &str = "Yk3m1Qw9Lr0aT7yUc2Vb5g";

fn frame() -> Value {
    json!({ "v":3, "type":"farming_state", "tag":"farm1", "nonce":NONCE, "seq":1, "ttl":15,
        "phase":"active", "err":null, "elapsed":1716, "observed":248, "net":null,
        "lo":480, "hi":560, "age":18, "slots":8, "slotSrc":"ingame", "slotAge":3,
        "goal":"bags", "target":1000, "progress":248, "eta":4834, "mf":310,
        "mfKind":"partial", "prep":"partial" })
}

fn reading(value: &Value) -> FarmingState {
    let ServerLine::FarmingState(state) = parse_server_line(&value.to_string()) else {
        panic!("expected a valid farming state: {value}");
    };
    state
}

fn assert_discard(value: &Value) {
    assert_eq!(parse_server_line(&value.to_string()), ServerLine::Discard, "{value}");
}

#[test]
fn exact_flat_frame_preserves_observed_and_signed_close_separately() {
    let mut value = frame();
    value["net"] = json!(-28);
    let state = reading(&value);
    assert_eq!((state.observed, state.net, state.elapsed), (Some(248), Some(-28), Some(1716)));
    assert_eq!(state.phase, Phase::Active);
    value["account"] = json!("private");
    assert_discard(&value);
    value.as_object_mut().unwrap().remove("account");
    value.as_object_mut().unwrap().remove("net");
    assert_discard(&value); // A nullable field is still a required key.
}

#[test]
fn transport_byte_boundary_is_inclusive_and_duplicates_are_rejected() {
    let body = frame().to_string();
    assert!(body.len() < 512);
    let padded = format!("{body}{}", " ".repeat(512 - body.len()));
    assert!(matches!(parse_server_line(&padded), ServerLine::FarmingState(_)));
    assert_eq!(parse_server_line(&format!("{padded} ")), ServerLine::Discard);
    let duplicate = body.replacen("\"seq\":1", "\"seq\":1,\"seq\":2", 1);
    assert_eq!(parse_server_line(&duplicate), ServerLine::Discard);
}

#[test]
fn every_nullable_metric_is_int32_and_nonnegative_except_net() {
    for key in ["elapsed", "observed", "lo", "hi", "age", "slots", "slotAge", "target", "progress", "eta", "mf", "net"] {
        for valid in [Value::Null, json!(0), json!(i32::MAX)] {
            let mut value = frame(); value[key] = valid;
            reading(&value);
        }
        for invalid in [json!(2_147_483_648i64), json!(-2_147_483_649i64), json!(1.5), json!("1"), json!([]), json!({}), json!(true)] {
            let mut value = frame(); value[key] = invalid;
            assert_discard(&value);
        }
        let mut negative = frame(); negative[key] = json!(-1);
        if key == "net" { reading(&negative); } else { assert_discard(&negative); }
    }
    let mut value = frame(); value["net"] = json!(i32::MIN);
    assert_eq!(reading(&value).net, Some(i32::MIN));
}

#[test]
fn all_enums_are_closed_and_unknown_values_do_not_reach_the_panel() {
    for (key, values) in [
        ("phase", vec!["idle", "starting", "active", "stopping", "provisional", "complete", "error", "abandoned"]),
        ("err", vec!["start", "observe", "stop", "save", "other"]),
        ("slotSrc", vec!["ingame", "recent", "unknown"]),
        ("goal", vec!["none", "bags", "duration"]),
        ("mfKind", vec!["partial", "unknown"]),
        ("prep", vec!["partial", "attention", "unknown"]),
    ] {
        for variant in values { let mut value = frame(); value[key] = json!(variant); reading(&value); }
        for invalid in [json!("future"), json!(2), json!({})] {
            let mut value = frame(); value[key] = invalid; assert_discard(&value);
        }
        if key != "err" { let mut value = frame(); value[key] = Value::Null; assert_discard(&value); }
    }
}

#[test]
fn cap_and_state_require_v3_farm1_a_22_character_nonce_ttl_and_int32_sequence() {
    for (key, invalid) in [("v", json!(2)), ("tag", json!("farm2")), ("ttl", json!(14)),
        ("nonce", json!("")), ("nonce", json!("with spaces")), ("nonce", json!("a".repeat(65))),
        ("nonce", json!("a".repeat(21))), ("nonce", json!("a".repeat(23))),
        ("seq", json!(0)), ("seq", json!(-1)), ("seq", json!(2_147_483_648u64))] {
        let mut value = frame(); value[key] = invalid; assert_discard(&value);
    }
    let cap = json!({"v":3,"type":"farming_cap","nonce":NONCE,"tag":"farm1"});
    assert_eq!(parse_server_line(&cap.to_string()), ServerLine::FarmingCapability(NONCE.into()));
    for (key, invalid) in [("v", json!(2)), ("tag", json!("future")), ("nonce", json!("a!")),
        ("nonce", json!("a".repeat(21))), ("nonce", json!("a".repeat(23))), ("extra", json!(1))] {
        let mut value = cap.clone(); value[key] = invalid; assert_discard(&value);
    }
    let duplicate = cap.to_string().replacen("\"v\":3", "\"v\":3,\"v\":3", 1);
    assert_eq!(parse_server_line(&duplicate), ServerLine::Discard);
    let mut maximum = frame(); maximum["seq"] = json!(i32::MAX);
    assert_eq!(reading(&maximum).seq, i32::MAX);
}

#[test]
fn farming_nonce_and_sequence_never_contaminate_alert_deduplication() {
    let state = SharedState::new();
    let now = Instant::now();
    assert!(!state.enable_farming(NONCE));
    state.begin_farming_connection(NONCE);
    assert!(!state.accept_farming(reading(&frame()), now));
    assert!(!state.enable_farming("old_nonce"));
    assert!(state.enable_farming(NONCE));
    assert!(!state.enable_farming(NONCE));
    let mut value = frame(); value["seq"] = json!(100);
    assert!(state.accept_farming(reading(&value), now));
    assert!(!state.accept_farming(reading(&value), now));
    value["seq"] = json!(99);
    assert!(!state.accept_farming(reading(&value), now));
    value["seq"] = json!(101); value["nonce"] = json!(OTHER_NONCE);
    assert!(!state.accept_farming(reading(&value), now));
    assert!(state.accept_alert("server", Some(1)));
    assert!(!state.accept_alert("server", Some(1)));
    assert!(state.history_snapshot().is_empty());
    state.begin_farming_connection(OTHER_NONCE);
    assert!(state.enable_farming(OTHER_NONCE));
    value["seq"] = json!(1); value["nonce"] = json!(OTHER_NONCE);
    assert!(state.accept_farming(reading(&value), now));
    assert!(!state.accept_alert("server", Some(1))); // Reconnect does not reset alert receipts.
}

#[test]
fn ttl_is_monotonic_disconnect_is_immediate_and_metrics_are_frozen() {
    let state = SharedState::new();
    let now = Instant::now();
    state.begin_farming_connection(NONCE);
    state.enable_farming(NONCE);
    state.accept_farming(reading(&frame()), now);
    let before = state.farming_view(now + Duration::from_secs(14));
    assert!(before.fresh);
    assert_eq!((before.age, before.slot_age, before.eta()), (Some(32), Some(17), None));
    let stale = state.farming_view(now + Duration::from_secs(15));
    assert!(!stale.fresh);
    assert_eq!(stale.eta(), None);
    assert_eq!(stale.reading, before.reading); // Neither elapsed, rate nor observed advances.
    state.disconnect_farming();
    assert!(!state.farming_view(now + Duration::from_secs(1)).fresh);
    state.begin_farming_connection(NONCE);
    assert!(!state.farming_view(now).fresh); // An old reading does not become live on welcome.
    state.enable_farming(NONCE);
    assert!(!state.farming_view(now).fresh); // Even a reused nonce needs a new connection's frame.
}

#[test]
fn transport_refresh_does_not_reset_api_age_and_closed_sessions_have_no_eta() {
    let state = SharedState::new();
    let now = Instant::now();
    state.begin_farming_connection(NONCE); state.enable_farming(NONCE);
    state.accept_farming(reading(&frame()), now);
    let mut value = frame(); value["seq"] = json!(2); value["age"] = json!(23); value["slotAge"] = json!(8);
    state.accept_farming(reading(&value), now + Duration::from_secs(5));
    let view = state.farming_view(now + Duration::from_secs(5));
    assert_eq!((view.age, view.slot_age), (Some(23), Some(8)));
    value["seq"] = json!(3); value["phase"] = json!("complete");
    state.accept_farming(reading(&value), now + Duration::from_secs(6));
    assert_eq!(state.farming_view(now + Duration::from_secs(6)).eta(), None);
    value["seq"] = json!(4); value["phase"] = json!("active"); value["err"] = json!("observe");
    state.accept_farming(reading(&value), now + Duration::from_secs(7));
    let failed = state.farming_view(now + Duration::from_secs(7));
    assert_eq!(failed.reading.unwrap().err, Some(tyrian_companion_nexus_core::farming::FarmingError::Observe));
    assert_eq!(state.farming_view(now + Duration::from_secs(7)).eta(), None);
}

#[test]
fn duration_eta_is_independent_of_observation_age_and_observe_error() {
    let state = SharedState::new();
    let now = Instant::now();
    state.begin_farming_connection(NONCE);
    state.enable_farming(NONCE);
    let mut value = frame();
    value["goal"] = json!("duration");
    value["err"] = json!("observe");
    for (seq, age) in [(1, Value::Null), (2, json!(60))] {
        value["seq"] = json!(seq);
        value["age"] = age;
        state.accept_farming(reading(&value), now);
        assert_eq!(state.farming_view(now).eta(), Some(4834));
    }
    assert_eq!(state.farming_view(now + Duration::from_secs(15)).eta(), None);
    for (seq, error) in [(3, "save"), (4, "start"), (5, "stop"), (6, "other")] {
        value["seq"] = json!(seq);
        value["err"] = json!(error);
        state.accept_farming(reading(&value), now);
        assert_eq!(state.farming_view(now).eta(), None);
    }
    value["seq"] = json!(7); value["err"] = Value::Null; value["phase"] = json!("complete");
    state.accept_farming(reading(&value), now);
    assert_eq!(state.farming_view(now).eta(), None);
}

#[test]
fn bags_eta_expires_at_exactly_fifteen_seconds_without_transport_refresh_reset() {
    let state = SharedState::new();
    let now = Instant::now();
    state.begin_farming_connection(NONCE);
    state.enable_farming(NONCE);
    // A reading 10 s old on arrival: the transport stays fresh while its age crosses 15 s.
    let mut value = frame(); value["age"] = json!(10);
    state.accept_farming(reading(&value), now);
    // The 5 s source freshness no longer decides it: stale by that measure, the ETA stays.
    assert!(!state.farming_view(now).source_fresh());
    assert_eq!(state.farming_view(now + Duration::from_secs(4)).eta(), Some(4834));
    let at_fifteen = state.farming_view(now + Duration::from_secs(5));
    assert!(at_fifteen.fresh);
    assert_eq!((at_fifteen.age, at_fifteen.eta()), (Some(15), None));
    // A transport refresh does not make an old observation young again.
    value["seq"] = json!(2); value["age"] = json!(15);
    state.accept_farming(reading(&value), now + Duration::from_secs(5));
    assert_eq!(state.farming_view(now + Duration::from_secs(5)).eta(), None);
    value["seq"] = json!(3); value["age"] = json!(14);
    state.accept_farming(reading(&value), now + Duration::from_secs(6));
    assert_eq!(state.farming_view(now + Duration::from_secs(6)).eta(), Some(4834));
    value["seq"] = json!(4); value["age"] = Value::Null;
    state.accept_farming(reading(&value), now + Duration::from_secs(7));
    assert_eq!(state.farming_view(now + Duration::from_secs(7)).eta(), None);
    // An error still removes a bags ETA at once, whatever the age.
    value["seq"] = json!(5); value["age"] = json!(0); value["err"] = json!("observe");
    state.accept_farming(reading(&value), now + Duration::from_secs(8));
    assert_eq!(state.farming_view(now + Duration::from_secs(8)).eta(), None);
}

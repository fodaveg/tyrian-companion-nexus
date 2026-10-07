//! When the panel paints "last reading ago Xs" and the inventory footer.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tyrian_companion_nexus_core::farming::{show_inventory_status, FarmingView};
use tyrian_companion_nexus_core::live::LiveStatus;
use tyrian_companion_nexus_core::protocol::{parse_server_line, ServerLine};
use tyrian_companion_nexus_core::state::SharedState;

const NONCE: &str = "Zk3m1Qw9Lr0aT7yUc2Vb5g";

fn frame(phase: &str, err: Value, age: Value) -> Value {
    json!({ "v":3, "type":"farming_state", "tag":"farm1", "nonce":NONCE, "seq":1, "ttl":15,
        "phase":phase, "err":err, "elapsed":1716, "observed":248, "net":null,
        "lo":480, "hi":560, "age":age, "slots":8, "slotSrc":"ingame", "slotAge":3,
        "goal":"bags", "target":1000, "progress":248, "eta":4834, "mf":310,
        "mfKind":"partial", "prep":"partial" })
}

/// The view `local` seconds after a frame with source age `age` arrived.
fn view(phase: &str, err: Value, age: Value, local: u64) -> FarmingView {
    let ServerLine::FarmingState(reading) = parse_server_line(&frame(phase, err, age).to_string())
    else {
        panic!()
    };
    let state = SharedState::new();
    let now = Instant::now();
    state.begin_farming_connection(NONCE);
    state.enable_farming(NONCE);
    state.accept_farming(reading, now);
    state.farming_view(now + Duration::from_secs(local))
}

#[test]
fn reading_age_shows_only_when_it_reports_a_problem_in_a_measuring_phase() {
    // (phase, err, source age, local seconds, expected)
    let rows: &[(&str, Value, Value, u64, bool)] = &[
        // Normal measurement: nothing to say, whatever the phase.
        ("active", Value::Null, json!(4), 0, false),
        ("active", Value::Null, json!(0), 0, false),
        ("active", Value::Null, json!(5), 0, false),
        ("active", Value::Null, json!(14), 0, false),
        ("starting", Value::Null, json!(3), 0, false),
        ("stopping", Value::Null, json!(3), 0, false),
        ("provisional", Value::Null, json!(3), 0, false),
        // 15 s and more is a problem; 14 s is not (no 5 s blinking).
        ("active", Value::Null, json!(15), 0, true),
        ("active", Value::Null, json!(14), 1, true),
        ("active", Value::Null, json!(10), 4, false),
        ("active", Value::Null, json!(10), 5, true),
        // No reading.
        ("active", Value::Null, Value::Null, 0, true),
        ("starting", Value::Null, Value::Null, 0, true),
        // Error set.
        ("active", json!("observe"), json!(1), 0, true),
        ("active", json!("save"), json!(1), 0, true),
        ("provisional", json!("stop"), json!(1), 0, true),
        // Phase `error` is a measuring phase: a quiet reading stays hidden, a missing one shows.
        ("error", Value::Null, json!(1), 0, false),
        ("error", Value::Null, Value::Null, 0, true),
        ("error", json!("other"), json!(1), 0, true),
        // Phases outside the measuring set never show it.
        ("idle", json!("observe"), Value::Null, 0, false),
        ("complete", json!("save"), json!(99), 0, false),
        ("abandoned", Value::Null, Value::Null, 0, false),
        // Transport older than 15 s.
        ("active", Value::Null, json!(0), 15, true),
        ("active", Value::Null, json!(0), 14, false),
        ("complete", Value::Null, json!(0), 15, false),
    ];
    for (phase, err, age, local, expected) in rows {
        let got = view(phase, err.clone(), age.clone(), *local).show_reading_age();
        assert_eq!(got, *expected, "{phase} err={err} age={age} +{local}s");
    }
}

#[test]
fn without_any_reading_nothing_is_shown() {
    let state = SharedState::new();
    assert!(!state.farming_view(Instant::now()).show_reading_age());
}

#[test]
fn the_inventory_footer_hides_only_measuring() {
    for status in [
        LiveStatus::NotNegotiated,
        LiveStatus::Waiting,
        LiveStatus::Partial,
        LiveStatus::UnsupportedBuild,
        LiveStatus::Unavailable,
        LiveStatus::Conflict,
        LiveStatus::StorageUnavailable,
    ] {
        assert!(show_inventory_status(status), "{status:?}");
    }
    assert!(!show_inventory_status(LiveStatus::Measuring));
}

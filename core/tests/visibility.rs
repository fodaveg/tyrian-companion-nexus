//! When the panel says "last reading ago Xs", what the rate line carries and when the ETA holds.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tyrian_companion_nexus_core::farming::FarmingView;
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
    assert!(state.farming_view(Instant::now()).rate_notes(false).is_empty());
}

/// The view `local` after `value` arrived, for times finer than a second.
fn view_after(value: &Value, local: Duration) -> FarmingView {
    let ServerLine::FarmingState(reading) = parse_server_line(&value.to_string()) else {
        panic!("{value}")
    };
    let state = SharedState::new();
    let now = Instant::now();
    state.begin_farming_connection(NONCE);
    state.enable_farming(NONCE);
    state.accept_farming(reading, now);
    state.farming_view(now + local)
}

/// The host sends a frame every 5 s and the reader samples once a second, so a frame arrives
/// with `age` 0, 1 or 2 and the next one 5 s later, give or take the 250 ms read poll. Across
/// that whole cycle there is nothing to say about the rate: no note, so no colour. The 5 s
/// freshness that used to paint "Last recorded rate" does flip inside the same cycle, which is
/// why that line came and went.
#[test]
fn normal_measurement_has_no_rate_note_between_two_frames() {
    let mut flips = 0;
    for emitted_age in [0, 1, 2] {
        let value = frame("active", Value::Null, json!(emitted_age));
        let mut fresh_at_start = None;
        for millis in (0..=5_500).step_by(50) {
            let view = view_after(&value, Duration::from_millis(millis));
            for english in [false, true] {
                let notes = view.rate_notes(english);
                assert!(notes.is_empty(), "age {emitted_age} +{millis} ms: {notes:?}");
            }
            let fresh = view.source_fresh();
            if *fresh_at_start.get_or_insert(fresh) != fresh {
                flips += 1;
                break;
            }
        }
    }
    assert_eq!(flips, 3, "the 5 s threshold is crossed in every one of those cycles");
}

/// The same cycle for a bags goal: the ETA line keeps its figure from one frame to the next,
/// second by second and in between, instead of turning into "ETA not available yet" at the
/// tail of each cycle. It goes at 15 s, like the rate's note.
#[test]
fn normal_measurement_keeps_the_bags_eta_between_two_frames() {
    let mut flips = 0;
    for emitted_age in [0, 1, 2] {
        let value = frame("active", Value::Null, json!(emitted_age));
        let mut fresh_at_start = None;
        for millis in (0..=5_500).step_by(50) {
            let view = view_after(&value, Duration::from_millis(millis));
            assert_eq!(view.eta(), Some(4834), "age {emitted_age} +{millis} ms");
            let fresh = view.source_fresh();
            if *fresh_at_start.get_or_insert(fresh) != fresh {
                flips += 1;
            }
        }
        for seconds in 0..15 {
            let expected = if emitted_age + seconds < 15 { Some(4834) } else { None };
            let view = view_after(&value, Duration::from_secs(seconds));
            assert_eq!(view.eta(), expected, "age {emitted_age} +{seconds} s");
        }
    }
    assert!(flips >= 3, "the 5 s threshold is crossed in every one of those cycles");
}

#[test]
fn the_rate_notes_are_what_used_to_be_lines_of_their_own() {
    let notes = |value: &Value, local: u64, english: bool| view_after(value, Duration::from_secs(local)).rate_notes(english);
    let active = |age: Value| frame("active", Value::Null, age);
    // No rate yet.
    let mut no_rate = active(json!(0));
    no_rate["lo"] = Value::Null;
    no_rate["hi"] = Value::Null;
    assert_eq!(notes(&no_rate, 0, false), ["Ritmo aún no disponible"]);
    assert_eq!(notes(&no_rate, 0, true), ["Rate not available yet"]);
    // A reading of 15 s or more: the rate is the last recorded one, and its age is told.
    assert!(notes(&active(json!(14)), 0, false).is_empty());
    assert_eq!(notes(&active(json!(15)), 0, false), ["Último ritmo registrado", "Última lectura hace 15s"]);
    assert_eq!(notes(&active(json!(10)), 7, true), ["Last recorded rate", "Last reading ago: 17s"]);
    // No reading of the source at all.
    assert_eq!(notes(&active(Value::Null), 0, false), ["Último ritmo registrado", "Sin lectura"]);
    assert_eq!(notes(&active(Value::Null), 0, true), ["Last recorded rate", "No reading"]);
    // Transport of 15 s or more.
    assert!(notes(&active(json!(0)), 14, false).is_empty());
    assert_eq!(notes(&active(json!(0)), 15, false), ["Último ritmo registrado", "Última lectura hace 15s"]);
    // An error tells the age even of a young reading; the rate itself is not old.
    assert_eq!(notes(&frame("active", json!("observe"), json!(1)), 0, false), ["Última lectura hace 1s"]);
    // Outside a measuring phase the age is not told, the old rate still is.
    assert_eq!(notes(&frame("complete", Value::Null, json!(99)), 0, false), ["Último ritmo registrado"]);
    assert!(notes(&frame("complete", Value::Null, json!(3)), 0, false).is_empty());
    // Everything at once keeps the order the lines had.
    let mut all = frame("active", json!("observe"), Value::Null);
    all["lo"] = Value::Null;
    assert_eq!(notes(&all, 0, false), ["Ritmo aún no disponible", "Último ritmo registrado", "Sin lectura"]);
}

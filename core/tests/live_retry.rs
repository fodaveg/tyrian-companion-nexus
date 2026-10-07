//! `live_open` is retried after a `source_conflict`; the other refusals stay blocked.
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use tyrian_companion_nexus_core::inventory::InventorySnapshot;
use tyrian_companion_nexus_core::live::*;
use tyrian_companion_nexus_core::protocol::{GameContext, GameState};
const NONCE: &str = "AQEBAQEBAQEBAQEBAQEBAQ";
const WAIT: Duration = CONFLICT_RETRY_INTERVAL;

fn sample(n: u32) -> InventorySnapshot {
    InventorySnapshot {
        owner: (0x10a000, 0x10b000),
        quantities: BTreeMap::from([(12147, n)]),
        unknown: 0,
        positions: 570,
        free_slots: None,
        wallet: None,
    }
}
fn context(map_id: u32) -> GameContext {
    GameContext {
        state: GameState::Gameplay,
        map_id: Some(map_id),
        character: Some("Test Character".into()),
    }
}
fn channel(now: Instant) -> Channel {
    let mut c = Channel::new();
    c.accept(
        Reply::Capability {
            nonce: NONCE.into(),
        },
        NONCE,
        now,
    );
    c.context_changed(&context(866));
    c
}
fn reply(c: &mut Channel, open: &Value, status: &str, now: Instant) {
    c.accept(
        Reply::Ready {
            nonce: NONCE.into(),
            epoch: open["epoch"].as_str().unwrap().into(),
            status: status.into(),
        },
        NONCE,
        now,
    );
}
/// Opens an epoch at `now` and answers it with `status` at the same instant.
fn open_and_answer(c: &mut Channel, status: &str, now: Instant) -> Value {
    let open = c.capture(Ok(sample(0)), 0, now).remove(0);
    assert_eq!(open["type"], "live_open");
    reply(c, &open, status, now);
    open
}

#[test]
fn source_conflict_waits_the_interval_then_opens_again_with_a_new_epoch() {
    let now = Instant::now();
    let mut c = channel(now);
    let first = open_and_answer(&mut c, "source_conflict", now);
    assert_eq!(c.status, LiveStatus::Conflict);
    for secs in [1, 2, 15, 29] {
        assert!(
            !c.wants_sample(now + Duration::from_secs(secs)),
            "no sample {secs} s into the wait"
        );
        assert_eq!(c.status, LiveStatus::Conflict);
    }
    assert!(!c.wants_sample(now + WAIT - Duration::from_millis(1)));
    assert!(c.wants_sample(now + WAIT));
    let retry = now + WAIT;
    let second = c.capture(Ok(sample(5)), 0, retry).remove(0);
    assert_eq!(second["type"], "live_open");
    assert_ne!(second["epoch"], first["epoch"]);
}

#[test]
fn a_retry_answered_ready_measures_normally_from_a_fresh_baseline() {
    let now = Instant::now();
    let mut c = channel(now);
    open_and_answer(&mut c, "source_conflict", now);
    let retry = now + WAIT;
    let open = open_and_answer(&mut c, "ready", retry);
    let frames = c.pending_frames(0, retry).unwrap();
    assert_eq!(frames[0]["mode"], "baseline");
    assert_eq!(frames[0]["cursor"], 0);
    assert_eq!(frames[0]["ms"], 0);
    assert_eq!(frames[0]["epoch"], open["epoch"]);
    assert!(c.accept(
        Reply::Ack {
            nonce: NONCE.into(),
            epoch: open["epoch"].as_str().unwrap().into(),
            cursor: 0,
            status: "stored".into(),
        },
        NONCE,
        retry,
    ));
    assert_eq!(c.status, LiveStatus::Measuring);
    // Back to the normal one-second capture interval, with no 30 s wait left over.
    assert!(!c.wants_sample(retry));
    assert!(c.wants_sample(retry + CAPTURE_INTERVAL));
    let next = c.capture(Ok(sample(3)), 0, retry + CAPTURE_INTERVAL);
    assert_eq!(next[0]["mode"], "sample");
    assert_eq!(next[0]["cursor"], 1);
}

#[test]
fn another_source_conflict_waits_the_interval_again_without_limit() {
    let now = Instant::now();
    let mut c = channel(now);
    let mut at = now;
    for round in 0..5 {
        open_and_answer(&mut c, "source_conflict", at);
        assert_eq!(c.status, LiveStatus::Conflict, "round {round}");
        assert!(!c.wants_sample(at + WAIT - Duration::from_secs(1)));
        assert!(c.wants_sample(at + WAIT), "round {round}");
        at += WAIT;
    }
}

#[test]
fn the_wait_can_be_shortened_for_a_single_channel_only() {
    let now = Instant::now();
    let mut c = channel(now);
    c.set_conflict_retry(Some(Duration::from_millis(200)));
    open_and_answer(&mut c, "source_conflict", now);
    assert!(!c.wants_sample(now + Duration::from_millis(199)));
    assert!(c.wants_sample(now + Duration::from_millis(200)));
    let other = channel(now);
    assert_eq!(other.status, LiveStatus::Waiting);
    let mut other = other;
    open_and_answer(&mut other, "source_conflict", now);
    assert!(!other.wants_sample(now + Duration::from_secs(1)));
}

#[test]
fn unsupported_build_and_not_gameplay_stay_blocked_for_good() {
    for (status, expected) in [
        ("unsupported_build", LiveStatus::UnsupportedBuild),
        ("not_gameplay", LiveStatus::Unavailable),
    ] {
        let now = Instant::now();
        let mut c = channel(now);
        open_and_answer(&mut c, status, now);
        assert_eq!(c.status, expected, "{status}");
        for secs in [1, 30, 31, 3600] {
            assert!(
                !c.wants_sample(now + Duration::from_secs(secs)),
                "{status} must not retry after {secs} s"
            );
        }
        // Only a context change lifts it.
        c.context_changed(&context(1000));
        assert!(c.wants_sample(now + Duration::from_secs(1)), "{status}");
    }
}

#[test]
fn a_context_change_lifts_the_conflict_wait_at_once() {
    let now = Instant::now();
    let mut c = channel(now);
    open_and_answer(&mut c, "source_conflict", now);
    let soon = now + Duration::from_secs(2);
    assert!(!c.wants_sample(soon));
    c.context_changed(&context(1000));
    assert!(c.wants_sample(soon));
    // The same context repeated does not cut the wait.
    let mut c = channel(now);
    open_and_answer(&mut c, "source_conflict", now);
    c.context_changed(&context(866));
    assert!(!c.wants_sample(soon));
}

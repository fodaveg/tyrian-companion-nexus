//! Contract fixtures and continuity controls for the negotiated passive producer.
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use tyrian_companion_nexus_core::inventory::{InventorySnapshot, ReadError};
use tyrian_companion_nexus_core::live::*;
use tyrian_companion_nexus_core::protocol::{is_canonical_instance, GameContext, GameState};
const NONCE: &str = "AQEBAQEBAQEBAQEBAQEBAQ";
const EPOCH: &str = "AgICAgICAgICAgICAgICAg";
fn sample(n: u32) -> InventorySnapshot {
    InventorySnapshot {
        owner: (0x10a000, 0x10b000),
        quantities: BTreeMap::from([(12147, n), (36038, 200)]),
        unknown: 0,
        positions: 570,
        free_slots: None,
    }
}
fn context() -> GameContext {
    GameContext {
        state: GameState::Gameplay,
        map_id: Some(866),
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
    c.context_changed(&context());
    c
}
fn ready(c: &mut Channel, open: &Value, now: Instant) {
    c.accept(
        Reply::Ready {
            nonce: NONCE.into(),
            epoch: open["epoch"].as_str().unwrap().into(),
            status: "ready".into(),
        },
        NONCE,
        now,
    );
}
fn ack(c: &mut Channel, open: &Value, cursor: u64, status: &str, now: Instant) -> bool {
    c.accept(
        Reply::Ack {
            nonce: NONCE.into(),
            epoch: open["epoch"].as_str().unwrap().into(),
            cursor,
            status: status.into(),
        },
        NONCE,
        now,
    )
}
#[test]
fn capability_and_replies_reject_duplicate_extra_wrong_nonce_shape_status_and_limits() {
    let cap = json!({"v":3,"type":"live_cap","nonce":NONCE,"tag":"live1"});
    assert!(parse_reply("live_cap", &cap.to_string()).is_some());
    for line in [
        format!(
            "{{\"v\":3,\"v\":3,\"type\":\"live_cap\",\"nonce\":\"{NONCE}\",\"tag\":\"live1\"}}"
        ),
        cap.to_string().replace("live1", "live2"),
        cap.to_string().replace(NONCE, "short"),
        cap.to_string().replace("3", "2"),
    ] {
        assert!(parse_reply("live_cap", &line).is_none());
    }
    let bad = json!({"v":3,"type":"live_ack","nonce":NONCE,"tag":"live1","epoch":EPOCH,"cursor":MAX_SAFE+1,"status":"stored"});
    assert!(parse_reply("live_ack", &bad.to_string()).is_none());
}
#[test]
fn old_servers_and_wrong_capability_nonce_do_not_read_or_send() {
    let now = Instant::now();
    let mut c = Channel::new();
    c.context_changed(&context());
    assert!(!c.wants_sample(now));
    c.accept(
        Reply::Capability {
            nonce: EPOCH.into(),
        },
        NONCE,
        now,
    );
    assert!(!c.enabled());
}
#[test]
fn canonical_baseline_then_zero_two_four_same_epoch_and_shared_safe_lines() {
    let now = Instant::now();
    let mut c = channel(now);
    let open = c.capture(Ok(sample(0)), 0, now).remove(0);
    assert!(is_canonical_instance(open["epoch"].as_str().unwrap()));
    assert!(!c.wants_sample(now + Duration::from_secs(1)));
    ready(&mut c, &open, now);
    let frames = c.pending_frames(0, now).unwrap();
    assert_eq!(frames[0]["mode"], "baseline");
    assert_eq!(frames[1]["rows"], json!([[0, 12147, 0], [0, 36038, 200]]));
    assert!(!c.wants_sample(now + Duration::from_secs(1)));
    assert!(ack(&mut c, &open, 0, "stored", now));
    for (cursor, n) in [(1, 2), (2, 4)] {
        let time = now + Duration::from_secs(cursor);
        assert!(c.wants_sample(time));
        let frames = c.capture(Ok(sample(n)), 0, time);
        assert_eq!(frames[0]["cursor"], cursor);
        assert_eq!(frames[0]["ms"], cursor * 1000);
        assert_eq!(frames[0]["mode"], "sample");
        for (i, f) in frames.into_iter().enumerate() {
            let line = frame_line(f, NONCE, cursor * 10 + i as u64).unwrap();
            assert!(line.trim_end().len() <= 512);
        }
        assert!(ack(&mut c, &open, cursor, "stored", time));
    }
}
#[test]
fn owner_character_map_loading_reconnect_and_read_failure_all_require_baseline() {
    let now = Instant::now();
    let mut c = channel(now);
    let open = c.capture(Ok(sample(0)), 0, now).remove(0);
    ready(&mut c, &open, now);
    c.pending_frames(0, now);
    ack(&mut c, &open, 0, "stored", now);
    let mut changed = sample(2);
    changed.owner.0 += 8;
    let owner_open = c
        .capture(Ok(changed), 0, now + Duration::from_secs(1))
        .remove(0);
    assert_eq!(owner_open["type"], "live_open");
    assert_ne!(owner_open["epoch"], open["epoch"]);
    let mut ctx = context();
    ctx.map_id = Some(15);
    c.context_changed(&ctx);
    let map_open = c
        .capture(Ok(sample(4)), 1, now + Duration::from_secs(2))
        .remove(0);
    assert_eq!(map_open["type"], "live_open");
    ctx.character = Some("Other Character".into());
    c.context_changed(&ctx);
    let char_open = c
        .capture(Ok(sample(4)), 2, now + Duration::from_secs(3))
        .remove(0);
    assert_ne!(char_open["epoch"], map_open["epoch"]);
    ctx.state = GameState::Loading;
    c.context_changed(&ctx);
    assert!(!c.wants_sample(now));
    let status = c.gameplay_status().remove(0);
    assert_eq!(status["reason"], "not_gameplay");
    assert_eq!(status["epoch"], char_open["epoch"]);
    c.context_changed(&context());
    let failed = c.capture(Err(ReadError::ReadFailed), 3, now + Duration::from_secs(4));
    assert_eq!(failed[0]["reason"], "read_failed");
    assert_eq!(failed[0]["epoch"], char_open["epoch"]);
    let recovered = c.capture(Ok(sample(4)), 3, now + Duration::from_secs(5));
    assert_eq!(recovered[0]["type"], "live_open");
    let mut reconnected = channel(now);
    assert_eq!(
        reconnected.capture(Ok(sample(4)), 0, now)[0]["type"],
        "live_open"
    );
}
#[test]
fn unavailable_status_before_any_open_has_null_epoch_and_invalidated_ready_is_ignored() {
    let now = Instant::now();
    let mut c = channel(now);
    assert_eq!(
        c.capture(Err(ReadError::ReadFailed), 0, now)[0]["epoch"],
        Value::Null
    );
    let open = c
        .capture(Ok(sample(0)), 0, now + CAPTURE_INTERVAL)
        .remove(0);
    let mut ctx = context();
    ctx.state = GameState::Loading;
    c.context_changed(&ctx);
    ready(&mut c, &open, now);
    assert!(c.pending_frames(1, now).is_none());
    assert_eq!(c.gameplay_status()[0]["epoch"], open["epoch"]);
    c.context_changed(&context());
    let recovered = c
        .capture(Ok(sample(2)), 2, now + CAPTURE_INTERVAL * 2)
        .remove(0);
    assert_ne!(recovered["epoch"], open["epoch"]);
    ready(&mut c, &open, now);
    assert!(c.pending_frames(2, now).is_none());
    ready(&mut c, &recovered, now);
    assert_eq!(c.pending_frames(2, now).unwrap()[0]["mode"], "baseline");
    let mut never_opened = channel(now);
    never_opened.context_changed(&ctx);
    assert_eq!(never_opened.gameplay_status()[0]["epoch"], Value::Null);
}
#[test]
fn identical_periodic_context_preserves_epoch_and_batch() {
    let now = Instant::now();
    let mut c = channel(now);
    let open = c.capture(Ok(sample(0)), 0, now).remove(0);
    c.context_changed(&context());
    ready(&mut c, &open, now);
    let frames = c.pending_frames(7, now).unwrap();
    assert_eq!(frames[0]["epoch"], open["epoch"]);
    assert_eq!(frames[0]["ctx"], 7);
}
#[test]
fn no_ack_timeout_failed_storage_and_conflicting_source_are_explicit() {
    let now = Instant::now();
    let mut c = channel(now);
    let open = c.capture(Ok(sample(0)), 0, now).remove(0);
    assert!(c.timed_out(now + RESPONSE_TIMEOUT));
    ready(&mut c, &open, now);
    c.pending_frames(0, now);
    assert!(!c.timed_out(now));
    assert!(c.timed_out(now + RESPONSE_TIMEOUT));
    assert!(!ack(&mut c, &open, 0, "storage_unavailable", now));
    assert_eq!(c.status, LiveStatus::StorageUnavailable);
    let mut c = channel(now);
    let open = c.capture(Ok(sample(0)), 0, now).remove(0);
    c.accept(
        Reply::Ready {
            nonce: NONCE.into(),
            epoch: open["epoch"].as_str().unwrap().into(),
            status: "source_conflict".into(),
        },
        NONCE,
        now,
    );
    assert_eq!(c.status, LiveStatus::Conflict);
    assert!(!c.wants_sample(now + Duration::from_secs(1)));
}
#[test]
fn partial_sample_is_inspectable_but_requires_new_epoch_after_ack() {
    let now = Instant::now();
    let mut c = channel(now);
    let mut s = sample(0);
    s.unknown = 1;
    let open = c.capture(Ok(s), 0, now).remove(0);
    ready(&mut c, &open, now);
    let frames = c.pending_frames(0, now).unwrap();
    assert_eq!(frames[0]["items"], "partial");
    assert_eq!(frames[0]["currencies"], "none");
    assert_eq!(frames[0]["slots"], Value::Null);
    ack(&mut c, &open, 0, "stored", now);
    assert_eq!(c.status, LiveStatus::Partial);
    assert_eq!(
        c.capture(Ok(sample(2)), 0, now + Duration::from_secs(1))[0]["type"],
        "live_open"
    );
}
#[test]
fn worst_rows_sequence_and_sample_memory_stay_bounded() {
    let s = InventorySnapshot {
        quantities: (1..=640)
            .map(|n| (i32::MAX as u32 - n, i32::MAX as u32))
            .collect(),
        ..sample(0)
    };
    let frames = snapshot_frames(EPOCH, MAX_SAFE, MAX_SAFE, MAX_SAFE, &s).unwrap();
    assert_eq!(frames.len(), 82);
    let bytes: usize = frames
        .into_iter()
        .map(|f| frame_line(f, NONCE, MAX_SAFE).unwrap().len())
        .sum();
    assert!(bytes < 256 * 1024);
    assert!(frame_line(json!({"type":"live_end"}), NONCE, MAX_SAFE + 1).is_none());
}

#[test]
fn shared_wire_fixtures_match_codec_and_final_lengths() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/live1.json")).unwrap();
    for expected in fixture["frames"].as_array().unwrap() {
        let kind = expected["type"].as_str().unwrap();
        if matches!(kind, "live_cap" | "live_ready" | "live_ack") {
            assert!(parse_reply(kind, &expected.to_string()).is_some());
        } else {
            let mut payload = expected.clone();
            let map = payload.as_object_mut().unwrap();
            for key in ["v", "tag", "nonce", "seq"] {
                map.remove(key);
            }
            let line = frame_line(
                payload,
                expected["nonce"].as_str().unwrap(),
                expected["seq"].as_u64().unwrap(),
            )
            .unwrap();
            assert_eq!(serde_json::from_str::<Value>(&line).unwrap(), *expected);
            assert!(line.trim_end().len() <= 512);
        }
    }
}
#[test]
fn cap_512_bytes_accepted_513_rejected_and_crlf_preserved() {
    let line = json!({"v":3,"type":"live_cap","nonce":NONCE,"tag":"live1"}).to_string();
    let padded = format!("{line}{}", " ".repeat(512 - line.len()));
    assert!(parse_reply("live_cap", &padded).is_some());
    assert!(parse_reply("live_cap", &format!("{padded} ")).is_none());
    assert!(parse_reply("live_cap", &format!("{padded}\r")).is_some());
}
#[test]
fn unknown_build_blocks_until_actual_context_change_and_duplicate_ack_does_not_release_next_sample()
{
    let now = Instant::now();
    let mut c = channel(now);
    let failure = c.capture(Err(ReadError::UnsupportedBuild), 0, now);
    assert_eq!(failure[0]["epoch"], Value::Null);
    assert!(!c.wants_sample(now + Duration::from_secs(1)));
    let mut c = channel(now);
    let open = c.capture(Ok(sample(0)), 0, now).remove(0);
    ready(&mut c, &open, now);
    c.pending_frames(0, now);
    ack(&mut c, &open, 0, "stored", now);
    c.capture(Ok(sample(2)), 0, now + Duration::from_secs(1));
    ack(&mut c, &open, 0, "stored", now + Duration::from_secs(1));
    assert!(!c.wants_sample(now + Duration::from_secs(2)));
    ack(&mut c, &open, 1, "stored", now + Duration::from_secs(1));
    assert!(c.wants_sample(now + Duration::from_secs(2)));
}

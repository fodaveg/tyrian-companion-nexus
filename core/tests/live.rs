//! Contract fixtures and continuity controls for the negotiated passive producer.
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use tyrian_companion_nexus_core::inventory::{InventorySnapshot, ReadError};
use tyrian_companion_nexus_core::live::*;
use tyrian_companion_nexus_core::protocol::{is_canonical_instance, GameContext, GameState};
use tyrian_companion_nexus_core::wallet::{WalletSnapshot, MAX_CURRENCIES};
const NONCE: &str = "AQEBAQEBAQEBAQEBAQEBAQ";
const EPOCH: &str = "AgICAgICAgICAgICAgICAg";
fn sample(n: u32) -> InventorySnapshot {
    InventorySnapshot {
        owner: (0x10a000, 0x10b000),
        quantities: BTreeMap::from([(12147, n), (36038, 200)]),
        unknown: 0,
        positions: 570,
        free_slots: None,
        wallet: None,
    }
}
const WALLET_OWNER: (u64, u64) = (0x210000, 0x220000);
fn wallet(owner: (u64, u64), balances: &[(u32, u32)]) -> Option<WalletSnapshot> {
    Some(WalletSnapshot::checked(owner, balances.iter().copied().collect()).unwrap())
}
/// The canonical inventory sample plus gold (1) at zero and volatile magic (45).
fn with_wallet(n: u32, magic: u32) -> InventorySnapshot {
    InventorySnapshot {
        wallet: wallet(WALLET_OWNER, &[(45, magic), (1, 0)]),
        ..sample(n)
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
/// For the reader diagnostics: every `live_open` is one epoch opened, and nothing else is.
#[test]
fn every_live_open_is_counted_as_one_epoch_opened_and_nothing_else_is() {
    let now = Instant::now();
    let mut c = channel(now);
    assert_eq!(c.epochs_opened(), 0);
    let open = c.capture(Ok(sample(0)), 0, now).remove(0);
    assert_eq!((open["type"].as_str(), c.epochs_opened()), (Some("live_open"), 1));
    ready(&mut c, &open, now);
    c.pending_frames(0, now);
    ack(&mut c, &open, 0, "stored", now);
    // Samples of the same epoch open none.
    for cursor in 1..=3 {
        let time = now + Duration::from_secs(cursor);
        assert_eq!(c.capture(Ok(sample(cursor as u32)), 0, time)[0]["type"], "live_begin");
        ack(&mut c, &open, cursor, "stored", time);
        assert_eq!(c.epochs_opened(), 1);
    }
    // A capture that fails cuts the epoch and opens none; the one after it opens the next.
    let failed = c.capture(Err(ReadError::ReadFailed), 0, now + Duration::from_secs(4));
    assert_eq!((failed[0]["type"].as_str(), c.epochs_opened()), (Some("live_status"), 1));
    assert_eq!(c.capture(Ok(sample(4)), 0, now + Duration::from_secs(5))[0]["type"], "live_open");
    assert_eq!(c.epochs_opened(), 2);
    // And so does a change of context.
    let mut other_map = context();
    other_map.map_id = Some(15);
    c.context_changed(&other_map);
    assert_eq!(c.epochs_opened(), 2);
    assert_eq!(c.capture(Ok(sample(4)), 1, now + Duration::from_secs(6))[0]["type"], "live_open");
    assert_eq!(c.epochs_opened(), 3);
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
/// A source that has been getting ready for too long says so as a reading it could not
/// complete: the line a failed capture sends, once. What it must not do is what a failed
/// capture does next, put the following sample off a second: nothing was sampled, and the
/// source has to be asked again on the next pass for what it is getting ready to go on.
#[test]
fn an_overdue_source_says_read_failed_once_and_is_asked_again_on_the_next_pass() {
    let now = Instant::now();
    let mut c = channel(now);
    assert_eq!(c.status, LiveStatus::Waiting);
    assert_eq!(
        c.overdue(),
        vec![json!({"type":"live_status","epoch":null,"status":"unavailable","reason":"read_failed"})]
    );
    assert_eq!(c.status, LiveStatus::Unavailable);
    // Pass after pass while it is still getting ready: nothing more to say, and still asked.
    for pass in 0..40 {
        assert!(c.wants_sample(now + Duration::from_millis(250) * pass), "pass {pass}");
        assert_eq!(c.overdue(), Vec::<Value>::new(), "pass {pass}");
    }
    assert_eq!(c.epochs_opened(), 0);
    // A capture that fails is not asked again for a second: that is the difference.
    let mut failed = channel(now);
    assert_eq!(failed.capture(Err(ReadError::ReadFailed), 0, now)[0]["reason"], "read_failed");
    assert!(!failed.wants_sample(now + Duration::from_millis(250)));
    // Ready at last: the first sample opens an epoch and is its baseline, as on any load.
    let later = now + Duration::from_secs(10);
    let open = c.capture(Ok(sample(0)), 0, later).remove(0);
    assert_eq!((open["type"].as_str(), c.status, c.epochs_opened()), (Some("live_open"), LiveStatus::Waiting, 1));
    ready(&mut c, &open, later);
    assert_eq!(c.pending_frames(0, later).unwrap()[0]["mode"], "baseline");
    // A failure of the system once the hash is over is the same reason, already said: only a
    // change of context makes it worth saying again.
    let mut said = channel(now);
    assert_eq!(said.overdue().len(), 1);
    assert_eq!(said.capture(Err(ReadError::ReadFailed), 0, now), Vec::<Value>::new());
    let mut elsewhere = context();
    elsewhere.map_id = Some(50);
    said.context_changed(&elsewhere);
    assert_eq!(said.overdue()[0]["reason"], "read_failed");
    // And another build, when that is what the hash comes to, is still said.
    let mut other = channel(now);
    assert_eq!(other.overdue().len(), 1);
    assert_eq!(other.capture(Err(ReadError::UnsupportedBuild), 0, now)[0]["reason"], "unsupported_build");
    assert_eq!(other.status, LiveStatus::UnsupportedBuild);
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
/// Every row of a sample, in transmission order.
fn all_rows(frames: &[Value]) -> Vec<Value> {
    frames
        .iter()
        .filter(|frame| frame["type"] == "live_rows")
        .flat_map(|frame| frame["rows"].as_array().unwrap().clone())
        .collect()
}
#[test]
fn listed_wallet_adds_currency_rows_after_the_items_and_none_carries_no_currency_row() {
    let frames = snapshot_frames(EPOCH, 0, 0, 0, &with_wallet(0, 10214)).unwrap();
    assert_eq!(frames.len(), 3);
    assert_eq!(frames[0]["currencies"], "listed");
    assert_eq!(frames[0]["items"], "complete");
    assert_eq!(frames[0]["rows"], 4);
    // A present key at zero is a covered zero; no other currency is implied.
    assert_eq!(
        frames[1]["rows"],
        json!([[0, 12147, 0], [0, 36038, 200], [1, 1, 0], [1, 45, 10214]])
    );
    let frames = snapshot_frames(EPOCH, 0, 0, 0, &sample(0)).unwrap();
    assert_eq!(frames[0]["currencies"], "none");
    assert_eq!(frames[0]["rows"], 2);
    assert!(all_rows(&frames).iter().all(|row| row[0] == 0));
    // An inventory without rows still lists its wallet; `listed` always has a currency row.
    let only_wallet = InventorySnapshot {
        quantities: BTreeMap::new(),
        ..with_wallet(0, 7)
    };
    let frames = snapshot_frames(EPOCH, 0, 0, 0, &only_wallet).unwrap();
    assert_eq!(frames[0]["currencies"], "listed");
    assert_eq!(frames[0]["rows"], 2);
    assert_eq!(all_rows(&frames), vec![json!([1, 1, 0]), json!([1, 45, 7])]);
}
#[test]
fn item_and_currency_rows_keep_one_global_order_across_parts_of_at_most_eight() {
    let s = InventorySnapshot {
        quantities: (1..=13).map(|n| (n * 1000, n)).collect(),
        wallet: wallet(
            WALLET_OWNER,
            &[(83, 9), (2, 0), (45, 5), (1, 1), (23, 3), (4, 4), (63, 6)],
        ),
        ..sample(0)
    };
    let frames = snapshot_frames(EPOCH, 3, 9, 3000, &s).unwrap();
    assert_eq!(frames[0]["rows"], 20);
    let parts: Vec<_> = frames[1..frames.len() - 1].iter().collect();
    assert_eq!(parts.len(), 3);
    for (index, part) in parts.iter().enumerate() {
        assert_eq!(part["type"], "live_rows");
        assert_eq!(part["part"], index);
        assert_eq!(part["rows"].as_array().unwrap().len(), [8, 8, 4][index]);
    }
    // The second part holds the last five items and the first three currencies.
    assert_eq!(parts[1]["rows"][4], json!([0, 13000, 13]));
    assert_eq!(parts[1]["rows"][5], json!([1, 1, 1]));
    let keys: Vec<(u64, u64)> = all_rows(&frames)
        .iter()
        .map(|row| (row[0].as_u64().unwrap(), row[1].as_u64().unwrap()))
        .collect();
    assert_eq!(keys.len(), 20);
    assert!(keys.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(frames.last().unwrap()["type"], "live_end");
}
#[test]
fn full_inventory_with_the_largest_wallet_stays_inside_every_live1_limit() {
    let s = InventorySnapshot {
        quantities: (1..=640)
            .map(|n| (i32::MAX as u32 - n, i32::MAX as u32))
            .collect(),
        wallet: wallet(
            WALLET_OWNER,
            &(1..=MAX_CURRENCIES as u32)
                .map(|n| (i32::MAX as u32 - n, i32::MAX as u32))
                .collect::<Vec<_>>(),
        ),
        ..sample(0)
    };
    let frames = snapshot_frames(EPOCH, MAX_SAFE, MAX_SAFE, MAX_SAFE, &s).unwrap();
    assert_eq!(frames[0]["rows"], MAX_ROWS);
    assert_eq!(all_rows(&frames).len(), MAX_ROWS);
    assert_eq!(frames.len(), 1 + 512 + 1);
    let mut bytes = 0;
    for frame in frames {
        let line = frame_line(frame, NONCE, MAX_SAFE).unwrap();
        assert!(line.trim_end().len() <= 512);
        bytes += line.len();
    }
    assert!(bytes < 256 * 1024, "{bytes}");
    // No wallet built by the reader can exceed the rows left next to a full inventory.
    let too_many = (1..=MAX_CURRENCIES as u32 + 1).map(|n| (n, 0)).collect();
    assert_eq!(WalletSnapshot::checked(WALLET_OWNER, too_many), None);
}
#[test]
fn failed_wallet_read_keeps_the_item_sample_its_epoch_and_never_a_live_status() {
    let now = Instant::now();
    let mut c = channel(now);
    let open = c.capture(Ok(with_wallet(0, 10214)), 0, now).remove(0);
    ready(&mut c, &open, now);
    let baseline = c.pending_frames(0, now).unwrap();
    assert_eq!(baseline[0]["mode"], "baseline");
    assert_eq!(baseline[0]["currencies"], "listed");
    assert_eq!(all_rows(&baseline).len(), 4);
    assert!(ack(&mut c, &open, 0, "stored", now));
    // The wallet read failed in this cycle: the inventory sample is what the reader returned.
    let time = now + Duration::from_secs(1);
    let frames = c.capture(Ok(sample(2)), 0, time);
    assert_eq!(frames[0]["type"], "live_begin");
    assert_eq!(frames[0]["epoch"], open["epoch"]);
    assert_eq!(frames[0]["cursor"], 1);
    assert_eq!(frames[0]["items"], "complete");
    assert_eq!(frames[0]["currencies"], "none");
    assert_eq!(frames[0]["rows"], 2);
    assert_eq!(
        all_rows(&frames),
        vec![json!([0, 12147, 2]), json!([0, 36038, 200])]
    );
    assert!(frames.iter().all(|frame| frame["type"] != "live_status"));
    assert!(ack(&mut c, &open, 1, "stored", time));
    assert_eq!(c.status, LiveStatus::Measuring);
    // The wallet comes back in the same epoch; the host owns the rebaseline of its IDs.
    let time = now + Duration::from_secs(2);
    let frames = c.capture(Ok(with_wallet(2, 10225)), 0, time);
    assert_eq!(frames[0]["epoch"], open["epoch"]);
    assert_eq!(frames[0]["cursor"], 2);
    assert_eq!(frames[0]["currencies"], "listed");
    assert_eq!(all_rows(&frames)[3], json!([1, 45, 10225]));
}
#[test]
fn wallet_of_another_owner_loses_currency_coverage_once_without_cutting_the_item_epoch() {
    let now = Instant::now();
    let mut c = channel(now);
    let open = c.capture(Ok(with_wallet(0, 10214)), 0, now).remove(0);
    ready(&mut c, &open, now);
    c.pending_frames(0, now);
    ack(&mut c, &open, 0, "stored", now);
    let other = (WALLET_OWNER.0, WALLET_OWNER.1 + 0x1000);
    let moved = |n| InventorySnapshot {
        wallet: wallet(other, &[(1, 0), (45, 99)]),
        ..sample(n)
    };
    let time = now + Duration::from_secs(1);
    let frames = c.capture(Ok(moved(2)), 0, time);
    assert_eq!(frames[0]["type"], "live_begin");
    assert_eq!(frames[0]["epoch"], open["epoch"]);
    assert_eq!(frames[0]["currencies"], "none");
    assert_eq!(
        all_rows(&frames),
        vec![json!([0, 12147, 2]), json!([0, 36038, 200])]
    );
    ack(&mut c, &open, 1, "stored", time);
    // From the uncovered sample on, the new owner's balances are listed again.
    let time = now + Duration::from_secs(2);
    let frames = c.capture(Ok(moved(2)), 0, time);
    assert_eq!(frames[0]["epoch"], open["epoch"]);
    assert_eq!(frames[0]["currencies"], "listed");
    assert_eq!(all_rows(&frames)[3], json!([1, 45, 99]));
}
#[test]
fn last_capture_diagnostics_are_current_only_while_an_epoch_is_being_sampled() {
    for (status, sampling) in [
        (LiveStatus::NotNegotiated, false),
        (LiveStatus::Waiting, true),
        (LiveStatus::Measuring, true),
        (LiveStatus::Partial, true),
        (LiveStatus::UnsupportedBuild, false),
        (LiveStatus::Unavailable, false),
        (LiveStatus::Conflict, false),
        (LiveStatus::StorageUnavailable, false),
    ] {
        assert_eq!(status.is_sampling(), sampling, "{status:?}");
    }
    let now = Instant::now();
    assert!(!Channel::new().status.is_sampling());
    let mut c = channel(now);
    c.capture(Ok(with_wallet(0, 10214)), 0, now);
    assert!(c.status.is_sampling());
    c.capture(Err(ReadError::ReadFailed), 0, now + Duration::from_secs(1));
    assert!(!c.status.is_sampling());
    let mut loading = context();
    loading.state = GameState::Loading;
    c.context_changed(&loading);
    c.gameplay_status();
    assert!(!c.status.is_sampling());
}
#[test]
fn shared_fixture_listed_sample_is_what_the_producer_emits_for_that_wallet() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/live1.json")).unwrap();
    let expected: Vec<&Value> = fixture["frames"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|frame| frame["cursor"] == 1 && frame["type"] != "live_ack")
        .collect();
    let frames = snapshot_frames(EPOCH, 1, 0, 1000, &with_wallet(2, 10225)).unwrap();
    assert_eq!(frames.len(), 3);
    assert_eq!(expected.len(), 3);
    for (frame, expected) in frames.into_iter().zip(expected) {
        let line = frame_line(frame, NONCE, expected["seq"].as_u64().unwrap()).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap(), *expected);
    }
}

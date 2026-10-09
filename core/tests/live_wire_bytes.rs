//! The exact bytes of every `live1` line the producer writes, pinned against a golden file that
//! was recorded from the producer when its frames were `serde_json::json!` trees. A frame built
//! any other way has to put the same keys in the same order with the same values, or this fails.
//!
//! The file is `fixtures/live1_wire_bytes.txt`, one line per frame. To record it again (only
//! when the wire format is meant to change) run with `WRITE_LIVE_WIRE_GOLDEN=1`.
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use tyrian_companion_nexus_core::inventory::{InventorySnapshot, ReadError};
use tyrian_companion_nexus_core::live::*;
use tyrian_companion_nexus_core::protocol::{GameContext, GameState};
use tyrian_companion_nexus_core::wallet::WalletSnapshot;

const NONCE: &str = "AQEBAQEBAQEBAQEBAQEBAQ";
const EPOCH: &str = "AgICAgICAgICAgICAgICAg";
const GOLDEN: &str = include_str!("fixtures/live1_wire_bytes.txt");

fn snapshot(items: u32, currencies: u32, unknown: u32, slots: Option<u32>) -> InventorySnapshot {
    InventorySnapshot {
        owner: (0x10a000, 0x10b000),
        quantities: (0..items).map(|n| (12000 + n * 7, 1 + n * 3)).collect(),
        unknown,
        positions: 570,
        free_slots: slots,
        wallet: (currencies > 0).then(|| {
            let balances: BTreeMap<u32, u32> = (0..currencies).map(|n| (1 + n, n * 1013)).collect();
            WalletSnapshot::checked((0x210000, 0x220000), balances).unwrap()
        }),
    }
}
fn context() -> GameContext {
    GameContext {
        state: GameState::Gameplay,
        map_id: Some(866),
        character: Some("Test Character".into()),
    }
}

/// Every line of the scenarios, with the random epoch of a channel replaced by `EPOCH`.
fn lines() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seq = 0u64;
    let push = |frames: Vec<_>, out: &mut Vec<String>, seq: &mut u64| {
        for frame in frames {
            out.push(frame_line(frame, NONCE, *seq).unwrap());
            *seq += 1;
        }
    };

    // The measured sample: 150 objects and 55 currencies, 28 frames.
    let big = snapshot(150, 55, 0, Some(37));
    push(
        snapshot_frames(EPOCH, 7, 12, 12_345, &big).unwrap(),
        &mut out,
        &mut seq,
    );
    // The same sample as the first of an epoch, partial, with no free slots read.
    let partial = snapshot(9, 0, 3, None);
    push(
        snapshot_frames(EPOCH, 0, 0, 0, &partial).unwrap(),
        &mut out,
        &mut seq,
    );
    push(
        snapshot_frames(EPOCH, 1, 4096, 1000, &snapshot(1, 1, 0, Some(0))).unwrap(),
        &mut out,
        &mut seq,
    );
    // The largest the contract lets one sample be.
    let worst = InventorySnapshot {
        quantities: (1..=640)
            .map(|n| (i32::MAX as u32 - n, i32::MAX as u32))
            .collect(),
        ..snapshot(0, 0, 0, Some(4096))
    };
    push(
        snapshot_frames(EPOCH, MAX_SAFE, MAX_SAFE, MAX_SAFE, &worst).unwrap(),
        &mut out,
        &mut seq,
    );

    // A channel through its whole life: open, samples, a failure, the statuses.
    let now = Instant::now();
    let mut c = Channel::new();
    c.accept(
        Reply::Capability {
            nonce: NONCE.into(),
        },
        NONCE,
        now,
    );
    c.context_changed(&context());
    let opened = c.capture(Ok(snapshot(3, 2, 0, Some(5))), 4, now);
    assert_eq!(opened.len(), 1);
    // The epoch is random: learn it from the line itself, before it is replaced.
    let open_line = frame_line(opened.into_iter().next().unwrap(), NONCE, seq).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&open_line).unwrap();
    let epoch = parsed["epoch"].as_str().unwrap().to_string();
    out.push(open_line.replace(&epoch, EPOCH));
    seq += 1;
    let push = |frames: Vec<_>, out: &mut Vec<String>, seq: &mut u64| {
        for frame in frames {
            let line = frame_line(frame, NONCE, *seq)
                .unwrap()
                .replace(&epoch, EPOCH);
            *seq += 1;
            out.push(line);
        }
    };
    c.accept(
        Reply::Ready {
            nonce: NONCE.into(),
            epoch: epoch.clone(),
            status: "ready".into(),
        },
        NONCE,
        now,
    );
    push(c.pending_frames(4, now).unwrap(), &mut out, &mut seq);
    let ack = |c: &mut Channel, cursor: u64, at: Instant| {
        c.accept(
            Reply::Ack {
                nonce: NONCE.into(),
                epoch: epoch.clone(),
                cursor,
                status: "stored".into(),
            },
            NONCE,
            at,
        )
    };
    ack(&mut c, 0, now);
    let later = now + Duration::from_secs(1);
    push(
        c.capture(Ok(snapshot(4, 2, 0, None)), 4, later),
        &mut out,
        &mut seq,
    );
    ack(&mut c, 1, later);
    // A sample with unknown quantities, then one that fails, twice (the reason is said once).
    let third = later + Duration::from_secs(1);
    push(
        c.capture(Ok(snapshot(4, 0, 2, Some(1))), 4, third),
        &mut out,
        &mut seq,
    );
    let fourth = third + Duration::from_secs(1);
    push(
        c.capture(Err(ReadError::ReadFailed), 4, fourth),
        &mut out,
        &mut seq,
    );
    push(
        c.capture(Err(ReadError::ReadFailed), 4, fourth),
        &mut out,
        &mut seq,
    );
    push(c.overdue(), &mut out, &mut seq);
    let mut other = Channel::new();
    other.accept(
        Reply::Capability {
            nonce: NONCE.into(),
        },
        NONCE,
        now,
    );
    other.context_changed(&context());
    push(
        other.capture(Err(ReadError::UnsupportedBuild), 0, now),
        &mut out,
        &mut seq,
    );
    let mut third_channel = Channel::new();
    third_channel.context_changed(&context());
    push(
        third_channel.capture(Err(ReadError::RootUnavailable), 0, now),
        &mut out,
        &mut seq,
    );
    let mut loading = context();
    loading.state = GameState::Loading;
    c.context_changed(&loading);
    push(c.gameplay_status(), &mut out, &mut seq);
    push(c.gameplay_status(), &mut out, &mut seq);
    out
}

#[test]
fn every_live1_line_keeps_the_bytes_it_always_had() {
    let got = lines().concat();
    if std::env::var_os("WRITE_LIVE_WIRE_GOLDEN").is_some() {
        std::fs::write(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/live1_wire_bytes.txt"
            ),
            &got,
        )
        .unwrap();
        return;
    }
    assert_eq!(got.lines().count(), GOLDEN.lines().count());
    for (n, (got, want)) in got.lines().zip(GOLDEN.lines()).enumerate() {
        assert_eq!(got, want, "line {n}");
    }
    assert_eq!(got, GOLDEN);
}

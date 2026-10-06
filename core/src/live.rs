//! Negotiated live1 producer: one bounded snapshot in flight, durable ACK before advancing.
//! This channel never computes causal loot. Baselines, context/owner changes and reconnects
//! establish fresh epochs; the host owns observations and persistence.
use crate::inventory::{InventorySnapshot, ReadError, BUILD_SHA256, PROFILE};
use crate::protocol::{is_canonical_instance, GameContext, GameState, MAX_LINE_BYTES};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

pub const MAX_SAFE: u64 = 9_007_199_254_740_991;
pub const CAPTURE_INTERVAL: Duration = Duration::from_secs(1);
pub const RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Capability {
        nonce: String,
    },
    Ready {
        nonce: String,
        epoch: String,
        status: String,
    },
    Ack {
        nonce: String,
        epoch: String,
        cursor: u64,
        status: String,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Capability {
    v: u64,
    #[serde(rename = "type")]
    kind: String,
    nonce: String,
    tag: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ready {
    v: u64,
    #[serde(rename = "type")]
    kind: String,
    nonce: String,
    tag: String,
    epoch: String,
    status: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ack {
    v: u64,
    #[serde(rename = "type")]
    kind: String,
    nonce: String,
    tag: String,
    epoch: String,
    cursor: u64,
    status: String,
}

/// Parse the original bytes too: serde struct decoding rejects duplicate keys and extras.
pub fn parse_reply(kind: &str, line: &str) -> Option<Reply> {
    let line = line.strip_suffix('\r').unwrap_or(line);
    if line.len() > MAX_LINE_BYTES {
        return None;
    }
    match kind {
        "live_cap" => {
            let r: Capability = serde_json::from_str(line).ok()?;
            (r.v == 3 && r.kind == kind && r.tag == "live1" && is_canonical_instance(&r.nonce))
                .then_some(Reply::Capability { nonce: r.nonce })
        }
        "live_ready" => {
            let r: Ready = serde_json::from_str(line).ok()?;
            (r.v == 3
                && r.kind == kind
                && r.tag == "live1"
                && is_canonical_instance(&r.nonce)
                && is_canonical_instance(&r.epoch)
                && matches!(
                    r.status.as_str(),
                    "ready" | "source_conflict" | "unsupported_build" | "not_gameplay"
                ))
            .then_some(Reply::Ready {
                nonce: r.nonce,
                epoch: r.epoch,
                status: r.status,
            })
        }
        "live_ack" => {
            let r: Ack = serde_json::from_str(line).ok()?;
            (r.v == 3
                && r.kind == kind
                && r.tag == "live1"
                && is_canonical_instance(&r.nonce)
                && is_canonical_instance(&r.epoch)
                && r.cursor <= MAX_SAFE
                && matches!(
                    r.status.as_str(),
                    "stored" | "storage_unavailable" | "not_owner"
                ))
            .then_some(Reply::Ack {
                nonce: r.nonce,
                epoch: r.epoch,
                cursor: r.cursor,
                status: r.status,
            })
        }
        _ => None,
    }
}

/// Shared UI diagnostics are deliberately independent of TCP presence and farm1 state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LiveStatus {
    #[default]
    NotNegotiated,
    Waiting,
    Measuring,
    Partial,
    UnsupportedBuild,
    Unavailable,
    Conflict,
    StorageUnavailable,
}

#[derive(Debug)]
struct Epoch {
    id: String,
    owner: (u64, u64),
    started: Instant,
    cursor: u64,
    ready: bool,
    pending: Option<InventorySnapshot>,
    waiting_ack: bool,
    sent_at: Instant,
    partial: bool,
}
#[derive(Debug, Default)]
pub struct Channel {
    enabled: bool,
    context: Option<GameContext>,
    epoch: Option<Epoch>,
    // Diagnostics identify the last declared epoch even after continuity is invalidated.
    last_declared_epoch: Option<String>,
    next_capture: Option<Instant>,
    reason: Option<&'static str>,
    blocked: bool,
    pub status: LiveStatus,
}
impl Channel {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    /// Returns false only when a negotiated live error requires closing/reconnecting TCP.
    pub fn accept(&mut self, reply: Reply, nonce: &str, now: Instant) -> bool {
        match reply {
            Reply::Capability { nonce: n } if n == nonce => {
                self.enabled = true;
                if self.status == LiveStatus::NotNegotiated {
                    self.status = LiveStatus::Waiting;
                }
            }
            Reply::Ready {
                nonce: n,
                epoch,
                status,
            } if n == nonce => {
                if let Some(e) = self.epoch.as_mut().filter(|e| e.id == epoch && !e.ready) {
                    if status == "ready" {
                        e.ready = true;
                        e.sent_at = now;
                    } else {
                        self.blocked = true;
                        self.epoch = None;
                        self.status = if status == "source_conflict" {
                            LiveStatus::Conflict
                        } else if status == "unsupported_build" {
                            LiveStatus::UnsupportedBuild
                        } else {
                            LiveStatus::Unavailable
                        };
                    }
                }
            }
            Reply::Ack {
                nonce: n,
                epoch,
                cursor,
                status,
            } if n == nonce => {
                if let Some(e) = self
                    .epoch
                    .as_mut()
                    .filter(|e| e.id == epoch && e.cursor == cursor && e.waiting_ack)
                {
                    if status != "stored" {
                        self.status = if status == "storage_unavailable" {
                            LiveStatus::StorageUnavailable
                        } else {
                            LiveStatus::Conflict
                        };
                        return false;
                    }
                    e.waiting_ack = false;
                    self.status = if e.partial {
                        LiveStatus::Partial
                    } else {
                        LiveStatus::Measuring
                    };
                    if e.partial {
                        self.epoch = None;
                    } else if let Some(next) =
                        e.cursor.checked_add(1).filter(|next| *next <= MAX_SAFE)
                    {
                        e.cursor = next;
                    } else {
                        self.epoch = None;
                    }
                }
            }
            _ => {}
        }
        true
    }
    /// Context equality is semantic: an identical periodic context does not cut continuity.
    pub fn context_changed(&mut self, context: &GameContext) {
        if self.context.as_ref() != Some(context) {
            self.context = Some(context.clone());
            self.epoch = None;
            self.next_capture = None;
            self.reason = None;
            self.blocked = false;
        }
    }
    pub fn timed_out(&self, now: Instant) -> bool {
        self.epoch.as_ref().is_some_and(|e| {
            (!e.ready || e.waiting_ack)
                && now.saturating_duration_since(e.sent_at) >= RESPONSE_TIMEOUT
        })
    }
    /// True only when taking a sample would be useful. No polling the game for old servers,
    /// non-gameplay, an unready epoch, an unacknowledged sample, or a conflicting producer.
    pub fn wants_sample(&self, now: Instant) -> bool {
        self.enabled
            && !self.blocked
            && self
                .context
                .as_ref()
                .is_some_and(|c| c.state == GameState::Gameplay)
            && self.next_capture.is_none_or(|due| now >= due)
            && self
                .epoch
                .as_ref()
                .is_none_or(|e| e.ready && !e.waiting_ack && e.pending.is_none())
    }
    /// Consume the last capture only after ready, without asking for a second baseline.
    pub fn pending_frames(&mut self, ctx: u64, now: Instant) -> Option<Vec<Value>> {
        let e = self.epoch.as_mut()?;
        if !e.ready || e.waiting_ack {
            return None;
        }
        let snapshot = e.pending.take()?;
        let frames = snapshot_frames(&e.id, e.cursor, ctx, 0, &snapshot)?;
        e.waiting_ack = true;
        e.sent_at = now;
        e.partial = snapshot.unknown != 0;
        Some(frames)
    }
    /// Capture errors invalidate the whole epoch. Unknown quantities preserve known IDs in a
    /// partial sample, and its ACK must be followed by a new epoch before further comparison.
    pub fn capture(
        &mut self,
        result: Result<InventorySnapshot, ReadError>,
        ctx: u64,
        now: Instant,
    ) -> Vec<Value> {
        self.next_capture = Some(now + CAPTURE_INTERVAL);
        let snapshot = match result {
            Ok(snapshot) => snapshot,
            Err(error) => {
                let reason = match error {
                    ReadError::UnsupportedBuild => "unsupported_build",
                    ReadError::RootUnavailable => "root_unavailable",
                    _ => "read_failed",
                };
                self.epoch = None;
                let epoch = self.last_declared_epoch.clone();
                self.status = if error == ReadError::UnsupportedBuild {
                    self.blocked = true;
                    LiveStatus::UnsupportedBuild
                } else {
                    LiveStatus::Unavailable
                };
                if self.reason == Some(reason) {
                    return vec![];
                }
                self.reason = Some(reason);
                return vec![
                    json!({"type":"live_status","epoch":epoch,"status":"unavailable","reason":reason}),
                ];
            }
        };
        self.reason = None;
        if self
            .epoch
            .as_ref()
            .is_some_and(|e| e.owner != snapshot.owner)
        {
            self.epoch = None;
        }
        if self.epoch.is_none() {
            let id = crate::instance::new_instance_id();
            self.last_declared_epoch = Some(id.clone());
            let frame =
                json!({"type":"live_open","epoch":id,"build":BUILD_SHA256,"profile":PROFILE});
            self.epoch = Some(Epoch {
                id,
                owner: snapshot.owner,
                started: now,
                cursor: 0,
                ready: false,
                pending: Some(snapshot),
                waiting_ack: false,
                sent_at: now,
                partial: false,
            });
            self.status = LiveStatus::Waiting;
            return vec![frame];
        }
        let e = self.epoch.as_mut().unwrap();
        let ms = now.saturating_duration_since(e.started).as_millis();
        if ms == 0 || ms > MAX_SAFE as u128 {
            self.epoch = None;
            return vec![];
        }
        let Some(frames) = snapshot_frames(&e.id, e.cursor, ctx, ms as u64, &snapshot) else {
            self.epoch = None;
            return vec![];
        };
        e.waiting_ack = true;
        e.sent_at = now;
        e.partial = snapshot.unknown != 0;
        frames
    }
    /// Non-gameplay status is emitted once per discontinuity, never every poll.
    pub fn gameplay_status(&mut self) -> Vec<Value> {
        if !self.enabled
            || self
                .context
                .as_ref()
                .is_some_and(|c| c.state == GameState::Gameplay)
            || self.reason == Some("not_gameplay")
        {
            return vec![];
        }
        self.reason = Some("not_gameplay");
        self.status = LiveStatus::Unavailable;
        vec![
            json!({"type":"live_status","epoch":self.last_declared_epoch,"status":"unavailable","reason":"not_gameplay"}),
        ]
    }
}

/// Frames before transport numbering; all row totals are sorted, never raw pointers/slots.
pub fn snapshot_frames(
    epoch: &str,
    cursor: u64,
    ctx: u64,
    ms: u64,
    s: &InventorySnapshot,
) -> Option<Vec<Value>> {
    if !is_canonical_instance(epoch)
        || cursor > MAX_SAFE
        || ctx > MAX_SAFE
        || ms > MAX_SAFE
        || s.unknown > 640
        || s.quantities.len() > 640
        || (cursor == 0 && ms != 0)
        || (cursor > 0 && ms == 0)
        || s.free_slots.is_some_and(|slots| slots > 4096)
        || s.quantities
            .iter()
            .any(|(id, q)| *id == 0 || *id > i32::MAX as u32 || *q > i32::MAX as u32)
    {
        return None;
    }
    let mut frames = vec![
        json!({"type":"live_begin","epoch":epoch,"cursor":cursor,"ctx":ctx,"ms":ms,
        "mode":if cursor==0 {"baseline"}else{"sample"},"items":if s.unknown==0 {"complete"}else{"partial"},
        "currencies":"none","unknown":s.unknown,"slots":s.free_slots,"rows":s.quantities.len()}),
    ];
    let rows: Vec<_> = s.quantities.iter().map(|(id, n)| [0, *id, *n]).collect();
    for (part, rows) in rows.chunks(8).enumerate() {
        frames.push(
            json!({"type":"live_rows","epoch":epoch,"cursor":cursor,"part":part,"rows":rows}),
        );
    }
    frames.push(json!({"type":"live_end","epoch":epoch,"cursor":cursor}));
    Some(frames)
}

/// Check the final serialization, including the shared sequence number, before any socket write.
pub fn frame_line(mut frame: Value, nonce: &str, seq: u64) -> Option<String> {
    if !is_canonical_instance(nonce) || seq > MAX_SAFE {
        return None;
    }
    let object = frame.as_object_mut()?;
    object.insert("v".into(), json!(3));
    object.insert("tag".into(), json!("live1"));
    object.insert("nonce".into(), json!(nonce));
    object.insert("seq".into(), json!(seq));
    let mut line = serde_json::to_string(&frame).ok()?;
    if line.len() > MAX_LINE_BYTES {
        return None;
    }
    line.push('\n');
    Some(line)
}

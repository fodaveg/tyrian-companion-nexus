//! The optional `farm1` feed. It only carries bounded aggregate readings from the plugin:
//! no account identity, API key, inventory contents, or game input belongs to this feed.

use std::time::{Duration, Instant};

use serde::Deserialize;

/// A transport snapshot expires independently of the source observation's own age.
pub const FARMING_TTL: Duration = Duration::from_secs(15);

/// A reading this many seconds old (or older) is worth telling the player about.
pub const READING_AGE_NOTICE: u64 = 15;

/// The panel footer "Inventory: observations stored" says nothing while measuring normally;
/// every other status is shown. The Options window keeps all of them as diagnostics.
pub fn show_inventory_status(status: crate::live::LiveStatus) -> bool {
    status != crate::live::LiveStatus::Measuring
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase { Idle, Starting, Active, Stopping, Provisional, Complete, Error, Abandoned }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FarmingError { Start, Observe, Stop, Save, Other }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotSource { Ingame, Recent, Unknown }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Goal { None, Bags, Duration }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MagicFindKind { Partial, Unknown }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preparation { Partial, Attention, Unknown }

/// One whole, flat snapshot. Optional numbers mean unknown, never zero. `observed` counts
/// positive API increments; signed `net` is a separate reconciliation at close. Rates and
/// estimates come from the host; the addon never extrapolates them or ticks `elapsed` locally.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FarmingState {
    pub v: u64,
    #[serde(rename = "type")]
    pub kind: String,
    pub tag: String,
    pub nonce: String,
    pub seq: i32,
    pub ttl: u64,
    pub phase: Phase,
    pub err: Option<FarmingError>,
    pub elapsed: Option<i32>,
    pub observed: Option<i32>,
    pub net: Option<i32>,
    pub lo: Option<i32>,
    pub hi: Option<i32>,
    /// Seconds since the source observation, independent of transport refreshes.
    pub age: Option<i32>,
    pub slots: Option<i32>,
    #[serde(rename = "slotSrc")]
    pub slot_source: SlotSource,
    #[serde(rename = "slotAge")]
    pub slot_age: Option<i32>,
    pub goal: Goal,
    pub target: Option<i32>,
    pub progress: Option<i32>,
    pub eta: Option<i32>,
    pub mf: Option<i32>,
    #[serde(rename = "mfKind")]
    pub mf_kind: MagicFindKind,
    pub prep: Preparation,
}

impl FarmingState {
    /// Checks numeric semantics after serde has rejected wrong types, overflow and enums.
    pub(crate) fn valid(&self) -> bool {
        self.v == 3 && self.kind == "farming_state" && self.tag == "farm1"
            && self.ttl == 15 && self.seq > 0
            && [self.elapsed, self.observed, self.lo, self.hi, self.age, self.slots,
                self.slot_age, self.target, self.progress, self.eta, self.mf]
                .iter().all(|number| number.is_none_or(|number| number >= 0))
    }
}

/// A reading ready for rendering, with monotonic transport freshness kept apart from source ages.
#[derive(Debug, Clone)]
pub struct FarmingView {
    pub capable: bool,
    pub reading: Option<FarmingState>,
    pub fresh: bool,
    pub age: Option<u64>,
    pub slot_age: Option<u64>,
}

impl FarmingView {
    /// Source age is independent of transport refreshes. Exact age 5 is already stale.
    pub fn source_fresh(&self) -> bool {
        self.fresh && self.age.is_some_and(|age| age < 5)
    }

    /// Whether "last reading ago Xs" (and the slot reading's age) is worth painting. In normal
    /// measurement it is noise, so it shows only when it reports a problem: the transport is
    /// old, `err` is set, there is no reading, or the reading is 15 s old or more. Only while a
    /// session is starting, active, stopping, provisional or in error. Not the 5 s freshness
    /// threshold: that one would make the line blink.
    pub fn show_reading_age(&self) -> bool {
        let Some(reading) = self.reading.as_ref() else { return false };
        matches!(reading.phase, Phase::Starting | Phase::Active | Phase::Stopping | Phase::Provisional | Phase::Error)
            && (!self.fresh || reading.err.is_some() || self.age.is_none_or(|age| age >= READING_AGE_NOTICE))
    }

    /// Bags depend on a fresh observation; duration is the host's declared countdown and
    /// remains available through an observation error. Transport and phase still apply.
    pub fn eta(&self) -> Option<i32> {
        let reading = self.reading.as_ref()?;
        if !self.fresh || reading.phase != Phase::Active { return None; }
        let allowed = match reading.goal {
            Goal::Duration => matches!(reading.err, None | Some(FarmingError::Observe)),
            Goal::Bags => reading.err.is_none() && self.source_fresh(),
            Goal::None => false,
        };
        if allowed { reading.eta } else { None }
    }
}

/// Per-connection capability and sequence, plus the last reading retained across disconnects.
#[derive(Debug, Default)]
pub(crate) struct FarmingFeed {
    nonce: Option<String>,
    capable: bool,
    last_seq: i32,
    reading: Option<(FarmingState, Instant)>,
}

impl FarmingFeed {
    pub fn begin(&mut self, nonce: &str) {
        self.nonce = Some(nonce.to_string());
        self.capable = false;
        self.last_seq = 0;
    }

    pub fn disconnect(&mut self) {
        self.nonce = None;
        self.capable = false;
    }

    /// A capability is accepted once, only after `welcome` and for its active nonce.
    pub fn enable(&mut self, nonce: &str) -> bool {
        if self.nonce.as_deref() != Some(nonce) || self.capable { return false; }
        self.capable = true;
        true
    }

    pub fn accept(&mut self, reading: FarmingState, now: Instant) -> bool {
        if !self.capable || self.nonce.as_deref() != Some(&reading.nonce)
            || reading.seq <= self.last_seq { return false; }
        self.last_seq = reading.seq;
        self.reading = Some((reading, now));
        true
    }

    pub fn view(&self, now: Instant) -> FarmingView {
        let fresh = self.reading.as_ref().is_some_and(|(reading, received)| {
            self.capable && self.last_seq > 0 && self.nonce.as_deref() == Some(&reading.nonce)
                && now.saturating_duration_since(*received) < FARMING_TTL
        });
        let age = |value: Option<i32>, received: Instant| {
            value.map(|value| value as u64 + now.saturating_duration_since(received).as_secs())
        };
        FarmingView {
            capable: self.capable,
            reading: self.reading.as_ref().map(|(reading, _)| reading.clone()),
            fresh,
            age: self.reading.as_ref().and_then(|(reading, received)| age(reading.age, *received)),
            slot_age: self.reading.as_ref().and_then(|(reading, received)| age(reading.slot_age, *received)),
        }
    }
}

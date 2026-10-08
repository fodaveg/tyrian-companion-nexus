//! The optional `price2` feed: the public trading-post price of one bag (item 36038), gross, as
//! the trading post shows it, with no fee taken off. No account identity, API key, item id or
//! game input belongs to this feed. Same shape as `farming`: a per-connection capability, an
//! increasing sequence, 15 s of monotonic transport freshness, and an `age` that keeps growing
//! locally from the moment a frame is received.
//!
//! `price2` replaces `price1`, whose frames had the same shape and carried the figures net of
//! fees. The tag is the only thing telling them apart, so a `price1` capability or state is
//! discarded: this addon never paints a net figure under a gross label, nor the reverse.

use std::time::{Duration, Instant};

use serde::Deserialize;

/// The only tag this addon subscribes to and reads.
pub const PRICE_TAG: &str = "price2";

/// A transport snapshot expires after this, whatever the quote's own age.
pub const PRICE_TTL: Duration = Duration::from_secs(15);

/// What the host says about the quote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriceStatus {
    Ok,
    Idle,
    Pending,
    Stale,
}

/// One whole, flat snapshot. Amounts are copper, `None` means unknown, never zero.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriceState {
    pub v: u64,
    #[serde(rename = "type")]
    pub kind: String,
    pub tag: String,
    pub nonce: String,
    pub seq: i32,
    pub ttl: u64,
    pub st: PriceStatus,
    /// Unit price of the highest buy order: what selling one bag at once is offered.
    pub sell: Option<i32>,
    #[serde(rename = "sellStack")]
    pub sell_stack: Option<i32>,
    /// Unit price of the lowest sell offer.
    pub list: Option<i32>,
    #[serde(rename = "listStack")]
    pub list_stack: Option<i32>,
    /// Seconds since the quote at emission time.
    pub age: Option<i32>,
}

impl PriceState {
    /// Numeric semantics after serde has rejected wrong types, overflow and enums.
    pub(crate) fn valid(&self) -> bool {
        let amounts = [self.sell, self.sell_stack, self.list, self.list_stack];
        self.v == 3
            && self.kind == "price_state"
            && self.tag == PRICE_TAG
            && self.ttl == 15
            && self.seq > 0
            && amounts
                .iter()
                .chain([&self.age])
                .all(|number| number.is_none_or(|number| number >= 0))
            && (self.st == PriceStatus::Ok || amounts.iter().all(Option::is_none))
    }
}

/// A reading ready for rendering.
#[derive(Debug, Clone)]
pub struct PriceView {
    pub capable: bool,
    pub reading: Option<PriceState>,
    pub fresh: bool,
    /// Quote age: the frame's `age` plus the local time since it was received.
    pub age: Option<u64>,
}

/// Per-connection capability and sequence, plus the last reading retained across disconnects.
#[derive(Debug, Default)]
pub(crate) struct PriceFeed {
    nonce: Option<String>,
    capable: bool,
    last_seq: i32,
    reading: Option<(PriceState, Instant)>,
}

impl PriceFeed {
    pub fn begin(&mut self, nonce: &str) {
        self.nonce = Some(nonce.to_string());
        self.capable = false;
        self.last_seq = 0;
    }

    /// Capability and figures are gone at once; nothing of an old reading is painted.
    pub fn disconnect(&mut self) {
        self.nonce = None;
        self.capable = false;
        self.reading = None;
    }

    /// A capability is accepted once, only after `welcome` and for its active nonce.
    pub fn enable(&mut self, nonce: &str) -> bool {
        if self.nonce.as_deref() != Some(nonce) || self.capable {
            return false;
        }
        self.capable = true;
        true
    }

    pub fn accept(&mut self, reading: PriceState, now: Instant) -> bool {
        if !self.capable
            || self.nonce.as_deref() != Some(&reading.nonce)
            || reading.seq <= self.last_seq
        {
            return false;
        }
        self.last_seq = reading.seq;
        self.reading = Some((reading, now));
        true
    }

    pub fn view(&self, now: Instant) -> PriceView {
        let current = self.reading.as_ref().filter(|(reading, _)| {
            self.capable && self.last_seq > 0 && self.nonce.as_deref() == Some(&reading.nonce)
        });
        PriceView {
            capable: self.capable,
            reading: current.map(|(reading, _)| reading.clone()),
            fresh: current
                .is_some_and(|(_, received)| now.saturating_duration_since(*received) < PRICE_TTL),
            age: current.and_then(|(reading, received)| {
                reading
                    .age
                    .map(|age| age as u64 + now.saturating_duration_since(*received).as_secs())
            }),
        }
    }
}

/// `45c`, `2s 93c`, `7g 33s 12c`, `1g 0s 0c`: no leading units at zero.
pub fn format_coins(copper: i32) -> String {
    let total = i64::from(copper).unsigned_abs();
    let (gold, silver, rest) = (total / 10_000, total / 100 % 100, total % 100);
    let sign = if copper < 0 { "-" } else { "" };
    if gold > 0 {
        format!("{sign}{gold}g {silver}s {rest}c")
    } else if silver > 0 {
        format!("{sign}{silver}s {rest}c")
    } else {
        format!("{sign}{rest}c")
    }
}

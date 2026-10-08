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

/// One painted line of the price block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriceLine {
    pub text: String,
    /// Painted in the warning colour.
    pub warning: bool,
}

fn side(label: &str, unit: Option<i32>, stack: Option<i32>) -> String {
    match (unit, stack) {
        (None, _) => format!("{label} —"),
        (Some(unit), None) => format!("{label} {} · ×250 —", format_coins(unit)),
        (Some(unit), Some(stack)) => format!(
            "{label} {} · ×250 {}",
            format_coins(unit),
            format_coins(stack)
        ),
    }
}

/// How many lines the price block takes whenever it is painted: a header and the two sides.
pub const PANEL_LINES: usize = 3;

/// The price as the three text lines of the `price2` contract (header, buy order, sell offer).
/// The Labyrinth panel painted these until 0.7.2; since 0.8.0 it paints the two stack prices
/// of [`crate::panel`] instead, and nothing in the addon calls this.
///
/// The lines of the price block: none without a capability (which includes disconnected) or
/// on `idle`, and [`PANEL_LINES`] in every other state, so the rest of the panel never moves
/// when the price changes state. The header carries the state and the two sides stay in place
/// with `—` when there is no figure to show: no frame yet, `pending`, `stale`, no quote, or a
/// transport of 15 s or more, whose old figures are not painted. The quote's age is shown only
/// on `stale`, in the warning colour.
pub fn panel_lines(view: &PriceView, english: bool) -> Vec<PriceLine> {
    let tr = |es: &str, en: &str| {
        if english {
            en.to_string()
        } else {
            es.to_string()
        }
    };
    let line = |text: String, warning: bool| PriceLine { text, warning };
    // `idle` hides the block even after its transport expires: it is not a state the price
    // leaves on its own, and a block that came back 15 s later would be a jump.
    let idle = view.reading.as_ref().is_some_and(|reading| reading.st == PriceStatus::Idle);
    if !view.capable || idle {
        return Vec::new();
    }
    // Only a reading whose transport is still fresh says anything about the price now.
    let reading = view.reading.as_ref().filter(|_| view.fresh);
    let figures = reading.filter(|reading| reading.st == PriceStatus::Ok);
    let header = match reading.map(|reading| reading.st) {
        None | Some(PriceStatus::Pending | PriceStatus::Idle) => line(
            tr("Saco · precio aún sin leer", "Bag · price not read yet"),
            false,
        ),
        Some(PriceStatus::Stale) => {
            let text = match view.age.or(reading.and_then(|reading| reading.age).map(|age| age as u64)) {
                Some(age) => {
                    let minutes = age / 60;
                    if english {
                        format!("Bag · price expired ({minutes} min ago)")
                    } else {
                        format!("Saco · precio caducado (hace {minutes} min)")
                    }
                }
                None => tr("Saco · precio caducado", "Bag · price expired"),
            };
            line(text, true)
        }
        Some(PriceStatus::Ok) if figures.is_some_and(|reading| reading.sell.is_none() && reading.list.is_none()) => {
            line(tr("Saco · sin cotización", "Bag · no quote"), false)
        }
        Some(PriceStatus::Ok) => line(tr("Saco · precio del bazar", "Bag · trading post price"), false),
    };
    // The contract's full labels are "Pedido más alto" / "Oferta más baja" ("Highest buy
    // order" / "Lowest sell offer"). With the amounts after them they run to 40 characters,
    // past what the panel holds on one line, so its own short forms are used.
    vec![
        header,
        line(
            side(
                &tr("Pedido", "Buy order"),
                figures.and_then(|reading| reading.sell),
                figures.and_then(|reading| reading.sell_stack),
            ),
            false,
        ),
        line(
            side(
                &tr("Oferta", "Sell offer"),
                figures.and_then(|reading| reading.list),
                figures.and_then(|reading| reading.list_stack),
            ),
            false,
        ),
    ]
}

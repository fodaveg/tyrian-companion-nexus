//! The wire contract fixed by `docs/SPEC-puente-ingame.md` in `tyrian-companion`.
//!
//! Plugin -> addon: one JSON line per alert, UTF-8, 512 bytes max, shaped like
//! `AlertIngamePayload` in `src/alerts/alert-ingame.ts`. Addon -> plugin: exactly one line,
//! at connect time, 128 bytes max, and never again. This module only builds and reads those
//! two shapes; it does not touch a socket (`crate::client` does).

use serde::{Deserialize, Serialize};

/// Default port the plugin listens on. Matches the spec and the plugin's own default.
pub const DEFAULT_PORT: u16 = 47823;

/// Wire cap on an incoming alert line (spec: "512 bytes como máximo").
pub const MAX_ALERT_LINE_BYTES: usize = 512;

/// The only protocol version this addon speaks.
pub const SUPPORTED_VERSION: u32 = 1;

/// Message shown once when the plugin sends a `v` this addon does not understand.
pub const UPDATE_ADDON_MESSAGE: &str = "Tyrian Companion: update the Nexus addon to see new alerts";

/// The client's own identifier in the `hello` line.
const CLIENT_NAME: &str = "nexus";

/// The `hello` line this addon sends once per connection, and never anything after it.
///
/// `docs/SPEC-puente-ingame.md` fixes the shape and the 128-byte cap; the server closes the
/// connection on any byte received after this line, so getting the shape exactly right (and
/// under the cap) matters more here than almost anywhere else in this addon.
#[derive(Serialize)]
struct HelloLine<'a> {
    v: u32,
    client: &'a str,
    #[serde(rename = "clientVersion")]
    client_version: &'a str,
}

/// Cap on the `hello` line the spec fixes separately from the alert line cap.
pub const MAX_HELLO_LINE_BYTES: usize = 128;

/// Builds the `hello` line, `\n` included, ready to write to the socket.
///
/// Returns `None` if the composed line (with its own crate version baked in) would exceed the
/// wire cap, so a caller cannot accidentally send a line the plugin's framer would refuse.
pub fn build_hello_line(client_version: &str) -> Option<String> {
    let hello = HelloLine { v: SUPPORTED_VERSION, client: CLIENT_NAME, client_version };
    let mut line = serde_json::to_string(&hello).ok()?;
    line.push('\n');
    if line.len() > MAX_HELLO_LINE_BYTES {
        return None;
    }
    Some(line)
}

/// The four kinds `AlertV1` declares. Only ever chooses a title/color, per the spec: the
/// rendered text always comes from `content`, never from this field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertKind {
    ValuableLoot,
    AlwaysAlert,
    SellSignal,
    HoldSignal,
    /// A `kind` string this addon does not recognize. Still shown: only the title/color
    /// picked from `kind` degrades, the alert itself does not get dropped for this reason.
    Unknown,
}

impl AlertKind {
    fn from_wire(kind: &str) -> Self {
        match kind {
            "valuable_loot" => Self::ValuableLoot,
            "always_alert" => Self::AlwaysAlert,
            "sell_signal" => Self::SellSignal,
            "hold_signal" => Self::HoldSignal,
            _ => Self::Unknown,
        }
    }

    /// Short label used only by the optional recent-alerts panel, never by the native alert
    /// (which paints `content` verbatim and nothing else).
    pub fn label(self) -> &'static str {
        match self {
            Self::ValuableLoot => "Valuable loot",
            Self::AlwaysAlert => "Alert",
            Self::SellSignal => "Sell",
            Self::HoldSignal => "Hold",
            Self::Unknown => "Alert",
        }
    }

    /// RGBA used only by the optional recent-alerts panel.
    pub fn color(self) -> [f32; 4] {
        match self {
            Self::ValuableLoot => [0.95, 0.78, 0.20, 1.0], // gold
            Self::AlwaysAlert => [0.95, 0.35, 0.35, 1.0],  // red
            Self::SellSignal => [0.45, 0.85, 0.45, 1.0],   // green
            Self::HoldSignal => [0.45, 0.70, 0.95, 1.0],   // blue
            Self::Unknown => [0.85, 0.85, 0.85, 1.0],      // grey
        }
    }
}

/// An alert this addon can act on: `content` to paint verbatim, `kind` to color/title an
/// optional extra panel, and `seq` to deduplicate across reconnects.
#[derive(Debug, Clone, PartialEq)]
pub struct Alert {
    pub seq: Option<u64>,
    pub kind: AlertKind,
    pub name: String,
    pub quantity: i64,
    pub total_copper: Option<i64>,
    pub content: String,
}

/// What reading one line from the plugin resolves to.
#[derive(Debug, Clone, PartialEq)]
pub enum LineOutcome {
    /// A well-formed alert, ready to paint.
    Alert(Alert),
    /// `v` is greater than `SUPPORTED_VERSION`: the caller should show
    /// [`UPDATE_ADDON_MESSAGE`] once and not try to interpret the rest of this line.
    UnsupportedVersion,
    /// Oversized, not JSON, not an object, or missing a field this addon needs. The spec is
    /// explicit that this closes nothing: the caller reads the next line as if this one had
    /// never arrived.
    Discard,
}

/// Every field this addon requires to act on a line, deserialized permissively: an `Option`
/// field that is simply absent from the JSON is `None` rather than a parse error (serde's
/// default behaviour), and a key this addon does not know about is ignored rather than
/// rejected (also serde's default behaviour, i.e. no `deny_unknown_fields`). Only a value of
/// the *wrong type* for a field present in the JSON turns into a hard parse error, which
/// `parse_alert_line` treats the same as broken JSON: discard the line.
#[derive(Debug, Deserialize)]
struct RawAlertLine {
    v: Option<u32>,
    seq: Option<u64>,
    kind: Option<String>,
    name: Option<String>,
    quantity: Option<i64>,
    #[serde(rename = "totalCopper")]
    total_copper: Option<i64>,
    content: Option<String>,
}

/// Parses one line from the plugin (without its trailing `\n`, which the framer strips).
///
/// Every failure mode the spec calls out — a version this addon does not speak, broken JSON,
/// an oversized line, a line missing a field this addon needs — resolves to
/// [`LineOutcome::Discard`] or [`LineOutcome::UnsupportedVersion`], never to an error the
/// caller has to propagate: this function cannot fail the connection.
pub fn parse_alert_line(line: &str) -> LineOutcome {
    if line.len() > MAX_ALERT_LINE_BYTES {
        return LineOutcome::Discard;
    }
    let Ok(raw) = serde_json::from_str::<RawAlertLine>(line) else {
        return LineOutcome::Discard;
    };
    let Some(v) = raw.v else {
        return LineOutcome::Discard;
    };
    if v > SUPPORTED_VERSION {
        return LineOutcome::UnsupportedVersion;
    }
    if v < SUPPORTED_VERSION {
        // Not a version this addon has ever spoken either; nothing to interpret.
        return LineOutcome::Discard;
    }
    let (Some(kind), Some(name), Some(content), Some(quantity)) =
        (raw.kind, raw.name, raw.content, raw.quantity)
    else {
        return LineOutcome::Discard;
    };
    LineOutcome::Alert(Alert {
        seq: raw.seq,
        kind: AlertKind::from_wire(&kind),
        name,
        quantity,
        total_copper: raw.total_copper,
        content,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_line_matches_the_spec_shape() {
        let line = build_hello_line("0.1.0").expect("hello line within cap");
        assert_eq!(line, "{\"v\":1,\"client\":\"nexus\",\"clientVersion\":\"0.1.0\"}\n");
        assert!(line.len() <= MAX_HELLO_LINE_BYTES);
    }

    #[test]
    fn hello_line_refuses_to_exceed_its_cap() {
        let absurd_version = "x".repeat(200);
        assert_eq!(build_hello_line(&absurd_version), None);
    }

    #[test]
    fn parses_a_well_formed_alert() {
        let line = r#"{"v":1,"seq":17,"kind":"valuable_loot","name":"Mystic Coin","quantity":3,"totalCopper":123456,"content":"Mystic Coin ×3 · 123456 copper"}"#;
        let outcome = parse_alert_line(line);
        let LineOutcome::Alert(alert) = outcome else { panic!("expected Alert, got {outcome:?}") };
        assert_eq!(alert.seq, Some(17));
        assert_eq!(alert.kind, AlertKind::ValuableLoot);
        assert_eq!(alert.name, "Mystic Coin");
        assert_eq!(alert.quantity, 3);
        assert_eq!(alert.total_copper, Some(123456));
        assert_eq!(alert.content, "Mystic Coin \u{00d7}3 \u{00b7} 123456 copper");
    }

    #[test]
    fn parses_an_alert_without_a_quote() {
        let line = r#"{"v":1,"kind":"always_alert","name":"Rare Skin","quantity":1,"totalCopper":null,"content":"no quoted value"}"#;
        let LineOutcome::Alert(alert) = parse_alert_line(line) else { panic!("expected Alert") };
        assert_eq!(alert.total_copper, None);
        assert_eq!(alert.seq, None);
        assert_eq!(alert.kind, AlertKind::AlwaysAlert);
    }

    #[test]
    fn an_unknown_kind_still_shows_the_alert() {
        let line = r#"{"v":1,"kind":"something_new","name":"X","quantity":1,"totalCopper":null,"content":"X"}"#;
        let LineOutcome::Alert(alert) = parse_alert_line(line) else { panic!("expected Alert") };
        assert_eq!(alert.kind, AlertKind::Unknown);
        assert_eq!(alert.content, "X");
    }

    #[test]
    fn extra_keys_are_tolerated() {
        let line = r#"{"v":1,"kind":"sell_signal","name":"X","quantity":1,"totalCopper":1,"content":"X","accountRef":"secret","reason":"skin_not_unlocked"}"#;
        let LineOutcome::Alert(alert) = parse_alert_line(line) else { panic!("expected Alert") };
        assert_eq!(alert.kind, AlertKind::SellSignal);
    }

    #[test]
    fn a_higher_version_asks_for_an_update_and_nothing_else() {
        let line = r#"{"v":2,"kind":"valuable_loot","name":"X","quantity":1,"totalCopper":1,"content":"X"}"#;
        assert_eq!(parse_alert_line(line), LineOutcome::UnsupportedVersion);
    }

    #[test]
    fn a_lower_version_is_discarded_not_interpreted() {
        let line = r#"{"v":0,"kind":"valuable_loot","name":"X","quantity":1,"totalCopper":1,"content":"X"}"#;
        assert_eq!(parse_alert_line(line), LineOutcome::Discard);
    }

    #[test]
    fn missing_v_is_discarded() {
        let line = r#"{"kind":"valuable_loot","name":"X","quantity":1,"totalCopper":1,"content":"X"}"#;
        assert_eq!(parse_alert_line(line), LineOutcome::Discard);
    }

    #[test]
    fn missing_a_required_field_is_discarded() {
        let line = r#"{"v":1,"kind":"valuable_loot","quantity":1,"totalCopper":1,"content":"X"}"#; // no name
        assert_eq!(parse_alert_line(line), LineOutcome::Discard);
    }

    #[test]
    fn a_wrong_type_for_a_present_field_is_discarded() {
        let line = r#"{"v":1,"kind":"valuable_loot","name":"X","quantity":"lots","totalCopper":1,"content":"X"}"#;
        assert_eq!(parse_alert_line(line), LineOutcome::Discard);
    }

    #[test]
    fn broken_json_is_discarded() {
        assert_eq!(parse_alert_line("{not json"), LineOutcome::Discard);
    }

    #[test]
    fn a_line_over_the_wire_cap_is_discarded_without_parsing() {
        let padding = "x".repeat(600);
        let line = format!(r#"{{"v":1,"kind":"valuable_loot","name":"{padding}","quantity":1,"totalCopper":1,"content":"X"}}"#);
        assert!(line.len() > MAX_ALERT_LINE_BYTES);
        assert_eq!(parse_alert_line(&line), LineOutcome::Discard);
    }

    #[test]
    fn an_empty_object_is_discarded() {
        assert_eq!(parse_alert_line("{}"), LineOutcome::Discard);
    }

    #[test]
    fn a_json_array_is_discarded() {
        assert_eq!(parse_alert_line("[1,2,3]"), LineOutcome::Discard);
    }
}

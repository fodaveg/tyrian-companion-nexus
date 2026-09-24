//! The wire contract fixed by `docs/SPEC-puente-ingame.md` in `tyrian-companion`, version 2.
//!
//! v2 is bidirectional and authenticated. The addon opens with a `hello` that carries the
//! per-installation token the user copies from the plugin's settings; the plugin answers with a
//! `welcome` (its `server` id and this connection's `nonce`), and from then on the addon reports
//! the game context (`context`), keeps the connection alive in the silences (`heartbeat`) and says
//! goodbye (`bye`), every one of those bound to the `welcome`'s nonce and to a sequence number that
//! starts at 0 and advances by exactly one. The plugin, for its part, sends `alert` lines and, as
//! the last line before it closes a connection it rejects, an `error` with a code.
//!
//! The plugin's validator (`src/alerts/alert-ingame-protocol.ts`) is strict: exact keys, no more
//! and no less, and any violation closes the connection. So every builder in this module checks
//! what it is about to send against the same rules and returns `None` rather than a line the
//! plugin would reject. Reading the plugin's lines is the opposite: the SPEC's "Tolerancia del
//! addon" says an unknown `type` is ignored and a known one with keys missing or extra is
//! discarded, both without closing, so [`parse_server_line`] never fails the connection.
//!
//! This module only builds and reads lines; it does not touch a socket (`crate::client` does).

use serde::Serialize;
use serde_json::{Map, Value};

/// Default port the plugin listens on. Matches the spec and the plugin's own default.
pub const DEFAULT_PORT: u16 = 47823;

/// The only protocol version this addon speaks.
pub const PROTOCOL_VERSION: u64 = 2;

/// Wire cap on one line, terminator excluded, in either direction (spec: "512 bytes como máximo
/// por línea, sin contar el terminador, en las dos direcciones").
pub const MAX_LINE_BYTES: usize = 512;

/// What the plugin puts in `heartbeatIntervalMs` today, used until a `welcome` says otherwise.
pub const DEFAULT_HEARTBEAT_INTERVAL_MS: u64 = 5_000;

/// Official map id of Mad King's Labyrinth. The addon sends it like any other map id; the
/// plugin is the one that labels a session with it.
pub const LABYRINTH_MAP_ID: u32 = 866;

/// Longest character name the plugin accepts, in characters (Unicode scalar values).
pub const CHARACTER_NAME_MAX_CHARS: usize = 32;

/// Bounds on the shared secret the plugin accepts at all (`isUsableIngameBridgeSecret`).
pub const TOKEN_MIN_CHARS: usize = 32;
pub const TOKEN_MAX_CHARS: usize = 128;

/// Largest `mapId` the plugin accepts (a signed 32-bit integer's maximum).
const MAP_ID_MAXIMUM: u32 = 2_147_483_647;

/// Longest `clientVersion` the plugin accepts.
const CLIENT_VERSION_MAX_CHARS: usize = 32;

/// Bytes in, and base64url characters out, of an `instance` id.
pub const INSTANCE_BYTES: usize = 16;
pub const INSTANCE_CHARS: usize = 22;

/// This addon's own name in the `hello` line.
const CLIENT_NAME: &str = "nexus";

/// Shown once when the plugin speaks a newer version than this addon does.
pub const UPDATE_ADDON_MESSAGE: &str = "Tyrian Companion: update the Nexus addon to see new alerts";

/// Shown once when the plugin refuses the token, until the user saves a new one. Says what to do,
/// not only what happened: the bare "rejected" of 0.2.0 left the player guessing.
pub const TOKEN_REJECTED_MESSAGE: &str = "Tyrian Companion: the plugin rejected the token. In Obsidian: Tyrian \
     Companion settings > \"Copy token\" (\"Copiar token\"), then paste it in Nexus options";

/// Status line of the options panel after an `auth_rejected`, with the same instructions.
pub const TOKEN_REJECTED_STATUS: &str = "Status: the plugin rejected the token. In Obsidian: Tyrian Companion \
     settings > \"Copy token\" (\"Copiar token\"), then paste it here and save";

/// Shown once per load when no usable token has been saved yet.
pub const TOKEN_MISSING_MESSAGE: &str =
    "Tyrian Companion: paste the addon token from Obsidian's settings in Nexus options";

const BASE64URL_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// `true` if `token` is a secret the plugin would accept at all (32 to 128 printable ASCII
/// characters, no spaces) and is not a Guild Wars 2 API key. Anything else is "no token": the
/// plugin rejects every `hello` without a usable secret, so the client does not even connect with
/// one, and an API key must never leave in a `hello` even though its shape passes the plugin's
/// format. See [`crate::token`].
pub fn is_usable_token(token: &str) -> bool {
    crate::token::is_plugin_format(token) && !crate::token::is_gw2_api_key(token)
}

/// Canonical base64url without padding, the encoding the plugin uses for its ids.
pub fn encode_base64url(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(chunk.get(1).copied().unwrap_or(0)) << 8)
            | u32::from(chunk.get(2).copied().unwrap_or(0));
        let emit = chunk.len() + 1;
        for index in 0..emit {
            let sextet = (value >> (18 - 6 * index)) & 63;
            encoded.push(char::from(BASE64URL_ALPHABET[sextet as usize]));
        }
    }
    encoded
}

/// `true` if `value` is exactly what [`encode_base64url`] makes of [`INSTANCE_BYTES`] bytes. The
/// plugin checks the round trip, so a 22-character string whose last character carries stray
/// low bits is refused there even though it decodes.
pub fn is_canonical_instance(value: &str) -> bool {
    if value.len() != INSTANCE_CHARS {
        return false;
    }
    let mut decoded = Vec::with_capacity(INSTANCE_BYTES);
    let mut accumulator: u32 = 0;
    let mut bits = 0;
    for byte in value.bytes() {
        let Some(sextet) = BASE64URL_ALPHABET.iter().position(|&candidate| candidate == byte) else {
            return false;
        };
        accumulator = (accumulator << 6) | sextet as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            decoded.push(((accumulator >> bits) & 0xff) as u8);
            accumulator &= (1 << bits) - 1;
        }
    }
    decoded.len() == INSTANCE_BYTES && encode_base64url(&decoded) == value
}

fn is_valid_client_version(value: &str) -> bool {
    (1..=CLIENT_VERSION_MAX_CHARS).contains(&value.len())
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'+' | b'-'))
}

/// Appends the terminator, refusing a line over the wire cap.
fn finish_line(body: String) -> Option<String> {
    if body.len() > MAX_LINE_BYTES {
        return None;
    }
    let mut line = body;
    line.push('\n');
    Some(line)
}

#[derive(Serialize)]
struct HelloLine<'a> {
    v: u64,
    #[serde(rename = "type")]
    kind: &'static str,
    client: &'static str,
    #[serde(rename = "clientVersion")]
    client_version: &'a str,
    instance: &'a str,
    token: &'a str,
}

/// Builds the `hello` line, `\n` included. `token` goes out verbatim; whether it is usable is the
/// caller's decision ([`is_usable_token`]), since a `hello` without one is pointless, not invalid.
///
/// Returns `None` if `client_version` or `instance` break the plugin's rules or the line would
/// exceed the cap: the plugin would close the connection over it, so it is never sent.
pub fn build_hello_line(client_version: &str, instance: &str, token: &str) -> Option<String> {
    if !is_valid_client_version(client_version) || !is_canonical_instance(instance) {
        return None;
    }
    let hello = HelloLine { v: PROTOCOL_VERSION, kind: "hello", client: CLIENT_NAME, client_version, instance, token };
    finish_line(serde_json::to_string(&hello).ok()?)
}

/// What the game is doing, as the three states the protocol knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GameState {
    /// A character is in the world.
    Gameplay,
    /// A loading screen.
    Loading,
    /// Character select or login.
    CharacterSelect,
}

/// One complete snapshot of the game context: the protocol sends it whole, never as a delta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameContext {
    pub state: GameState,
    /// `None` when there is no map to report.
    pub map_id: Option<u32>,
    /// `None` when there is no character, or its name could not be read cleanly.
    pub character: Option<String>,
}

impl GameContext {
    /// Character select with nothing else known: what the addon reports before the game has
    /// loaded a character, and what it reports when it cannot read anything at all.
    pub fn character_select() -> Self {
        Self { state: GameState::CharacterSelect, map_id: None, character: None }
    }
}

/// `true` if the plugin accepts `name` as a character: 1 to 32 characters, no leading or trailing
/// whitespace, no C0 or C1 control characters.
fn is_valid_character_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= CHARACTER_NAME_MAX_CHARS
        && trim_like_javascript(name) == name
        && !name.chars().any(is_control_character)
}

/// The control characters the plugin refuses in a name: `[\u0000-\u001f\u007f-\u009f]`.
fn is_control_character(character: char) -> bool {
    matches!(character, '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}')
}

/// Trims at least what JavaScript's `String.prototype.trim` trims, which is what the plugin
/// compares against: Unicode whitespace plus U+FEFF, which Rust's own `trim` leaves alone. Rust's
/// set is the wider of the two (it also counts U+0085), so a name that survives this has nothing
/// left at either end for the plugin to trim.
fn trim_like_javascript(value: &str) -> &str {
    value.trim_matches(|character: char| character.is_whitespace() || character == '\u{feff}')
}

/// Turns a raw character name into one the plugin accepts, or `None`. Trims, cuts at 32
/// characters (a Guild Wars 2 name is far shorter; the cut only bounds a garbled read), and gives
/// up on anything carrying a control character rather than guessing what it should have said.
pub fn sanitize_character_name(raw: &str) -> Option<String> {
    let trimmed = trim_like_javascript(raw);
    if trimmed.chars().any(is_control_character) {
        return None;
    }
    let cut: String = trimmed.chars().take(CHARACTER_NAME_MAX_CHARS).collect();
    let cut = trim_like_javascript(&cut);
    is_valid_character_name(cut).then(|| cut.to_string())
}

/// `Some(id)` if the plugin accepts `id` as a `mapId` (1 to 2147483647), `None` otherwise. Guild
/// Wars 2 writes 0 before the first map has loaded.
pub fn sanitize_map_id(id: u32) -> Option<u32> {
    (1..=MAP_ID_MAXIMUM).contains(&id).then_some(id)
}

#[derive(Serialize)]
struct ContextLine<'a> {
    v: u64,
    #[serde(rename = "type")]
    kind: &'static str,
    nonce: &'a str,
    seq: u64,
    state: GameState,
    #[serde(rename = "mapId")]
    map_id: Option<u32>,
    character: Option<&'a str>,
}

/// Builds a `context` line, `\n` included. `None` if the context itself would be refused (use
/// [`sanitize_map_id`] and [`sanitize_character_name`] to build it) or the line is over the cap.
pub fn build_context_line(nonce: &str, seq: u64, context: &GameContext) -> Option<String> {
    if context.map_id.is_some_and(|id| sanitize_map_id(id).is_none()) {
        return None;
    }
    if context.character.as_deref().is_some_and(|name| !is_valid_character_name(name)) {
        return None;
    }
    let line = ContextLine {
        v: PROTOCOL_VERSION,
        kind: "context",
        nonce,
        seq,
        state: context.state,
        map_id: context.map_id,
        character: context.character.as_deref(),
    };
    finish_line(serde_json::to_string(&line).ok()?)
}

#[derive(Serialize)]
struct HeartbeatLine<'a> {
    v: u64,
    #[serde(rename = "type")]
    kind: &'static str,
    nonce: &'a str,
    seq: u64,
}

/// Builds a `heartbeat` line, `\n` included.
pub fn build_heartbeat_line(nonce: &str, seq: u64) -> Option<String> {
    let line = HeartbeatLine { v: PROTOCOL_VERSION, kind: "heartbeat", nonce, seq };
    finish_line(serde_json::to_string(&line).ok()?)
}

/// Why the addon is leaving. Only `GameExit` tells the plugin the game is closing; it closes the
/// session at once, where any other end of a connection starts a ten-minute grace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ByeReason {
    /// Positive evidence only: the game window received `WM_CLOSE` or `WM_DESTROY`.
    GameExit,
    /// The addon is being unloaded without knowing whether the game goes on.
    AddonUnload,
}

#[derive(Serialize)]
struct ByeLine<'a> {
    v: u64,
    #[serde(rename = "type")]
    kind: &'static str,
    nonce: &'a str,
    seq: u64,
    reason: ByeReason,
}

/// Builds a `bye` line, `\n` included.
pub fn build_bye_line(nonce: &str, seq: u64, reason: ByeReason) -> Option<String> {
    let line = ByeLine { v: PROTOCOL_VERSION, kind: "bye", nonce, seq, reason };
    finish_line(serde_json::to_string(&line).ok()?)
}

/// The plugin's answer to an accepted `hello`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Welcome {
    /// Identifies this start of the plugin's server; alerts are deduplicated per `server`.
    pub server: String,
    /// Identifies this connection; every line the addon sends from now on carries it.
    pub nonce: String,
    /// Longest the addon may stay silent before it sends a `heartbeat`.
    pub heartbeat_interval_ms: u64,
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
/// optional extra panel, and `seq` to deduplicate within one `server`.
#[derive(Debug, Clone, PartialEq)]
pub struct Alert {
    /// The plugin always sends it in production; its own type still marks it optional, so an
    /// alert without one is shown and simply not deduplicated.
    pub seq: Option<u64>,
    pub kind: AlertKind,
    pub name: String,
    pub quantity: i64,
    pub total_copper: Option<i64>,
    pub content: String,
}

/// The `code` of an `error` line, which is always the last line before the plugin closes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorCode {
    /// The token is wrong: tell the user, and do not retry until the settings change.
    AuthRejected,
    /// The plugin speaks another version: ask for an update, and do not retry.
    VersionUnsupported,
    /// A deadline or the plugin's capacity: nothing wrong on this side, retry with backoff.
    HelloTimeout,
    LivenessTimeout,
    Capacity,
    /// `frame_*`, `nonce_mismatch`, `sequence_mismatch`, `unexpected_message`: a bug in this addon.
    /// Logged (the code only, never the token) and retried.
    AddonFault(String),
    /// A code this addon does not know yet: treated like a fault, logged and retried.
    Unknown(String),
}

impl ErrorCode {
    fn from_wire(code: &str) -> Self {
        match code {
            "auth_rejected" => Self::AuthRejected,
            "version_unsupported" => Self::VersionUnsupported,
            "hello_timeout" => Self::HelloTimeout,
            "liveness_timeout" => Self::LivenessTimeout,
            "capacity" => Self::Capacity,
            "frame_length" | "frame_utf8" | "frame_json" | "frame_schema" | "nonce_mismatch"
            | "sequence_mismatch" | "unexpected_message" => Self::AddonFault(code.to_string()),
            other => Self::Unknown(other.chars().take(32).collect()),
        }
    }

    /// The code as the plugin wrote it, for the log.
    pub fn as_str(&self) -> &str {
        match self {
            Self::AuthRejected => "auth_rejected",
            Self::VersionUnsupported => "version_unsupported",
            Self::HelloTimeout => "hello_timeout",
            Self::LivenessTimeout => "liveness_timeout",
            Self::Capacity => "capacity",
            Self::AddonFault(code) | Self::Unknown(code) => code,
        }
    }

    /// `false` for the two codes after which the spec forbids reconnecting until the user changes
    /// the addon's settings.
    pub fn retries(&self) -> bool {
        !matches!(self, Self::AuthRejected | Self::VersionUnsupported)
    }
}

/// What one line from the plugin resolves to.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerLine {
    Welcome(Welcome),
    Alert(Alert),
    Error(ErrorCode),
    /// `v` is greater than [`PROTOCOL_VERSION`]: ask for an update once, interpret nothing.
    UnsupportedVersion,
    /// A `type` this addon does not know. Ignored without closing, per the spec.
    Ignored,
    /// Oversized, not JSON, not an object, a `v` below 2, or a known `type` with keys missing,
    /// extra or of the wrong type. Dropped without closing, per the spec.
    Discard,
}

const WELCOME_KEYS: &[&str] = &["v", "type", "server", "nonce", "heartbeatIntervalMs"];
const ALERT_KEYS: &[&str] = &["v", "type", "kind", "name", "quantity", "totalCopper", "content"];
const ALERT_KEYS_WITH_SEQ: &[&str] = &["v", "type", "seq", "kind", "name", "quantity", "totalCopper", "content"];
const ERROR_KEYS: &[&str] = &["v", "type", "code"];

fn has_exact_keys(record: &Map<String, Value>, keys: &[&str]) -> bool {
    record.len() == keys.len() && keys.iter().all(|key| record.contains_key(*key))
}

/// An id the plugin issued (`server`, `nonce`): 1 to 64 base64url characters. The plugin makes
/// them 22 long; the looser bound here only keeps a strange one from reaching the socket.
fn as_wire_id(value: Option<&Value>) -> Option<String> {
    let text = value?.as_str()?;
    let valid = (1..=64).contains(&text.len())
        && text.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
    valid.then(|| text.to_string())
}

/// Parses one line from the plugin (without its trailing `\n`, which the framer strips).
///
/// Every failure resolves to [`ServerLine::Discard`], [`ServerLine::Ignored`] or
/// [`ServerLine::UnsupportedVersion`], never to an error the caller has to propagate: this
/// function cannot fail the connection. Only the plugin's `error` line ends it, and the plugin
/// closes the socket itself right after sending one.
pub fn parse_server_line(line: &str) -> ServerLine {
    let line = line.strip_suffix('\r').unwrap_or(line);
    if line.len() > MAX_LINE_BYTES {
        return ServerLine::Discard;
    }
    let Ok(Value::Object(record)) = serde_json::from_str::<Value>(line) else {
        return ServerLine::Discard;
    };
    let Some(version) = record.get("v").and_then(Value::as_u64) else {
        return ServerLine::Discard;
    };
    if version > PROTOCOL_VERSION {
        return ServerLine::UnsupportedVersion;
    }
    if version != PROTOCOL_VERSION {
        return ServerLine::Discard;
    }
    let Some(kind) = record.get("type").and_then(Value::as_str) else {
        return ServerLine::Discard;
    };
    let parsed = match kind {
        "welcome" => parse_welcome(&record).map(ServerLine::Welcome),
        "alert" => parse_alert(&record).map(ServerLine::Alert),
        "error" => parse_error(&record).map(ServerLine::Error),
        _ => return ServerLine::Ignored,
    };
    parsed.unwrap_or(ServerLine::Discard)
}

fn parse_welcome(record: &Map<String, Value>) -> Option<Welcome> {
    if !has_exact_keys(record, WELCOME_KEYS) {
        return None;
    }
    let heartbeat_interval_ms = record.get("heartbeatIntervalMs")?.as_u64().filter(|&ms| ms > 0)?;
    Some(Welcome {
        server: as_wire_id(record.get("server"))?,
        nonce: as_wire_id(record.get("nonce"))?,
        heartbeat_interval_ms,
    })
}

fn parse_alert(record: &Map<String, Value>) -> Option<Alert> {
    if !has_exact_keys(record, ALERT_KEYS) && !has_exact_keys(record, ALERT_KEYS_WITH_SEQ) {
        return None;
    }
    let seq = match record.get("seq") {
        None => None,
        Some(value) => Some(value.as_u64()?),
    };
    let total_copper = match record.get("totalCopper")? {
        Value::Null => None,
        value => Some(value.as_i64()?),
    };
    Some(Alert {
        seq,
        kind: AlertKind::from_wire(record.get("kind")?.as_str()?),
        name: record.get("name")?.as_str()?.to_string(),
        quantity: record.get("quantity")?.as_i64()?,
        total_copper,
        content: record.get("content")?.as_str()?.to_string(),
    })
}

fn parse_error(record: &Map<String, Value>) -> Option<ErrorCode> {
    if !has_exact_keys(record, ERROR_KEYS) {
        return None;
    }
    Some(ErrorCode::from_wire(record.get("code")?.as_str()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ids every example in `docs/SPEC-puente-ingame.md` uses.
    const SPEC_INSTANCE: &str = "q8Hq3n2t0dQyYf0nJ1p0Aw";
    const SPEC_SERVER: &str = "Pq0v4c3Wm9Xs1Ya7Tb2NeQ";
    const SPEC_NONCE: &str = "Zk3m1Qw9Lr0aT7yUc2Vb5g";

    fn without_terminator(line: Option<String>) -> String {
        let line = line.expect("the builder accepted the line");
        line.strip_suffix('\n').expect("every built line ends in a newline").to_string()
    }

    fn context(state: GameState, map_id: Option<u32>, character: Option<&str>) -> GameContext {
        GameContext { state, map_id, character: character.map(str::to_string) }
    }

    // --- Addon -> plugin, byte for byte against the SPEC's example lines ---

    #[test]
    fn hello_matches_the_spec_example() {
        assert_eq!(
            without_terminator(build_hello_line("0.2.0", SPEC_INSTANCE, "<token>")),
            r#"{"v":2,"type":"hello","client":"nexus","clientVersion":"0.2.0","instance":"q8Hq3n2t0dQyYf0nJ1p0Aw","token":"<token>"}"#,
        );
    }

    #[test]
    fn context_matches_the_spec_example() {
        let line = build_context_line(SPEC_NONCE, 0, &context(GameState::Gameplay, Some(LABYRINTH_MAP_ID), Some("Astra Uno")));
        assert_eq!(
            without_terminator(line),
            r#"{"v":2,"type":"context","nonce":"Zk3m1Qw9Lr0aT7yUc2Vb5g","seq":0,"state":"gameplay","mapId":866,"character":"Astra Uno"}"#,
        );
    }

    #[test]
    fn heartbeat_matches_the_spec_example() {
        assert_eq!(
            without_terminator(build_heartbeat_line(SPEC_NONCE, 5)),
            r#"{"v":2,"type":"heartbeat","nonce":"Zk3m1Qw9Lr0aT7yUc2Vb5g","seq":5}"#,
        );
    }

    #[test]
    fn bye_matches_the_spec_example() {
        assert_eq!(
            without_terminator(build_bye_line(SPEC_NONCE, 9, ByeReason::GameExit)),
            r#"{"v":2,"type":"bye","nonce":"Zk3m1Qw9Lr0aT7yUc2Vb5g","seq":9,"reason":"game_exit"}"#,
        );
        assert_eq!(
            without_terminator(build_bye_line(SPEC_NONCE, 3, ByeReason::AddonUnload)),
            r#"{"v":2,"type":"bye","nonce":"Zk3m1Qw9Lr0aT7yUc2Vb5g","seq":3,"reason":"addon_unload"}"#,
        );
    }

    #[test]
    fn the_spec_walkthrough_is_reproduced_line_for_line() {
        // "Ejemplo completo", addon lines only (the SPEC's `hello` there is Blish's; this addon's
        // own is covered above). The sequence is shared by context, heartbeat and bye.
        let n = SPEC_NONCE;
        let built = [
            build_context_line(n, 0, &context(GameState::CharacterSelect, None, None)),
            build_context_line(n, 1, &context(GameState::Loading, Some(50), Some("Astra Uno"))),
            build_context_line(n, 2, &context(GameState::Gameplay, Some(50), Some("Astra Uno"))),
            build_heartbeat_line(n, 3),
            build_context_line(n, 4, &context(GameState::Gameplay, Some(866), Some("Astra Uno"))),
            build_bye_line(n, 5, ByeReason::GameExit),
        ];
        let expected = [
            r#"{"v":2,"type":"context","nonce":"Zk3m1Qw9Lr0aT7yUc2Vb5g","seq":0,"state":"character_select","mapId":null,"character":null}"#,
            r#"{"v":2,"type":"context","nonce":"Zk3m1Qw9Lr0aT7yUc2Vb5g","seq":1,"state":"loading","mapId":50,"character":"Astra Uno"}"#,
            r#"{"v":2,"type":"context","nonce":"Zk3m1Qw9Lr0aT7yUc2Vb5g","seq":2,"state":"gameplay","mapId":50,"character":"Astra Uno"}"#,
            r#"{"v":2,"type":"heartbeat","nonce":"Zk3m1Qw9Lr0aT7yUc2Vb5g","seq":3}"#,
            r#"{"v":2,"type":"context","nonce":"Zk3m1Qw9Lr0aT7yUc2Vb5g","seq":4,"state":"gameplay","mapId":866,"character":"Astra Uno"}"#,
            r#"{"v":2,"type":"bye","nonce":"Zk3m1Qw9Lr0aT7yUc2Vb5g","seq":5,"reason":"game_exit"}"#,
        ];
        for (line, expected) in built.into_iter().zip(expected) {
            assert_eq!(without_terminator(line), expected);
        }
    }

    #[test]
    fn a_character_name_travels_as_utf8_not_as_escapes() {
        let line = without_terminator(build_context_line(SPEC_NONCE, 0, &context(GameState::Gameplay, Some(15), Some("Zoë Ärger"))));
        assert!(line.ends_with(r#""character":"Zoë Ärger"}"#), "{line}");
    }

    // --- Builders refuse what the plugin would refuse ---

    #[test]
    fn hello_refuses_a_non_canonical_instance_or_a_bad_client_version() {
        assert_eq!(build_hello_line("0.2.0", "q8Hq3n2t0dQyYf0nJ1p0Ax", "<token>"), None, "stray low bits");
        assert_eq!(build_hello_line("0.2.0", "short", "<token>"), None);
        assert_eq!(build_hello_line("0.2.0 beta", SPEC_INSTANCE, "<token>"), None);
        assert_eq!(build_hello_line("", SPEC_INSTANCE, "<token>"), None);
        assert_eq!(build_hello_line(&"1".repeat(33), SPEC_INSTANCE, "<token>"), None);
    }

    #[test]
    fn hello_with_the_longest_usable_token_still_fits_the_cap() {
        let token = "x".repeat(TOKEN_MAX_CHARS);
        let line = build_hello_line("0.2.0", SPEC_INSTANCE, &token).expect("fits");
        assert!(line.len() - 1 <= MAX_LINE_BYTES);
    }

    #[test]
    fn context_refuses_a_name_or_map_the_plugin_would_refuse() {
        assert_eq!(build_context_line(SPEC_NONCE, 0, &context(GameState::Gameplay, Some(0), None)), None);
        assert_eq!(build_context_line(SPEC_NONCE, 0, &context(GameState::Gameplay, Some(2_147_483_648), None)), None);
        assert_eq!(build_context_line(SPEC_NONCE, 0, &context(GameState::Gameplay, None, Some(" Astra"))), None);
        assert_eq!(build_context_line(SPEC_NONCE, 0, &context(GameState::Gameplay, None, Some(""))), None);
        assert_eq!(build_context_line(SPEC_NONCE, 0, &context(GameState::Gameplay, None, Some("a\u{7}b"))), None);
        assert_eq!(build_context_line(SPEC_NONCE, 0, &context(GameState::Gameplay, None, Some(&"a".repeat(33)))), None);
        assert!(build_context_line(SPEC_NONCE, 0, &context(GameState::Gameplay, Some(2_147_483_647), Some(&"a".repeat(32)))).is_some());
    }

    #[test]
    fn character_names_are_sanitized_to_what_the_plugin_accepts() {
        assert_eq!(sanitize_character_name("  Astra Uno \u{feff}").as_deref(), Some("Astra Uno"));
        assert_eq!(sanitize_character_name(""), None);
        assert_eq!(sanitize_character_name("   "), None);
        assert_eq!(sanitize_character_name("Astra\u{0}Uno"), None);
        assert_eq!(sanitize_character_name("Astra\u{85}"), Some("Astra".to_string()), "U+0085 is trimmed as whitespace first");
        let long = format!("{} tail", "é".repeat(31));
        assert_eq!(sanitize_character_name(&long).map(|name| name.chars().count()), Some(31), "cut at 32, then trimmed");
    }

    #[test]
    fn map_ids_outside_the_plugins_range_become_null() {
        assert_eq!(sanitize_map_id(0), None);
        assert_eq!(sanitize_map_id(866), Some(866));
        assert_eq!(sanitize_map_id(u32::MAX), None);
    }

    #[test]
    fn usable_tokens_are_32_to_128_printable_ascii_without_spaces() {
        assert!(is_usable_token(&"a".repeat(32)));
        assert!(is_usable_token(&"a".repeat(128)));
        assert!(is_usable_token("Pq0v4c3Wm9Xs1Ya7Tb2NeQ-_Pq0v4c3Wm9Xs1Ya7Tb2Ne"));
        assert!(!is_usable_token(&"a".repeat(31)));
        assert!(!is_usable_token(&"a".repeat(129)));
        assert!(!is_usable_token(&format!("{} {}", "a".repeat(20), "b".repeat(20))));
        assert!(!is_usable_token(&format!("{}é", "a".repeat(40))));
        assert!(!is_usable_token(""));
    }

    #[test]
    fn a_gw2_api_key_is_never_a_usable_token() {
        // 72 printable ASCII characters without spaces: the plugin's format alone would let it by.
        let api_key = "0A1B2C3D-4E5F-6071-8293-A4B5C6D7E8F90A1B2C3D-4E5F-6071-8293-A4B5C6D7E8F9";
        assert!(!is_usable_token(api_key));
        assert!(!is_usable_token(&api_key.to_lowercase()));
    }

    #[test]
    fn base64url_matches_the_plugins_encoding() {
        assert_eq!(encode_base64url(&[]), "");
        assert_eq!(encode_base64url(&[0xfb]), "-w");
        assert_eq!(encode_base64url(&[0xfb, 0xff]), "-_8");
        assert_eq!(encode_base64url(&[0xfb, 0xff, 0xbf]), "-_-_");
        assert_eq!(encode_base64url(&[0u8; 16]), "AAAAAAAAAAAAAAAAAAAAAA");
        assert!(is_canonical_instance(SPEC_INSTANCE));
        assert!(is_canonical_instance(&encode_base64url(&[0xa5; 16])));
    }

    // --- Plugin -> addon ---

    #[test]
    fn parses_the_spec_welcome() {
        let line = r#"{"v":2,"type":"welcome","server":"Pq0v4c3Wm9Xs1Ya7Tb2NeQ","nonce":"Zk3m1Qw9Lr0aT7yUc2Vb5g","heartbeatIntervalMs":5000}"#;
        assert_eq!(
            parse_server_line(line),
            ServerLine::Welcome(Welcome { server: SPEC_SERVER.into(), nonce: SPEC_NONCE.into(), heartbeat_interval_ms: 5000 }),
        );
    }

    #[test]
    fn parses_the_spec_alert() {
        let line = r#"{"v":2,"type":"alert","seq":17,"kind":"valuable_loot","name":"Mystic Coin","quantity":3,"totalCopper":123456,"content":"Mystic Coin ×3 · 12g 34s 56c"}"#;
        let ServerLine::Alert(alert) = parse_server_line(line) else { panic!("expected an alert") };
        assert_eq!(alert.seq, Some(17));
        assert_eq!(alert.kind, AlertKind::ValuableLoot);
        assert_eq!(alert.name, "Mystic Coin");
        assert_eq!(alert.quantity, 3);
        assert_eq!(alert.total_copper, Some(123456));
        assert_eq!(alert.content, "Mystic Coin \u{00d7}3 \u{00b7} 12g 34s 56c");
    }

    #[test]
    fn an_alert_without_a_quote_or_a_seq_still_shows() {
        let line = r#"{"v":2,"type":"alert","kind":"always_alert","name":"Rare Skin","quantity":1,"totalCopper":null,"content":"Rare Skin ×1 · no quoted value"}"#;
        let ServerLine::Alert(alert) = parse_server_line(line) else { panic!("expected an alert") };
        assert_eq!(alert.seq, None);
        assert_eq!(alert.total_copper, None);
        assert_eq!(alert.kind, AlertKind::AlwaysAlert);
    }

    #[test]
    fn an_unknown_kind_still_shows_the_alert() {
        let line = r#"{"v":2,"type":"alert","seq":1,"kind":"something_new","name":"X","quantity":1,"totalCopper":null,"content":"X"}"#;
        let ServerLine::Alert(alert) = parse_server_line(line) else { panic!("expected an alert") };
        assert_eq!(alert.kind, AlertKind::Unknown);
    }

    #[test]
    fn parses_every_error_code_in_the_spec() {
        let expectations = [
            ("auth_rejected", ErrorCode::AuthRejected, false),
            ("version_unsupported", ErrorCode::VersionUnsupported, false),
            ("hello_timeout", ErrorCode::HelloTimeout, true),
            ("liveness_timeout", ErrorCode::LivenessTimeout, true),
            ("capacity", ErrorCode::Capacity, true),
            ("frame_length", ErrorCode::AddonFault("frame_length".into()), true),
            ("frame_utf8", ErrorCode::AddonFault("frame_utf8".into()), true),
            ("frame_json", ErrorCode::AddonFault("frame_json".into()), true),
            ("frame_schema", ErrorCode::AddonFault("frame_schema".into()), true),
            ("nonce_mismatch", ErrorCode::AddonFault("nonce_mismatch".into()), true),
            ("sequence_mismatch", ErrorCode::AddonFault("sequence_mismatch".into()), true),
            ("unexpected_message", ErrorCode::AddonFault("unexpected_message".into()), true),
            ("brand_new", ErrorCode::Unknown("brand_new".into()), true),
        ];
        for (wire, expected, retries) in expectations {
            let line = format!(r#"{{"v":2,"type":"error","code":"{wire}"}}"#);
            assert_eq!(parse_server_line(&line), ServerLine::Error(expected.clone()), "{wire}");
            assert_eq!(expected.retries(), retries, "{wire}");
            assert_eq!(expected.as_str(), wire);
        }
    }

    #[test]
    fn an_unknown_type_is_ignored_not_discarded() {
        assert_eq!(parse_server_line(r#"{"v":2,"type":"news","anything":1}"#), ServerLine::Ignored);
    }

    #[test]
    fn a_known_type_with_a_key_too_many_or_too_few_is_discarded() {
        let extra = r#"{"v":2,"type":"alert","seq":1,"kind":"sell_signal","name":"X","quantity":1,"totalCopper":1,"content":"X","accountRef":"secret"}"#;
        assert_eq!(parse_server_line(extra), ServerLine::Discard);
        let missing = r#"{"v":2,"type":"alert","seq":1,"kind":"sell_signal","quantity":1,"totalCopper":1,"content":"X"}"#;
        assert_eq!(parse_server_line(missing), ServerLine::Discard);
        let welcome_extra = r#"{"v":2,"type":"welcome","server":"a","nonce":"b","heartbeatIntervalMs":5000,"x":1}"#;
        assert_eq!(parse_server_line(welcome_extra), ServerLine::Discard);
        let error_extra = r#"{"v":2,"type":"error","code":"capacity","detail":"x"}"#;
        assert_eq!(parse_server_line(error_extra), ServerLine::Discard);
    }

    #[test]
    fn a_wrong_type_for_a_present_field_is_discarded() {
        let line = r#"{"v":2,"type":"alert","seq":1,"kind":"valuable_loot","name":"X","quantity":"lots","totalCopper":1,"content":"X"}"#;
        assert_eq!(parse_server_line(line), ServerLine::Discard);
        let welcome = r#"{"v":2,"type":"welcome","server":"a","nonce":"b","heartbeatIntervalMs":0}"#;
        assert_eq!(parse_server_line(welcome), ServerLine::Discard);
        let hostile_nonce = r#"{"v":2,"type":"welcome","server":"a","nonce":"b\"}","heartbeatIntervalMs":5000}"#;
        assert_eq!(parse_server_line(hostile_nonce), ServerLine::Discard);
    }

    #[test]
    fn a_higher_version_asks_for_an_update_and_nothing_else() {
        assert_eq!(parse_server_line(r#"{"v":3,"type":"alert"}"#), ServerLine::UnsupportedVersion);
    }

    #[test]
    fn a_v1_alert_is_discarded_not_interpreted() {
        let line = r#"{"v":1,"seq":17,"kind":"valuable_loot","name":"Mystic Coin","quantity":3,"totalCopper":123456,"content":"X"}"#;
        assert_eq!(parse_server_line(line), ServerLine::Discard);
    }

    #[test]
    fn malformed_lines_are_discarded() {
        assert_eq!(parse_server_line("{not json"), ServerLine::Discard);
        assert_eq!(parse_server_line("[1,2,3]"), ServerLine::Discard);
        assert_eq!(parse_server_line("{}"), ServerLine::Discard);
        assert_eq!(parse_server_line(r#"{"type":"alert"}"#), ServerLine::Discard);
        assert_eq!(parse_server_line(r#"{"v":2}"#), ServerLine::Discard);
        let padding = "x".repeat(600);
        assert_eq!(parse_server_line(&format!(r#"{{"v":2,"type":"alert","name":"{padding}"}}"#)), ServerLine::Discard);
    }

    #[test]
    fn a_trailing_carriage_return_is_tolerated() {
        assert_eq!(parse_server_line("{\"v\":2,\"type\":\"error\",\"code\":\"capacity\"}\r"), ServerLine::Error(ErrorCode::Capacity));
    }
}

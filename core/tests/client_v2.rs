//! The real client loop (`client::run`, the one the addon runs in the game) against a fake
//! plugin that validates every line the way the plugin's own `alert-ingame-protocol.ts` and
//! `alert-ingame-server.ts` do: exact keys, no duplicate keys, the 512-byte cap, `client`,
//! `clientVersion`, a canonical `instance`, the token, and the nonce and exact sequence of every
//! line after the `welcome`. A line the plugin would close the connection over fails the test
//! with the plugin's own error code.

use std::collections::HashSet;
use std::fmt;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use serde_json::{Map, Value};

use tyrian_companion_nexus_core::client::{spawn, ClientConfig, ClientHandle, GameReading, Host};
use tyrian_companion_nexus_core::game_context::MumbleSnapshot;
use tyrian_companion_nexus_core::instance::new_instance_id;
use tyrian_companion_nexus_core::protocol::{
    is_canonical_instance, TOKEN_MISSING_MESSAGE, TOKEN_REJECTED_MESSAGE, UPDATE_ADDON_MESSAGE,
};
use tyrian_companion_nexus_core::state::{SharedState, Status};

const TOKEN: &str = "k2VnU0bq9mRjYp8tXwH3cL5sA7dF1gJ4hN6zQ0eT2uB";
const OTHER_TOKEN: &str = "differentTokenThatIsLongEnough-0123456789ab";
const SERVER_A: &str = "Pq0v4c3Wm9Xs1Ya7Tb2NeQ";
const SERVER_B: &str = "Bq0v4c3Wm9Xs1Ya7Tb2NeQ";
const NONCE_1: &str = "Zk3m1Qw9Lr0aT7yUc2Vb5g";
const NONCE_2: &str = "Yk3m1Qw9Lr0aT7yUc2Vb5g";
const NONCE_3: &str = "Xk3m1Qw9Lr0aT7yUc2Vb5g";
const ALERT_CONTENT: &str = "Mystic Coin ×3 · 12g 34s 56c";

// --- The fake plugin's validator, ported from `alert-ingame-protocol.ts` ---

/// Collects the top-level keys in order, so a duplicate can be seen (the plugin rejects one;
/// `serde_json::Map` would silently keep the last).
struct KeyList(Vec<String>);

impl<'de> Deserialize<'de> for KeyList {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct KeysVisitor;
        impl<'de> Visitor<'de> for KeysVisitor {
            type Value = KeyList;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<KeyList, A::Error> {
                let mut keys = Vec::new();
                while let Some((key, _)) = map.next_entry::<String, serde::de::IgnoredAny>()? {
                    keys.push(key);
                }
                Ok(KeyList(keys))
            }
        }
        deserializer.deserialize_map(KeysVisitor)
    }
}

/// `decodeIngameFrame`: one closed JSON object, at most 512 bytes, no BOM, no duplicate keys.
fn decode_frame(frame: &str) -> Result<Map<String, Value>, &'static str> {
    let frame = frame.strip_suffix('\r').unwrap_or(frame);
    if frame.is_empty() || frame.len() > 512 {
        return Err("frame_length");
    }
    if frame.starts_with('\u{feff}') {
        return Err("frame_utf8");
    }
    let Ok(Value::Object(record)) = serde_json::from_str::<Value>(frame) else { return Err("frame_json") };
    let keys = serde_json::from_str::<KeyList>(frame).map_err(|_| "frame_json")?;
    if keys.0.iter().collect::<HashSet<_>>().len() != keys.0.len() {
        return Err("frame_json");
    }
    Ok(record)
}

fn exact_keys(record: &Map<String, Value>, keys: &[&str]) -> bool {
    record.len() == keys.len() && keys.iter().all(|key| record.contains_key(*key))
}

/// `parseIngameHello`, plus the constant-time secret check the server runs right after it.
fn validate_hello(frame: &str, token: &str) -> Result<Map<String, Value>, &'static str> {
    let record = decode_frame(frame)?;
    match record.get("v") {
        Some(Value::Number(number)) if number.as_u64() != Some(2) => return Err("version_unsupported"),
        Some(Value::Number(_)) => {}
        _ => return Err("frame_schema"),
    }
    if record.get("type").and_then(Value::as_str) != Some("hello") {
        return Err("unexpected_message");
    }
    if !exact_keys(&record, &["v", "type", "client", "clientVersion", "instance", "token"]) {
        return Err("frame_schema");
    }
    let client_ok = matches!(record["client"].as_str(), Some("nexus" | "blish"));
    let version_ok = record["clientVersion"].as_str().is_some_and(|version| {
        (1..=32).contains(&version.len())
            && version.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'+' | b'-'))
    });
    let instance_ok = record["instance"].as_str().is_some_and(is_canonical_instance);
    if !client_ok || !version_ok || !instance_ok || !record["token"].is_string() {
        return Err("frame_schema");
    }
    if record["token"].as_str() != Some(token) {
        return Err("auth_rejected");
    }
    Ok(record)
}

/// `parseIngameSequenced`.
fn validate_sequenced(frame: &str, nonce: &str, seq: u64) -> Result<Map<String, Value>, &'static str> {
    let record = decode_frame(frame)?;
    if record.get("v").and_then(Value::as_u64) != Some(2) {
        return Err("frame_schema");
    }
    let keys: &[&str] = match record.get("type").and_then(Value::as_str) {
        Some("context") => &["v", "type", "nonce", "seq", "state", "mapId", "character"],
        Some("heartbeat") => &["v", "type", "nonce", "seq"],
        Some("bye") => &["v", "type", "nonce", "seq", "reason"],
        _ => return Err("unexpected_message"),
    };
    if !exact_keys(&record, keys) || !record["nonce"].is_string() || !record["seq"].is_u64() {
        return Err("frame_schema");
    }
    if record["nonce"].as_str() != Some(nonce) {
        return Err("nonce_mismatch");
    }
    if record["seq"].as_u64() != Some(seq) {
        return Err("sequence_mismatch");
    }
    match record["type"].as_str() {
        Some("bye") if !matches!(record["reason"].as_str(), Some("game_exit" | "addon_unload")) => Err("frame_schema"),
        Some("context") => {
            let state_ok = matches!(record["state"].as_str(), Some("gameplay" | "loading" | "character_select"));
            let map_ok = record["mapId"].is_null() || record["mapId"].as_u64().is_some_and(|id| (1..=2_147_483_647).contains(&id));
            let character_ok = match &record["character"] {
                Value::Null => true,
                Value::String(name) => {
                    !name.is_empty()
                        && name.chars().count() <= 32
                        && name.trim() == name
                        && !name.chars().any(|c| matches!(c, '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}'))
                }
                _ => false,
            };
            if state_ok && map_ok && character_ok { Ok(record) } else { Err("frame_schema") }
        }
        _ => Ok(record),
    }
}

// --- The fake plugin's sockets ---

struct FakePlugin {
    listener: TcpListener,
}

impl FakePlugin {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        listener.set_nonblocking(true).unwrap();
        Self { listener }
    }

    fn port(&self) -> u16 {
        self.listener.local_addr().unwrap().port()
    }

    fn try_accept(&self, within: Duration) -> Option<Connection> {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            match self.listener.accept() {
                Ok((stream, _)) => return Some(Connection::new(stream)),
                Err(error) if error.kind() == ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(20)),
                Err(error) => panic!("accept failed: {error}"),
            }
        }
        None
    }

    fn accept(&self) -> Connection {
        self.try_accept(Duration::from_secs(5)).expect("the client connected")
    }
}

struct Connection {
    writer: TcpStream,
    reader: BufReader<TcpStream>,
    nonce: String,
    next_seq: u64,
}

impl Connection {
    fn new(stream: TcpStream) -> Self {
        stream.set_nonblocking(false).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        Self { writer: stream.try_clone().unwrap(), reader: BufReader::new(stream), nonce: String::new(), next_seq: 0 }
    }

    /// One `\n`-terminated line, or `None` at end of stream.
    fn read_frame(&mut self) -> Option<String> {
        let mut line = String::new();
        let read = self.reader.read_line(&mut line).expect("a line within 5 s");
        if read == 0 {
            return None;
        }
        Some(line.strip_suffix('\n').expect("every frame ends in a newline").to_string())
    }

    fn send(&mut self, line: &str) {
        self.writer.write_all(format!("{line}\n").as_bytes()).unwrap();
    }

    /// Reads the `hello`, checks it like the plugin, and answers `welcome`.
    fn authenticate(&mut self, server: &str, nonce: &str, heartbeat_ms: u64) -> Map<String, Value> {
        let frame = self.read_frame().expect("a hello");
        let hello = validate_hello(&frame, TOKEN).unwrap_or_else(|code| panic!("plugin would answer {code} to {frame}"));
        self.nonce = nonce.to_string();
        self.next_seq = 0;
        self.send(&format!(r#"{{"v":2,"type":"welcome","server":"{server}","nonce":"{nonce}","heartbeatIntervalMs":{heartbeat_ms}}}"#));
        hello
    }

    /// Reads the next line and checks it like the plugin does after a `welcome`.
    fn expect_sequenced(&mut self) -> Map<String, Value> {
        let frame = self.read_frame().expect("a sequenced line");
        let record = validate_sequenced(&frame, &self.nonce, self.next_seq)
            .unwrap_or_else(|code| panic!("plugin would answer {code} to {frame}"));
        self.next_seq += 1;
        record
    }

    /// Like [`Connection::expect_sequenced`], skipping heartbeats (still validated, still counted
    /// in the sequence), for steps where timing decides whether one slips in first.
    fn expect_non_heartbeat(&mut self) -> Map<String, Value> {
        loop {
            let record = self.expect_sequenced();
            if record["type"] != "heartbeat" {
                return record;
            }
        }
    }

    fn send_alert(&mut self, seq: u64, content: &str) {
        self.send(&format!(
            r#"{{"v":2,"type":"alert","seq":{seq},"kind":"valuable_loot","name":"Mystic Coin","quantity":3,"totalCopper":123456,"content":"{content}"}}"#
        ));
    }

    fn assert_closed(&mut self) {
        assert_eq!(self.read_frame(), None, "the client closed the connection");
    }
}

// --- The host the client sees ---

#[derive(Clone, Default)]
struct TestHost {
    alerts: Arc<Mutex<Vec<String>>>,
    reading: Arc<Mutex<GameReading>>,
    exiting: Arc<AtomicBool>,
}

impl Host for TestHost {
    fn show_alert(&self, text: &str) {
        self.alerts.lock().unwrap().push(text.to_string());
    }
    fn read_game(&self) -> GameReading {
        self.reading.lock().unwrap().clone()
    }
    fn game_exiting(&self) -> bool {
        self.exiting.load(Ordering::Relaxed)
    }
}

impl TestHost {
    fn alerts(&self) -> Vec<String> {
        self.alerts.lock().unwrap().clone()
    }

    fn set_in_game(&self, map_id: u32, character: &str) {
        *self.reading.lock().unwrap() = GameReading {
            is_gameplay: Some(true),
            mumble: Some(MumbleSnapshot { ui_tick: 10, written_by_game: true, map_id, character: Some(character.into()) }),
        };
    }

    fn wait_for_alerts(&self, count: usize) -> Vec<String> {
        wait_until(|| self.alerts().len() >= count);
        self.alerts()
    }
}

fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "condition not met within 5 s");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn start_client(plugin: &FakePlugin, token: &str) -> (Arc<SharedState>, TestHost, ClientHandle) {
    let state = Arc::new(SharedState::new());
    state.apply_settings(plugin.port(), token);
    let host = TestHost::default();
    let config = ClientConfig { client_version: "0.2.0".into(), instance: new_instance_id() };
    let handle = spawn(Arc::clone(&state), host.clone(), config).expect("spawn the client thread");
    (state, host, handle)
}

// --- Scenarios ---

#[test]
fn a_full_session_follows_the_spec_walkthrough() {
    let plugin = FakePlugin::start();
    let (state, host, handle) = start_client(&plugin, TOKEN);
    let mut connection = plugin.accept();

    let hello = connection.authenticate(SERVER_A, NONCE_1, 300);
    assert_eq!(hello["client"], "nexus");
    assert_eq!(hello["clientVersion"], "0.2.0");

    // Right after the welcome: the context, whole. Nothing is in game yet.
    let first = connection.expect_sequenced();
    assert_eq!(first["type"], "context");
    assert_eq!((&first["state"], &first["mapId"], &first["character"]), (&Value::from("character_select"), &Value::Null, &Value::Null));
    wait_until(|| state.status() == Status::Connected);

    // The character enters the Labyrinth: a new context, next seq.
    host.set_in_game(866, "Astra Uno");
    let second = connection.expect_non_heartbeat();
    assert_eq!((&second["type"], &second["state"], &second["mapId"], &second["character"]),
        (&Value::from("context"), &Value::from("gameplay"), &Value::from(866), &Value::from("Astra Uno")));

    // Nothing changes: a heartbeat after the interval the welcome gave (300 ms here).
    let third = connection.expect_sequenced();
    assert_eq!(third["type"], "heartbeat");

    // Alerts: shown once per seq; unknown types and malformed known types change nothing.
    connection.send_alert(1, ALERT_CONTENT);
    connection.send_alert(1, ALERT_CONTENT);
    connection.send(r#"{"v":2,"type":"news","text":"ignored"}"#);
    connection.send(r#"{"v":2,"type":"alert","seq":9,"kind":"valuable_loot","name":"X","quantity":1,"totalCopper":1,"content":"extra key","accountRef":"x"}"#);
    connection.send_alert(2, "second");
    assert_eq!(host.wait_for_alerts(2), vec![ALERT_CONTENT.to_string(), "second".to_string()]);
    assert_eq!(state.history_snapshot().len(), 2);

    // Unloading the addon: `bye addon_unload` on the same sequence, then the socket closes.
    handle.stop();
    let last = connection.expect_non_heartbeat();
    assert_eq!((&last["type"], &last["reason"]), (&Value::from("bye"), &Value::from("addon_unload")));
    connection.assert_closed();
    assert_eq!(state.status(), Status::WaitingForPlugin);
}

#[test]
fn a_plugin_restart_is_deduplicated_by_server_and_keeps_the_instance() {
    let plugin = FakePlugin::start();
    let (_state, host, handle) = start_client(&plugin, TOKEN);

    let mut first = plugin.accept();
    let first_hello = first.authenticate(SERVER_A, NONCE_1, 5000);
    first.expect_sequenced();
    first.send_alert(5, "from the first server");
    host.wait_for_alerts(1);
    drop(first); // Obsidian restarts.

    // The new server starts its counter over: seq 1 must show.
    let mut second = plugin.accept();
    let second_hello = second.authenticate(SERVER_B, NONCE_2, 5000);
    assert_eq!(first_hello["instance"], second_hello["instance"], "one process, one instance");
    let context = second.expect_sequenced();
    assert_eq!(context["seq"], 0, "a new welcome starts the sequence over");
    second.send_alert(1, "from the second server");
    host.wait_for_alerts(2);
    drop(second); // The connection drops; the same server comes back.

    // Same server again: seq 1 was already shown, seq 2 was not.
    let mut third = plugin.accept();
    third.authenticate(SERVER_B, NONCE_3, 5000);
    third.expect_sequenced();
    third.send_alert(1, "duplicate");
    third.send_alert(2, "new");
    assert_eq!(host.wait_for_alerts(3), vec!["from the first server", "from the second server", "new"]);
    handle.stop();
}

#[test]
fn a_rejected_token_stops_retrying_until_the_settings_change() {
    let plugin = FakePlugin::start();
    let (state, host, handle) = start_client(&plugin, OTHER_TOKEN);

    let mut connection = plugin.accept();
    let frame = connection.read_frame().unwrap();
    assert_eq!(validate_hello(&frame, TOKEN).unwrap_err(), "auth_rejected");
    connection.send(r#"{"v":2,"type":"error","code":"auth_rejected"}"#);
    drop(connection);

    assert_eq!(host.wait_for_alerts(1), vec![TOKEN_REJECTED_MESSAGE]);
    wait_until(|| state.status() == Status::TokenRejected);
    assert!(plugin.try_accept(Duration::from_millis(1500)).is_none(), "no retry with the same token");

    // The user pastes the right token and saves: the client comes back at once.
    state.apply_settings(plugin.port(), TOKEN);
    let mut connection = plugin.accept();
    connection.authenticate(SERVER_A, NONCE_1, 5000);
    connection.expect_sequenced();
    wait_until(|| state.status() == Status::Connected);
    assert_eq!(host.alerts().len(), 1, "the rejection was shown once");
    handle.stop();
}

#[test]
fn an_unsupported_version_asks_for_an_update_and_does_not_retry() {
    let plugin = FakePlugin::start();
    let (state, host, handle) = start_client(&plugin, TOKEN);

    let mut connection = plugin.accept();
    connection.read_frame().unwrap();
    connection.send(r#"{"v":2,"type":"error","code":"version_unsupported"}"#);
    drop(connection);

    assert_eq!(host.wait_for_alerts(1), vec![UPDATE_ADDON_MESSAGE]);
    wait_until(|| state.status() == Status::UpdateRequired);
    assert!(plugin.try_accept(Duration::from_millis(1500)).is_none(), "no retry");
    handle.stop();
}

#[test]
fn a_retryable_error_reconnects_on_the_backoff() {
    let plugin = FakePlugin::start();
    let (_state, host, handle) = start_client(&plugin, TOKEN);

    let mut connection = plugin.accept();
    connection.read_frame().unwrap();
    connection.send(r#"{"v":2,"type":"error","code":"capacity"}"#);
    drop(connection);

    let mut again = plugin.try_accept(Duration::from_secs(3)).expect("reconnected after capacity");
    again.authenticate(SERVER_A, NONCE_1, 5000);
    again.expect_sequenced();
    assert!(host.alerts().is_empty(), "a retryable error shows nothing to the player");
    handle.stop();
}

#[test]
fn a_closing_game_says_game_exit_and_does_not_reconnect() {
    let plugin = FakePlugin::start();
    let (state, host, handle) = start_client(&plugin, TOKEN);
    let mut connection = plugin.accept();
    connection.authenticate(SERVER_A, NONCE_1, 5000);
    connection.expect_sequenced();

    host.exiting.store(true, Ordering::Relaxed);
    let bye = connection.expect_sequenced();
    assert_eq!((&bye["type"], &bye["reason"]), (&Value::from("bye"), &Value::from("game_exit")));
    connection.assert_closed();
    wait_until(|| state.status() == Status::GameExiting);
    assert!(plugin.try_accept(Duration::from_millis(1000)).is_none(), "no reconnection while the game closes");
    handle.stop();
}

#[test]
fn without_a_usable_token_it_does_not_connect_and_says_so_once() {
    let plugin = FakePlugin::start();
    let (state, host, handle) = start_client(&plugin, "too-short");
    assert_eq!(host.wait_for_alerts(1), vec![TOKEN_MISSING_MESSAGE]);
    assert_eq!(state.status(), Status::MissingToken);
    assert!(plugin.try_accept(Duration::from_millis(1000)).is_none(), "nothing to authenticate with");
    assert_eq!(host.alerts().len(), 1);
    handle.stop();
}

#[test]
fn a_gw2_api_key_in_the_token_setting_is_never_sent_in_a_hello() {
    // It passes the plugin's format (72 printable characters), so only the addon can hold it back.
    let api_key = "0A1B2C3D-4E5F-6071-8293-A4B5C6D7E8F90A1B2C3D-4E5F-6071-8293-A4B5C6D7E8F9";
    let plugin = FakePlugin::start();
    let (state, host, handle) = start_client(&plugin, api_key);
    assert_eq!(host.wait_for_alerts(1), vec![TOKEN_MISSING_MESSAGE]);
    assert_eq!(state.status(), Status::MissingToken);
    assert!(plugin.try_accept(Duration::from_millis(1000)).is_none(), "the API key went out in a hello");
    handle.stop();
}

#[test]
fn the_fake_plugin_validator_catches_what_the_plugin_rejects() {
    // Controls for the validator above: if it accepted these, the scenarios would prove nothing.
    let instance = "q8Hq3n2t0dQyYf0nJ1p0Aw";
    let hello = |extra: &str| format!(r#"{{"v":2,"type":"hello","client":"nexus","clientVersion":"0.2.0","instance":"{instance}","token":"{TOKEN}"{extra}}}"#);
    assert!(validate_hello(&hello(""), TOKEN).is_ok());
    assert_eq!(validate_hello(&hello(r#","x":1"#), TOKEN).unwrap_err(), "frame_schema");
    assert_eq!(validate_hello(&hello(r#","v":2"#), TOKEN).unwrap_err(), "frame_json");
    assert_eq!(validate_hello(&hello(""), OTHER_TOKEN).unwrap_err(), "auth_rejected");
    assert_eq!(validate_hello(r#"{"v":1,"client":"nexus","clientVersion":"0.1.0"}"#, TOKEN).unwrap_err(), "version_unsupported");
    let context = r#"{"v":2,"type":"context","nonce":"N","seq":0,"state":"gameplay","mapId":866,"character":"Astra Uno"}"#;
    assert!(validate_sequenced(context, "N", 0).is_ok());
    assert_eq!(validate_sequenced(context, "M", 0).unwrap_err(), "nonce_mismatch");
    assert_eq!(validate_sequenced(context, "N", 1).unwrap_err(), "sequence_mismatch");
    let spaced = r#"{"v":2,"type":"context","nonce":"N","seq":0,"state":"gameplay","mapId":866,"character":" Astra"}"#;
    assert_eq!(validate_sequenced(spaced, "N", 0).unwrap_err(), "frame_schema");
}

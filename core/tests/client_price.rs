//! Real loopback transport for the `price1` handshake: one subscription, only after `price_cap`.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tyrian_companion_nexus_core::client::{spawn, ClientConfig, ClientHandle, GameReading, Host};
use tyrian_companion_nexus_core::obsidian_launch::{LaunchApp, ObsidianLaunchOutcome};
use tyrian_companion_nexus_core::state::SharedState;

const TOKEN: &str = "k2VnU0bq9mRjYp8tXwH3cL5sA7dF1gJ4hN6zQ0eT2uB";
const NONCE: &str = "AQEBAQEBAQEBAQEBAQEBAQ";
const SERVER: &str = "AgICAgICAgICAgICAgICAg";

#[derive(Clone, Default)]
struct FakeHost {
    alerts: Arc<Mutex<Vec<String>>>,
}

impl Host for FakeHost {
    fn show_alert(&self, text: &str) {
        self.alerts.lock().unwrap().push(text.into());
    }
    fn read_game(&self) -> GameReading {
        GameReading::default()
    }
    fn game_exiting(&self) -> bool {
        false
    }
    fn open_app(&self, _: LaunchApp) -> ObsidianLaunchOutcome {
        ObsidianLaunchOutcome::Launched
    }
}

struct Peer {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl Peer {
    fn new(stream: TcpStream) -> Self {
        stream
            .set_read_timeout(Some(Duration::from_millis(300)))
            .unwrap();
        Self {
            writer: stream.try_clone().unwrap(),
            reader: BufReader::new(stream),
        }
    }
    fn send(&mut self, value: Value) {
        writeln!(self.writer, "{value}").unwrap();
    }
    /// Everything the client sends for `window`, minus heartbeats and contexts.
    fn drain(&mut self, window: Duration) -> Vec<Value> {
        let end = Instant::now() + window;
        let mut lines = Vec::new();
        while Instant::now() < end {
            let mut line = String::new();
            match self.reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => lines.push(serde_json::from_str::<Value>(&line).unwrap()),
                Err(_) => {}
            }
        }
        lines
    }
    fn welcome(&mut self, version: u64) {
        let mut hello = String::new();
        self.reader.read_line(&mut hello).unwrap();
        assert!(hello.contains("\"hello\""));
        self.send(json!({"v":version,"type":"welcome","server":SERVER,"nonce":NONCE,"heartbeatIntervalMs":100}));
    }
}

fn start(listener: &TcpListener) -> (FakeHost, Arc<SharedState>, ClientHandle) {
    let state = Arc::new(SharedState::new());
    state.apply_settings(
        listener.local_addr().unwrap().port(),
        TOKEN,
        false,
        LaunchApp::Obsidian,
    );
    let host = FakeHost::default();
    let handle = spawn(
        state.clone(),
        host.clone(),
        ClientConfig {
            client_version: "0.7.0".into(),
            instance: tyrian_companion_nexus_core::instance::new_instance_id(),
        },
    )
    .unwrap();
    (host, state, handle)
}

fn of_type<'a>(lines: &'a [Value], kind: &str) -> Vec<&'a Value> {
    lines.iter().filter(|line| line["type"] == kind).collect()
}

fn price_state(seq: u64) -> Value {
    json!({"v":3,"type":"price_state","tag":"price1","nonce":NONCE,"seq":seq,"ttl":15,"st":"ok",
        "sell":293,"sellStack":73312,"list":312,"listStack":77987,"age":412})
}

#[test]
fn subscribes_once_after_price_cap_on_the_shared_sequence_and_paints_state() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (_host, state, handle) = start(&listener);
    let mut peer = Peer::new(listener.accept().unwrap().0);
    peer.welcome(3);
    peer.send(json!({"v":3,"type":"price_cap","nonce":NONCE,"tag":"price1"}));
    peer.send(json!({"v":3,"type":"price_cap","nonce":NONCE,"tag":"price1"}));
    peer.send(price_state(1));
    let lines = peer.drain(Duration::from_millis(900));
    let subs = of_type(&lines, "price_sub");
    assert_eq!(subs.len(), 1, "{lines:?}");
    assert_eq!(
        subs[0],
        &json!({"v":3,"type":"price_sub","nonce":NONCE,"seq":subs[0]["seq"],"tag":"price1"})
    );
    // It took its place in the one outgoing sequence, with no gap and no repeat.
    let seqs: Vec<u64> = lines
        .iter()
        .map(|line| line["seq"].as_u64().unwrap())
        .collect();
    assert_eq!(seqs, (0..seqs.len() as u64).collect::<Vec<_>>());
    assert!(of_type(&lines, "farming_sub").is_empty());
    let view = state.price_view(Instant::now());
    assert!(view.capable && view.fresh);
    handle.stop();
    assert!(
        !state.price_view(Instant::now()).capable,
        "disconnect clears the capability"
    );
}

#[test]
fn a_v3_server_that_never_sends_price_cap_never_receives_price_sub() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (_host, state, handle) = start(&listener);
    let mut peer = Peer::new(listener.accept().unwrap().0);
    peer.welcome(3);
    // Even an unsolicited state is ignored without a capability.
    peer.send(price_state(1));
    let lines = peer.drain(Duration::from_millis(900));
    assert!(of_type(&lines, "price_sub").is_empty(), "{lines:?}");
    assert!(state.price_view(Instant::now()).reading.is_none());
    handle.stop();
}

#[test]
fn farming_and_alert_deduplication_are_unchanged_next_to_price() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (host, _state, handle) = start(&listener);
    let mut peer = Peer::new(listener.accept().unwrap().0);
    peer.welcome(3);
    peer.send(json!({"v":3,"type":"farming_cap","nonce":NONCE,"tag":"farm1"}));
    peer.send(json!({"v":3,"type":"price_cap","nonce":NONCE,"tag":"price1"}));
    let alert = json!({"v":3,"type":"alert","seq":1,"kind":"valuable_loot","name":"Fixture","quantity":2,"totalCopper":20,"content":"Aviso"});
    peer.send(alert.clone());
    peer.send(alert);
    let lines = peer.drain(Duration::from_millis(900));
    assert_eq!(of_type(&lines, "farming_sub").len(), 1, "{lines:?}");
    assert_eq!(of_type(&lines, "price_sub").len(), 1, "{lines:?}");
    assert_eq!(of_type(&lines, "alert_ack").len(), 1, "{lines:?}");
    assert_eq!(host.alerts.lock().unwrap().as_slice(), ["Aviso"]);
    handle.stop();
}

//! Real loopback transport for the same worker that runs inside the Nexus DLL.
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tyrian_companion_nexus_core::client::{spawn, ClientConfig, ClientHandle, GameReading, Host};
use tyrian_companion_nexus_core::game_context::MumbleSnapshot;
use tyrian_companion_nexus_core::inventory::{InventorySnapshot, ReadError};
use tyrian_companion_nexus_core::obsidian_launch::{LaunchApp, ObsidianLaunchOutcome};
use tyrian_companion_nexus_core::state::SharedState;
use tyrian_companion_nexus_core::wallet::WalletSnapshot;
const TOKEN: &str = "k2VnU0bq9mRjYp8tXwH3cL5sA7dF1gJ4hN6zQ0eT2uB";
const NONCE: &str = "AQEBAQEBAQEBAQEBAQEBAQ";
const SERVER: &str = "AgICAgICAgICAgICAgICAg";
#[derive(Clone)]
struct FakeHost {
    quantity: Arc<AtomicU32>,
    calls: Arc<AtomicU32>,
    game: Arc<Mutex<GameReading>>,
    alerts: Arc<Mutex<Vec<String>>>,
    change_on_capture: Arc<AtomicBool>,
    block_capture: Arc<AtomicBool>,
    capture_started: Arc<AtomicBool>,
    /// What the wallet read of the next capture yields; `None` is a failed or absent read.
    wallet: Arc<Mutex<Option<WalletSnapshot>>>,
}
impl FakeHost {
    fn new() -> Self {
        Self {
            quantity: Arc::new(AtomicU32::new(0)),
            calls: Arc::new(AtomicU32::new(0)),
            alerts: Arc::new(Mutex::new(vec![])),
            change_on_capture: Arc::new(AtomicBool::new(false)),
            block_capture: Arc::new(AtomicBool::new(false)),
            capture_started: Arc::new(AtomicBool::new(false)),
            wallet: Arc::new(Mutex::new(None)),
            game: Arc::new(Mutex::new(GameReading {
                is_gameplay: Some(true),
                mumble: Some(MumbleSnapshot {
                    ui_tick: 1,
                    written_by_game: true,
                    map_id: 866,
                    character: Some("Fixture Character".into()),
                }),
            })),
        }
    }
}
impl Host for FakeHost {
    fn show_alert(&self, text: &str) {
        self.alerts.lock().unwrap().push(text.into());
    }
    fn read_game(&self) -> GameReading {
        self.game.lock().unwrap().clone()
    }
    fn read_inventory(&self, stop: &AtomicBool) -> Result<InventorySnapshot, ReadError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.capture_started.store(true, Ordering::Relaxed);
        if self.block_capture.load(Ordering::Relaxed) {
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(5));
            }
            return Err(ReadError::ReadFailed);
        }
        let quantity = self.quantity.load(Ordering::Relaxed);
        if self.change_on_capture.swap(false, Ordering::Relaxed) {
            self.game.lock().unwrap().mumble.as_mut().unwrap().character =
                Some("Changed Character".into());
            self.quantity.store(2, Ordering::Relaxed);
        }
        Ok(InventorySnapshot {
            owner: (0x10a000, 0x10b000),
            quantities: BTreeMap::from([(12147, quantity)]),
            unknown: 0,
            positions: 570,
            free_slots: None,
            wallet: self.wallet.lock().unwrap().clone(),
        })
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
    seq: u64,
    nonce: String,
}
impl Peer {
    fn new(stream: TcpStream) -> Self {
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        Self {
            writer: stream.try_clone().unwrap(),
            reader: BufReader::new(stream),
            seq: 0,
            nonce: NONCE.into(),
        }
    }
    fn send(&mut self, value: Value) {
        writeln!(self.writer, "{value}").unwrap();
    }
    fn read(&mut self) -> Value {
        let mut line = String::new();
        assert!(self.reader.read_line(&mut line).unwrap() > 0);
        assert!(line.trim_end().len() <= 512);
        serde_json::from_str(&line).unwrap()
    }
    fn auth(&mut self, version: u64, cap: bool) {
        self.auth_nonce(version, cap, NONCE);
    }
    fn auth_nonce(&mut self, version: u64, cap: bool, nonce: &str) {
        self.nonce = nonce.into();
        let hello = self.read();
        assert_eq!(hello["type"], "hello");
        assert_eq!(hello["token"], TOKEN);
        assert_eq!(hello["client"], "nexus");
        self.send(json!({"v":version,"type":"welcome","server":SERVER,"nonce":self.nonce,"heartbeatIntervalMs":100}));
        if cap {
            self.send(json!({"v":3,"type":"live_cap","nonce":self.nonce,"tag":"live1"}));
        }
    }
    fn sequenced(&mut self) -> Value {
        let v = self.read();
        assert_eq!(v["nonce"], self.nonce);
        assert_eq!(v["seq"], self.seq);
        self.seq += 1;
        if v["type"].as_str().unwrap().starts_with("live_") {
            assert_eq!(v["tag"], "live1");
        }
        v
    }
    fn next(&mut self) -> Value {
        loop {
            let v = self.sequenced();
            if v["type"] != "heartbeat" {
                return v;
            }
        }
    }
    fn open(&mut self) -> String {
        let v = self.next();
        assert_eq!(v["type"], "live_open");
        v["epoch"].as_str().unwrap().into()
    }
    fn ready(&mut self, epoch: &str) {
        self.send(json!({"v":3,"type":"live_ready","nonce":self.nonce,"tag":"live1","epoch":epoch,"status":"ready"}));
    }
    fn ack(&mut self, epoch: &str, cursor: u64, status: &str) {
        self.send(json!({"v":3,"type":"live_ack","nonce":self.nonce,"tag":"live1","epoch":epoch,"cursor":cursor,"status":status}));
    }
    fn sample(&mut self, epoch: &str, cursor: u64, quantity: u32) {
        self.sample_ctx(epoch, cursor, quantity, 0);
    }
    fn sample_ctx(&mut self, epoch: &str, cursor: u64, quantity: u32, ctx: u64) {
        let begin = self.next();
        assert_eq!(begin["type"], "live_begin");
        assert_eq!(begin["epoch"], epoch);
        assert_eq!(begin["cursor"], cursor);
        assert_eq!(begin["ctx"], ctx);
        assert_eq!(begin["items"], "complete");
        assert_eq!(begin["currencies"], "none");
        assert_eq!(begin["slots"], Value::Null);
        assert_eq!(begin["rows"], 1);
        let rows = self.next();
        assert_eq!(rows["type"], "live_rows");
        assert_eq!(rows["rows"], json!([[0, 12147, quantity]]));
        let end = self.next();
        assert_eq!(end["type"], "live_end");
        assert_eq!(end["cursor"], cursor);
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
    let host = FakeHost::new();
    let handle = spawn(
        state.clone(),
        host.clone(),
        ClientConfig {
            client_version: "0.4.0".into(),
            instance: tyrian_companion_nexus_core::instance::new_instance_id(),
        },
    )
    .unwrap();
    (host, state, handle)
}
#[test]
fn real_worker_negotiates_zero_two_four_waits_for_ack_and_interleaves_alert_ack() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (host, _state, handle) = start(&listener);
    let mut p = Peer::new(listener.accept().unwrap().0);
    p.auth(3, true);
    let ctx = p.next();
    assert_eq!(ctx["type"], "context");
    assert_eq!(ctx["seq"], 0);
    let epoch = p.open();
    p.ready(&epoch);
    p.sample(&epoch, 0, 0);
    host.quantity.store(2, Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(1250));
    assert_eq!(host.calls.load(Ordering::Relaxed), 1);
    p.send(json!({"v":3,"type":"alert","seq":1,"kind":"valuable_loot","name":"Fixture","quantity":2,"totalCopper":20,"content":"Aumento observado: fixture"}));
    let ack = p.next();
    assert_eq!(ack["type"], "alert_ack");
    assert_eq!(ack["alertSeq"], 1);
    assert_eq!(
        host.alerts.lock().unwrap().as_slice(),
        ["Aumento observado: fixture"]
    );
    p.ack(&epoch, 0, "stored");
    p.sample(&epoch, 1, 2);
    host.quantity.store(4, Ordering::Relaxed);
    p.ack(&epoch, 1, "stored");
    p.sample(&epoch, 2, 4);
    p.ack(&epoch, 2, "stored");
    handle.stop();
}
#[test]
fn real_worker_lists_wallet_rows_and_a_lost_wallet_keeps_the_item_sample_epoch_and_status() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (host, _state, handle) = start(&listener);
    let wallet = |magic: u32| {
        WalletSnapshot::checked((0x210000, 0x220000), BTreeMap::from([(1, 0), (45, magic)]))
    };
    *host.wallet.lock().unwrap() = wallet(10214);
    let mut p = Peer::new(listener.accept().unwrap().0);
    p.auth(3, true);
    assert_eq!(p.next()["type"], "context");
    let epoch = p.open();
    p.ready(&epoch);
    let listed = |p: &mut Peer, cursor: u64, mode: &str, rows: Value| {
        let begin = p.next();
        assert_eq!(begin["type"], "live_begin");
        assert_eq!(begin["epoch"], epoch.as_str());
        assert_eq!(begin["cursor"], cursor);
        assert_eq!(begin["mode"], mode);
        assert_eq!(begin["items"], "complete");
        assert_eq!(begin["currencies"], "listed");
        assert_eq!(begin["rows"], 3);
        let part = p.next();
        assert_eq!(part["type"], "live_rows");
        assert_eq!(part["rows"], rows);
        assert_eq!(p.next()["type"], "live_end");
    };
    listed(
        &mut p,
        0,
        "baseline",
        json!([[0, 12147, 0], [1, 1, 0], [1, 45, 10214]]),
    );
    // The next wallet read fails while the inventory read succeeds: same epoch, next cursor,
    // `currencies:none` with item rows only, and no `live_status` in between.
    *host.wallet.lock().unwrap() = None;
    host.quantity.store(2, Ordering::Relaxed);
    p.ack(&epoch, 0, "stored");
    p.sample(&epoch, 1, 2);
    *host.wallet.lock().unwrap() = wallet(10225);
    p.ack(&epoch, 1, "stored");
    listed(
        &mut p,
        2,
        "sample",
        json!([[0, 12147, 2], [1, 1, 0], [1, 45, 10225]]),
    );
    p.ack(&epoch, 2, "stored");
    handle.stop();
}
#[test]
fn v2_welcome_even_with_v3_capability_never_reads_inventory() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (host, _state, handle) = start(&listener);
    let mut p = Peer::new(listener.accept().unwrap().0);
    p.auth(2, true);
    assert_eq!(p.next()["type"], "context");
    for _ in 0..5 {
        assert_eq!(p.sequenced()["type"], "heartbeat");
    }
    assert_eq!(host.calls.load(Ordering::Relaxed), 0);
    handle.stop();
}
#[test]
fn reconnect_uses_new_nonce_epoch_and_baseline_without_replaying_prior_increase() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (host, _state, handle) = start(&listener);
    let mut p = Peer::new(listener.accept().unwrap().0);
    p.auth(3, true);
    p.next();
    let first = p.open();
    p.ready(&first);
    p.sample(&first, 0, 0);
    p.ack(&first, 0, "stored");
    drop(p);
    host.quantity.store(9, Ordering::Relaxed);
    let mut p = Peer::new(listener.accept().unwrap().0);
    p.auth_nonce(3, true, "AwMDAwMDAwMDAwMDAwMDAw");
    assert_eq!(p.next()["type"], "context");
    let second = p.open();
    assert_ne!(first, second);
    p.ready(&second);
    p.sample(&second, 0, 9);
    p.ack(&second, 0, "stored");
    handle.stop();
}

#[test]
fn context_change_during_capture_discards_sample_before_live_open() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (host, _state, handle) = start(&listener);
    host.quantity.store(99, Ordering::Relaxed);
    host.change_on_capture.store(true, Ordering::Relaxed);
    let mut p = Peer::new(listener.accept().unwrap().0);
    p.auth(3, true);
    assert_eq!(p.next()["type"], "context");
    let changed = p.next();
    assert_eq!(changed["type"], "context");
    assert_eq!(changed["character"], "Changed Character");
    assert_eq!(changed["seq"], 1);
    let epoch = p.open();
    p.ready(&epoch);
    p.sample_ctx(&epoch, 0, 2, 1);
    p.ack(&epoch, 0, "stored");
    handle.stop();
}

#[test]
fn source_conflict_is_retried_on_the_same_connection_without_a_context_change() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let state = Arc::new(SharedState::new());
    state.apply_settings(
        listener.local_addr().unwrap().port(),
        TOKEN,
        false,
        LaunchApp::Obsidian,
    );
    // The real wait is 30 s; the loopback test shortens it through the shared state.
    state.set_live_conflict_retry(Some(Duration::from_millis(400)));
    let host = FakeHost::new();
    let handle = spawn(
        state.clone(),
        host.clone(),
        ClientConfig {
            client_version: "0.4.0".into(),
            instance: tyrian_companion_nexus_core::instance::new_instance_id(),
        },
    )
    .unwrap();
    let mut p = Peer::new(listener.accept().unwrap().0);
    p.auth(3, true);
    assert_eq!(p.next()["type"], "context");
    let first = p.open();
    p.send(json!({"v":3,"type":"live_ready","nonce":p.nonce,"tag":"live1","epoch":first,"status":"source_conflict"}));
    // Between the conflict and the retry the game is not read, and the panel keeps the conflict.
    std::thread::sleep(Duration::from_millis(250));
    assert_eq!(host.calls.load(Ordering::Relaxed), 1);
    assert_eq!(
        state.live_status(),
        tyrian_companion_nexus_core::live::LiveStatus::Conflict
    );
    host.quantity.store(7, Ordering::Relaxed);
    // Heartbeats keep arriving, so a missing retry must fail by deadline, not by read timeout.
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let second = loop {
        let line = p.sequenced();
        if line["type"] == "live_open" {
            break line["epoch"].as_str().unwrap().to_string();
        }
        assert_eq!(line["type"], "heartbeat");
        assert!(
            std::time::Instant::now() < deadline,
            "no second live_open after the conflict wait"
        );
    };
    assert_ne!(first, second);
    assert_eq!(host.calls.load(Ordering::Relaxed), 2);
    p.ready(&second);
    p.sample(&second, 0, 7);
    p.ack(&second, 0, "stored");
    handle.stop();
}

#[test]
fn unload_cancels_pending_capture_without_sending_status_or_inventory_after_bye() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (host, _state, handle) = start(&listener);
    host.block_capture.store(true, Ordering::Relaxed);
    let mut p = Peer::new(listener.accept().unwrap().0);
    p.auth(3, true);
    assert_eq!(p.next()["type"], "context");
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while !host.capture_started.load(Ordering::Relaxed) {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let started = std::time::Instant::now();
    handle.stop();
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(p.next()["type"], "bye");
}

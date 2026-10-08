//! Real loopback transport for the same worker that runs inside the Nexus DLL.
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tyrian_companion_nexus_core::client::{spawn, ClientConfig, ClientHandle, GameReading, Host, Readiness};
use tyrian_companion_nexus_core::game_context::MumbleSnapshot;
use tyrian_companion_nexus_core::inventory::{Diagnostics, InventorySnapshot, ReadError};
use tyrian_companion_nexus_core::magic_find::{MagicFind, MagicFindCoverage};
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
    /// How many more times the source is not ready to sample: the adapter still verifying the
    /// game's executable, a slice at a time.
    pending: Arc<AtomicU32>,
    /// How many times the loop asked whether it is ready.
    prepared: Arc<AtomicU32>,
    /// From which of those times on a source that is not ready has been getting ready for too
    /// long, as the adapter's hash past `executable::OVERDUE`. Never, unless a test says.
    overdue_from: Arc<AtomicU32>,
    /// What the verification comes to once it is done: `None` is a source that samples.
    verdict: Arc<Mutex<Option<ReadError>>>,
    /// The slice of pending work does not end until it is interrupted.
    block_prepare: Arc<AtomicBool>,
    /// The game window is closing.
    exiting: Arc<AtomicBool>,
    /// A capture that has started does not go on until the test lets it.
    hold_capture: Arc<AtomicBool>,
    /// After the capture during which the character changes, the next ones block.
    block_after_change: Arc<AtomicBool>,
    /// The Magic Find the reader's diagnostics show for a capture, in whole points; 0 is none.
    magic_find: Arc<AtomicU32>,
    /// What the reader says about its last capture.
    diagnostics: Arc<Mutex<Diagnostics>>,
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
            pending: Arc::new(AtomicU32::new(0)),
            prepared: Arc::new(AtomicU32::new(0)),
            overdue_from: Arc::new(AtomicU32::new(u32::MAX)),
            verdict: Arc::new(Mutex::new(None)),
            block_prepare: Arc::new(AtomicBool::new(false)),
            exiting: Arc::new(AtomicBool::new(false)),
            hold_capture: Arc::new(AtomicBool::new(false)),
            block_after_change: Arc::new(AtomicBool::new(false)),
            magic_find: Arc::new(AtomicU32::new(0)),
            diagnostics: Arc::new(Mutex::new(Diagnostics::default())),
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
    fn prepare_inventory(&self, _stop: &AtomicBool, interrupted: &dyn Fn() -> bool) -> Readiness {
        let asked = self.prepared.fetch_add(1, Ordering::Relaxed) + 1;
        // Not ready, and whether that has gone on for too long.
        let unready = if asked >= self.overdue_from.load(Ordering::Relaxed) { Readiness::Overdue } else { Readiness::Pending };
        // A slice that would go on for as long as it is let: only being interrupted ends it.
        if self.block_prepare.load(Ordering::Relaxed) {
            while !interrupted() {
                std::thread::sleep(Duration::from_millis(5));
            }
            return unready;
        }
        // One slice of the work that is left, as the adapter does on each call.
        match self.pending.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |left| left.checked_sub(1)) {
            Ok(_) => unready,
            Err(_) => Readiness::Ready,
        }
    }
    fn read_inventory(&self, stop: &AtomicBool) -> Result<InventorySnapshot, ReadError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.capture_started.store(true, Ordering::Relaxed);
        while self.hold_capture.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(1));
        }
        // What the adapter answers to a sample asked for while its verdict is pending: it does
        // its slice and has no sample. That answer is a failed reading to whoever takes it for
        // one, which is why the loop asks `prepare_inventory` first.
        if self.pending.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |left| left.checked_sub(1)).is_ok() {
            return Err(ReadError::ReadFailed);
        }
        if let Some(error) = *self.verdict.lock().unwrap() {
            return Err(error);
        }
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
            if self.block_after_change.load(Ordering::Relaxed) {
                self.block_capture.store(true, Ordering::Relaxed);
            }
        }
        // The reader's own account of this cycle, as the adapter leaves it for the diagnostics.
        let points = self.magic_find.load(Ordering::Relaxed);
        if points != 0 {
            self.diagnostics.lock().unwrap().magic_find =
                MagicFindCoverage::Read(MagicFind { total: points as f32, luck: points, pushed: 0.0, buffs: 0.0, boon: false });
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
    fn inventory_diagnostics(&self) -> Diagnostics {
        *self.diagnostics.lock().unwrap()
    }
    fn game_exiting(&self) -> bool {
        self.exiting.load(Ordering::Relaxed)
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
    /// Every line on the wire up to and including the first that is not a heartbeat.
    fn through_first_other(&mut self) -> Vec<Value> {
        let mut wire = Vec::new();
        loop {
            wire.push(self.sequenced());
            if wire.last().is_some_and(|line| line["type"] != "heartbeat") {
                return wire;
            }
        }
    }
    /// The same for a line that is owed: it fails after `most` heartbeats without it, where
    /// the other would go on reading them for as long as the worker lives.
    fn through_first_other_within(&mut self, most: usize) -> Vec<Value> {
        let mut wire = Vec::new();
        loop {
            wire.push(self.sequenced());
            if wire.last().is_some_and(|line| line["type"] != "heartbeat") {
                return wire;
            }
            assert!(wire.len() <= most, "{most} heartbeats and nothing else: {wire:?}");
        }
    }
    /// The next `count` lines, which must all be heartbeats: nothing else is being said.
    fn only_heartbeats(&mut self, count: usize) {
        for _ in 0..count {
            let line = self.sequenced();
            assert_eq!(line["type"], "heartbeat", "{line}");
        }
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
/// Before its first sample the adapter verifies the game's executable, a slice of its hash on
/// each pass. That is not a reading that failed: while it is under way the loop takes no sample
/// and the plugin is told nothing, where a `live_status` would open a gap in the session that
/// reads "the reader could not complete the sample" on every load. Then the source opens as it
/// always did.
#[test]
fn a_pending_verdict_says_nothing_on_the_wire_and_then_the_source_opens_as_always() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (host, state, handle) = start(&listener);
    host.pending.store(3, Ordering::Relaxed);
    let mut p = Peer::new(listener.accept().unwrap().0);
    p.auth(3, true);
    assert_eq!(p.next()["type"], "context");
    let wire = p.through_first_other();
    let (open, before) = wire.split_last().unwrap();
    assert!(before.iter().all(|line| line["type"] == "heartbeat"), "{wire:?}");
    assert_eq!(open["type"], "live_open", "{wire:?}");
    // Asked three times and told "not yet", and not once asked for a sample meanwhile.
    assert_eq!(host.pending.load(Ordering::Relaxed), 0);
    assert_eq!(host.prepared.load(Ordering::Relaxed), 4);
    assert_eq!(host.calls.load(Ordering::Relaxed), 1);
    assert_eq!(state.live_epochs_opened(), 1);
    // The baseline of that epoch: cursor 0, as on any other load.
    let epoch = open["epoch"].as_str().unwrap().to_string();
    p.ready(&epoch);
    p.sample(&epoch, 0, 0);
    p.ack(&epoch, 0, "stored");
    handle.stop();
}

/// The verification ends in "another build": that is said once, as it always was, and only
/// when it is known.
#[test]
fn a_pending_verdict_that_ends_in_another_build_says_unsupported_build_once() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (host, state, handle) = start(&listener);
    host.pending.store(3, Ordering::Relaxed);
    *host.verdict.lock().unwrap() = Some(ReadError::UnsupportedBuild);
    let mut p = Peer::new(listener.accept().unwrap().0);
    p.auth(3, true);
    assert_eq!(p.next()["type"], "context");
    let wire = p.through_first_other();
    let (status, before) = wire.split_last().unwrap();
    assert!(before.iter().all(|line| line["type"] == "heartbeat"), "{wire:?}");
    assert_eq!(
        (status["type"].as_str(), status["status"].as_str(), status["reason"].as_str(), &status["epoch"]),
        (Some("live_status"), Some("unavailable"), Some("unsupported_build"), &Value::Null),
        "{wire:?}"
    );
    assert_eq!((host.pending.load(Ordering::Relaxed), host.calls.load(Ordering::Relaxed)), (0, 1));
    // Once: the source is stopped, and is not asked again.
    p.only_heartbeats(6);
    assert_eq!(host.calls.load(Ordering::Relaxed), 1);
    assert_eq!(state.live_epochs_opened(), 0);
    handle.stop();
}

/// The verification ends in a failure of the system, the file that cannot be read: that one is
/// a reading that failed, and it is said when it happens, not while the hash was under way.
#[test]
fn a_pending_verdict_that_ends_in_a_system_failure_says_read_failed() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (host, state, handle) = start(&listener);
    host.pending.store(3, Ordering::Relaxed);
    *host.verdict.lock().unwrap() = Some(ReadError::ReadFailed);
    let mut p = Peer::new(listener.accept().unwrap().0);
    p.auth(3, true);
    assert_eq!(p.next()["type"], "context");
    let wire = p.through_first_other();
    let (status, before) = wire.split_last().unwrap();
    assert!(before.iter().all(|line| line["type"] == "heartbeat"), "{wire:?}");
    assert_eq!(
        (status["type"].as_str(), status["status"].as_str(), status["reason"].as_str()),
        (Some("live_status"), Some("unavailable"), Some("read_failed")),
        "{wire:?}"
    );
    // It came from the one sample asked for after the verification was over.
    assert_eq!((host.pending.load(Ordering::Relaxed), host.prepared.load(Ordering::Relaxed), host.calls.load(Ordering::Relaxed)), (0, 4, 1));
    // Said once; the source goes on trying, a second apart, and has nothing new to say.
    p.only_heartbeats(6);
    assert_eq!(state.live_epochs_opened(), 0);
    handle.stop();
}

/// The hash has no limit of its own, and while it was under way the loop said nothing: on a
/// cold, slow disk the plugin saw the capability and then heartbeats for as long as that took,
/// and the panel waited without a word of why. Past what the source keeps quiet about, the
/// plugin is told that it could not complete a reading, once, and no sample is asked for. The
/// hash is not given up: the source is asked for its next slice on every pass as before, and
/// when it is over it opens as on any other load.
#[test]
fn a_verdict_pending_for_too_long_says_read_failed_once_and_the_source_opens_when_it_is_over() {
    use tyrian_companion_nexus_core::live::LiveStatus;
    use tyrian_companion_nexus_core::panel;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (host, state, handle) = start(&listener);
    // A hash that goes on for as long as the test lets it, overdue from its fourth slice.
    host.pending.store(u32::MAX, Ordering::Relaxed);
    host.overdue_from.store(4, Ordering::Relaxed);
    let mut p = Peer::new(listener.accept().unwrap().0);
    p.auth(3, true);
    assert_eq!(p.next()["type"], "context");
    // Three slices of silence, and on the fourth the plugin is told.
    let wire = p.through_first_other_within(12);
    let (status, before) = wire.split_last().unwrap();
    assert!(before.iter().all(|line| line["type"] == "heartbeat"), "{wire:?}");
    assert_eq!(
        (status["type"].as_str(), status["status"].as_str(), status["reason"].as_str(), &status["epoch"]),
        (Some("live_status"), Some("unavailable"), Some("read_failed"), &Value::Null),
        "{wire:?}"
    );
    // The panel and Options say it too, where they said the source was waiting.
    assert_eq!(state.live_status(), LiveStatus::Unavailable);
    assert_eq!(panel::inventory_status(state.live_status(), true), "Inventory: reading unavailable");
    // Said once. Six more passes: each asked the source for its next slice, so the hash went on
    // at the pace it had, and none asked for a sample, there being no verdict to read under.
    p.only_heartbeats(6);
    assert!(host.prepared.load(Ordering::Relaxed) >= 9, "asked {} times", host.prepared.load(Ordering::Relaxed));
    assert_eq!(host.calls.load(Ordering::Relaxed), 0);
    assert_eq!(state.live_epochs_opened(), 0);
    assert_eq!(state.inventory_reading().1, None, "no pass of the reader to show");
    // Two slices left, and the hash is over: the source opens, with nothing said in between.
    host.pending.store(2, Ordering::Relaxed);
    let wire = p.through_first_other_within(12);
    let (open, before) = wire.split_last().unwrap();
    assert!(before.iter().all(|line| line["type"] == "heartbeat"), "{wire:?}");
    assert_eq!(open["type"], "live_open", "{wire:?}");
    assert_eq!((host.pending.load(Ordering::Relaxed), host.calls.load(Ordering::Relaxed)), (0, 1));
    assert_eq!(state.live_epochs_opened(), 1);
    // The baseline of that epoch, and the samples after it, as on any other load.
    let epoch = open["epoch"].as_str().unwrap().to_string();
    p.ready(&epoch);
    p.sample(&epoch, 0, 0);
    host.quantity.store(2, Ordering::Relaxed);
    p.ack(&epoch, 0, "stored");
    p.sample(&epoch, 1, 2);
    p.ack(&epoch, 1, "stored");
    handle.stop();
}

/// The slice of a hash that is overdue, cut short because the game is closing: nothing is said
/// about the source on the way out, as when it was only pending.
#[test]
fn an_overdue_slice_cut_short_by_the_game_closing_says_nothing_before_the_bye() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (host, _state, handle) = start(&listener);
    host.block_prepare.store(true, Ordering::Relaxed);
    host.overdue_from.store(1, Ordering::Relaxed);
    let mut p = Peer::new(listener.accept().unwrap().0);
    p.auth(3, true);
    assert_eq!(p.next()["type"], "context");
    let waiting = std::time::Instant::now();
    while host.prepared.load(Ordering::Relaxed) == 0 {
        assert!(waiting.elapsed() < Duration::from_secs(5), "the source was never asked to prepare");
        std::thread::sleep(Duration::from_millis(5));
    }
    host.exiting.store(true, Ordering::Relaxed);
    let bye = p.next();
    assert_eq!((bye["type"].as_str(), bye["reason"].as_str()), (Some("bye"), Some("game_exit")), "{bye}");
    assert_eq!(host.calls.load(Ordering::Relaxed), 0);
    handle.stop();
}

/// A slice of the pending work lasts a second in the adapter. Told to stop, the worker cut it
/// at once; with the game closing it did not, and the `bye` waited for the slice to run out.
/// The slice here never runs out by itself: only being interrupted ends it.
#[test]
fn a_slice_of_pending_work_is_cut_short_when_the_game_is_closing() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (host, _state, handle) = start(&listener);
    host.block_prepare.store(true, Ordering::Relaxed);
    let mut p = Peer::new(listener.accept().unwrap().0);
    p.auth(3, true);
    assert_eq!(p.next()["type"], "context");
    // The worker is inside the slice.
    let waiting = std::time::Instant::now();
    while host.prepared.load(Ordering::Relaxed) == 0 {
        assert!(waiting.elapsed() < Duration::from_secs(5), "the source was never asked to prepare");
        std::thread::sleep(Duration::from_millis(5));
    }
    let closing = std::time::Instant::now();
    host.exiting.store(true, Ordering::Relaxed);
    let bye = p.next();
    assert_eq!((bye["type"].as_str(), bye["reason"].as_str()), (Some("bye"), Some("game_exit")), "{bye}");
    assert!(closing.elapsed() < Duration::from_secs(1), "the bye took {:?}", closing.elapsed());
    // Nothing was sampled and nothing said about the source on the way out.
    assert_eq!(host.calls.load(Ordering::Relaxed), 0);
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
    let (host, state, handle) = start(&listener);
    assert_eq!(state.live_epochs_opened(), 0);
    let mut p = Peer::new(listener.accept().unwrap().0);
    p.auth(3, true);
    p.next();
    let first = p.open();
    // Counted for the Options window before the `live_open` is written, and only there.
    assert_eq!(state.live_epochs_opened(), 1);
    p.ready(&first);
    p.sample(&first, 0, 0);
    p.ack(&first, 0, "stored");
    assert_eq!(state.live_epochs_opened(), 1, "a sample of the same epoch opens none");
    drop(p);
    host.quantity.store(9, Ordering::Relaxed);
    let mut p = Peer::new(listener.accept().unwrap().0);
    p.auth_nonce(3, true, "AwMDAwMDAwMDAwMDAwMDAw");
    assert_eq!(p.next()["type"], "context");
    let second = p.open();
    assert_ne!(first, second);
    assert_eq!(state.live_epochs_opened(), 2, "added up over the connections of one load");
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

/// The reader's own account of a cycle (bags, Magic Find) was put where the panel reads it
/// before the loop looked at the context again. A cycle during which the character changed was
/// discarded for the plugin and still left its Magic Find there, dated with that cycle: until
/// the next pass the panel painted it as a reading of the character it still had, and took it
/// for that character's highest of the session, whichever of the two the bytes were.
#[test]
fn a_cycle_whose_context_changed_leaves_none_of_the_readers_output_for_the_panel() {
    use tyrian_companion_nexus_core::panel::{self, PanelMemory, PanelSources};
    use tyrian_companion_nexus_core::protocol::{parse_server_line, ServerLine};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (host, state, handle) = start(&listener);
    host.hold_capture.store(true, Ordering::Relaxed);
    host.change_on_capture.store(true, Ordering::Relaxed);
    host.block_after_change.store(true, Ordering::Relaxed);
    host.magic_find.store(400, Ordering::Relaxed);
    let mut p = Peer::new(listener.accept().unwrap().0);
    p.auth(3, true);
    assert_eq!(p.next()["type"], "context");
    // A session that is measuring, as the plugin reports one: only then is there a highest.
    let frame = json!({ "v":3, "type":"farming_state", "tag":"farm1", "nonce":NONCE, "seq":1, "ttl":15,
        "phase":"active", "err":null, "elapsed":1716, "observed":143, "net":null,
        "lo":37, "hi":37, "age":0, "slots":null, "slotSrc":"unknown", "slotAge":null,
        "goal":"none", "target":null, "progress":null, "eta":null, "mf":null,
        "mfKind":"unknown", "prep":"unknown" });
    let ServerLine::FarmingState(reading) = parse_server_line(&frame.to_string()) else { panic!("{frame}") };
    assert!(state.enable_farming(NONCE) && state.accept_farming(reading, std::time::Instant::now()));
    let waiting = std::time::Instant::now();
    let wait_for = |what: &str, done: &dyn Fn() -> bool| {
        while !done() {
            assert!(waiting.elapsed() < Duration::from_secs(5), "{what}");
            std::thread::sleep(Duration::from_millis(1));
        }
    };
    // The first cycle is under way, on the first character.
    wait_for("the first capture never started", &|| host.capture_started.load(Ordering::Relaxed));
    let mut memory = PanelMemory::new();
    // A frame of the panel, closed or open: what it remembers is the same.
    let mut paint = || {
        let sources = PanelSources::read(&state, std::time::Instant::now());
        panel::observe(&sources.input(), &mut memory);
        memory.magic_find_peak()
    };
    assert_eq!(paint(), None, "nothing read yet");
    // The cycle goes on and the character changes under it. Every frame from here until the
    // next cycle, of the other character, has started: none of them sees a reading.
    host.hold_capture.store(false, Ordering::Relaxed);
    while host.calls.load(Ordering::Relaxed) < 2 {
        assert!(waiting.elapsed() < Duration::from_secs(5), "the second capture never started");
        assert_eq!(paint(), None, "the Magic Find of a discarded cycle became a highest");
        assert!(state.inventory_reading().1.is_none(), "a discarded cycle was dated as a reading");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(paint(), None);
    let (diagnostics, read_at) = state.inventory_reading();
    assert!(read_at.is_none() && diagnostics.magic_find == MagicFindCoverage::NotRead, "{diagnostics:?} {read_at:?}");
    // On the wire it is the change of context it always was, and no sample.
    let changed = p.next();
    assert_eq!((changed["type"].as_str(), changed["character"].as_str()), (Some("context"), Some("Changed Character")), "{changed}");
    assert_eq!(state.live_epochs_opened(), 0);
    handle.stop();
}

/// The other side of it: a cycle whose context held is what the panel reads, dated with that
/// cycle, whether its sample was good or the capture failed.
#[test]
fn a_cycle_whose_context_held_puts_the_readers_output_where_the_panel_reads_it() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (host, state, handle) = start(&listener);
    host.magic_find.store(333, Ordering::Relaxed);
    let mut p = Peer::new(listener.accept().unwrap().0);
    p.auth(3, true);
    assert_eq!(p.next()["type"], "context");
    let epoch = p.open();
    let (diagnostics, first) = state.inventory_reading();
    assert!(matches!(diagnostics.magic_find, MagicFindCoverage::Read(read) if read.total == 333.0), "{diagnostics:?}");
    assert!(first.is_some());
    // The next capture fails: no sample, and still a pass of the reader, with its own date.
    *host.verdict.lock().unwrap() = Some(ReadError::ReadFailed);
    p.ready(&epoch);
    p.sample(&epoch, 0, 0);
    p.ack(&epoch, 0, "stored");
    let status = p.next();
    assert_eq!((status["type"].as_str(), status["reason"].as_str()), (Some("live_status"), Some("read_failed")), "{status}");
    assert!(state.inventory_reading().1 > first);
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

//! The background client: connects to the plugin's loopback TCP server, sends the one-line
//! `hello`, reads alert lines forever, and reconnects on `[250, 500, 1000, 2000, 5000]` ms
//! saturated backoff when there is no server yet or the connection drops.
//!
//! Runs on its own OS thread, started from `load()` and stopped from `unload()`. `nexus`'s
//! `render!` macro needs a plain `fn(&Ui)`, not a capturing closure (see `state.rs`), so this
//! module reaches for the same `state::shared()` static the options panel reads, plus its own
//! stop flag threaded in at spawn time.
//!
//! What this thread never does, by construction: after `hello` it only calls `stream.read`,
//! never `stream.write` again (see `serve`), and it never calls anything from the GW2 API,
//! Mumble Link, or NexusLink — the only Nexus API surface it touches is `nexus::alert::send_alert`.

use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use tyrian_companion_nexus_core::backoff::Backoff;
use tyrian_companion_nexus_core::framer::{FramedLine, LineFramer};
use tyrian_companion_nexus_core::protocol::{self, LineOutcome, UPDATE_ADDON_MESSAGE};
use tyrian_companion_nexus_core::state;

/// How long a single connection attempt may take before it counts as a failure.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);

/// The socket read timeout while a connection is up. Short enough that `unload()`'s stop
/// signal is noticed quickly, long enough not to busy-loop.
const READ_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Granularity for waiting out a backoff delay while still noticing a stop request promptly.
const STOP_CHECK_INTERVAL: Duration = Duration::from_millis(100);

pub struct ClientHandle {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl ClientHandle {
    /// Signals the background thread to stop and blocks until it does. Bounded by
    /// `CONNECT_TIMEOUT` / `READ_POLL_INTERVAL`: the thread never blocks longer than those
    /// without checking the stop flag, so this returns quickly and does not leave a thread
    /// running against Nexus APIs the DLL is about to have unloaded.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Starts the background thread. `client_version` is baked into the `hello` line once at
/// spawn time (it is this crate's own version, not something that changes at runtime).
pub fn spawn(client_version: String) -> ClientHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let join = std::thread::Builder::new()
        .name("tyrian-companion-nexus-client".into())
        .spawn(move || run(&thread_stop, &client_version))
        .expect("failed to spawn the in-game alert client thread");
    ClientHandle { stop, join: Some(join) }
}

fn run(stop: &AtomicBool, client_version: &str) {
    let mut backoff = Backoff::new();
    while !stop.load(Ordering::Relaxed) {
        let port = state::shared().port();
        match connect(port) {
            Ok(stream) => {
                backoff.record_success();
                state::shared().set_connected(true);
                // The plugin restarts its own `seq` counter at 1 on every relaunch, so the
                // dedup window has to start over here too, or a plugin restart would leave
                // every alert below the old high-water mark discarded in silence.
                state::shared().reset_seq();
                log::info!("connected to the Tyrian Companion plugin on 127.0.0.1:{port}");
                serve(stream, client_version, stop);
                state::shared().set_connected(false);
                log::info!("disconnected from the Tyrian Companion plugin");
            }
            Err(_) => {
                backoff.record_failure();
            }
        }
        if stop.load(Ordering::Relaxed) {
            break;
        }
        wait(backoff.delay(), stop);
    }
}

fn connect(port: u16) -> std::io::Result<TcpStream> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
}

/// Sends the one `hello` line, then only ever reads. Returns once the connection ends, for
/// any reason: a clean close, an error, an unparsable `hello` (a local bug, not a wire
/// problem), or a stop request. The caller decides what happens next; this function never
/// retries anything itself.
fn serve(mut stream: TcpStream, client_version: &str, stop: &AtomicBool) {
    let Some(hello) = protocol::build_hello_line(client_version) else {
        log::error!("the hello line does not fit the wire's 128-byte cap; not connecting");
        return;
    };
    if stream.write_all(hello.as_bytes()).is_err() {
        return;
    }
    // The one-direction guarantee at this end: after `hello`, this socket is only ever read
    // from. There is no code path anywhere below that writes to `stream` again.
    if stream.set_read_timeout(Some(READ_POLL_INTERVAL)).is_err() {
        return;
    }

    let mut framer = LineFramer::new();
    let mut buffer = [0u8; 1024];
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        match stream.read(&mut buffer) {
            Ok(0) => return, // server closed the connection
            Ok(n) => {
                for line in framer.push(&buffer[..n]) {
                    handle_line(line);
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock || error.kind() == ErrorKind::TimedOut => {
                continue;
            }
            Err(_) => return,
        }
    }
}

fn handle_line(line: FramedLine) {
    let FramedLine::Complete(line) = line else {
        // `Oversized`: the spec's own 512-byte cap would have discarded this anyway once
        // framed; nothing to act on, and the framer has already resynced on its own.
        return;
    };
    match protocol::parse_alert_line(&line) {
        LineOutcome::Alert(alert) => {
            if !state::shared().accept_seq(alert.seq) {
                return;
            }
            // The one required delivery: paint `content` exactly as the plugin composed it.
            nexus::alert::send_alert(&alert.content);
            state::shared().push_history(&alert);
        }
        LineOutcome::UnsupportedVersion => {
            if state::shared().warn_about_version_once() {
                nexus::alert::send_alert(UPDATE_ADDON_MESSAGE);
            }
        }
        LineOutcome::Discard => {}
    }
}

/// Sleeps out `delay`, but in short increments so a stop request lands within
/// `STOP_CHECK_INTERVAL` instead of after the full backoff delay.
fn wait(delay: Duration, stop: &AtomicBool) {
    let mut remaining = delay;
    while remaining > Duration::ZERO {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let step = remaining.min(STOP_CHECK_INTERVAL);
        std::thread::sleep(step);
        remaining -= step;
    }
}

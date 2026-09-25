//! The client of the plugin's loopback bridge, protocol v2: connect, authenticate with the
//! token, report the game context, paint alerts, keep the connection alive, say goodbye, and
//! reconnect on the SPEC's backoff when the plugin is not there.
//!
//! It lives in this crate, not in the Windows-only `addon`, so the loop that actually runs in
//! the game is the same loop `cargo test` drives against a fake plugin on Linux
//! (`tests/client_v2.rs`). Everything that needs Nexus comes in through [`Host`]: painting an
//! alert, reading `NexusLink`/Mumble Link, and knowing the game window is closing.
//!
//! One connection, as the SPEC's "Ejemplo completo" lays it out:
//!
//! 1. `hello` with `client`, `clientVersion`, this process's `instance` and the token;
//! 2. the plugin's `welcome` (`server`, `nonce`, `heartbeatIntervalMs`), which resets the backoff;
//! 3. a `context` straight away, and again every time state, map or character changes;
//! 4. a `heartbeat` whenever nothing has been sent for `heartbeatIntervalMs`;
//! 5. `alert` lines in between, shown once per `(server, seq)`;
//! 6. a `bye` on the way out: `game_exit` only when the game window is closing, `addon_unload`
//!    otherwise.
//!
//! What the plugin's `error` means is in [`ErrorCode::retries`]: after `auth_rejected` or
//! `version_unsupported` this client stops trying until the user saves the settings again.
//! Every other end of a connection, the plugin missing included, is retried forever on
//! `[250, 500, 1000, 2000, 5000]` ms, well inside the plugin's 10-minute grace, so a dropped
//! connection comes back as the same presence rather than as a new session.
//!
//! The token goes into the `hello` and nowhere else: no log line in this module formats it.

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::backoff::Backoff;
use crate::framer::{FramedLine, LineFramer};
use crate::game_context::{ContextTracker, MumbleSnapshot};
use crate::obsidian_launch::{should_launch, FirstConnectOutcome, ObsidianLaunchOutcome};
use crate::protocol::{
    build_bye_line, build_context_line, build_heartbeat_line, build_hello_line, is_usable_token, parse_server_line,
    ByeReason, ErrorCode, GameContext, ServerLine, Welcome, TOKEN_MISSING_MESSAGE, TOKEN_REJECTED_MESSAGE,
    UPDATE_ADDON_MESSAGE,
};
use crate::state::{SharedState, Status};

/// How long a single connection attempt may take before it counts as a failure.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);

/// The socket read timeout while a connection is up. It is also how often the game context is
/// sampled and how late a stop request can be noticed.
const READ_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Granularity for waiting out a delay while still noticing a stop request promptly.
const STOP_CHECK_INTERVAL: Duration = Duration::from_millis(100);

/// How long to wait for a `welcome` before giving up on a connection. The plugin itself closes
/// a connection without a valid `hello` after 5 s; this only bounds a listener that accepts and
/// then never answers at all.
const WELCOME_TIMEOUT: Duration = Duration::from_secs(10);

/// Floor on the heartbeat interval, whatever the `welcome` says, so a strange value cannot turn
/// the heartbeat into a flood.
const MIN_HEARTBEAT_INTERVAL: Duration = Duration::from_millis(100);

/// One reading of what the host exposes about the game.
#[derive(Debug, Clone, Default)]
pub struct GameReading {
    /// `NexusLink::is_gameplay`, or `None` if the link could not be read.
    pub is_gameplay: Option<bool>,
    /// The Mumble Link as Nexus shares it, or `None` if it could not be read.
    pub mumble: Option<MumbleSnapshot>,
}

/// Everything the client needs from the host it runs in.
pub trait Host: Send + 'static {
    /// Paints one line for the player. In Nexus, `GUI_SendAlert`.
    fn show_alert(&self, text: &str);
    /// Reads the game's current state. Called about four times a second while connected.
    fn read_game(&self) -> GameReading;
    /// `true` once the game window has received `WM_CLOSE` or `WM_DESTROY`: the only evidence
    /// that allows a `bye` with `game_exit`.
    fn game_exiting(&self) -> bool;
    /// Tries to open or focus Obsidian. Called at most once per load, only when
    /// [`obsidian_launch::should_launch`](crate::obsidian_launch::should_launch) says so (see
    /// [`run`]'s own doc). Must not block on the child process it starts.
    fn open_obsidian(&self) -> ObsidianLaunchOutcome;
}

/// What does not change for the life of the process.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// This addon's version, sent as `clientVersion`.
    pub client_version: String,
    /// This process's `instance`, from [`crate::instance::new_instance_id`].
    pub instance: String,
}

/// The protocol state of one authenticated connection, with no socket in it.
#[derive(Debug)]
pub struct Session {
    server: String,
    nonce: String,
    next_seq: u64,
    heartbeat_interval: Duration,
    last_sent_at: Instant,
    last_context: Option<GameContext>,
}

impl Session {
    /// Starts the session a `welcome` opens. The sequence starts at 0, and the `hello` just sent
    /// counts as the last line for the heartbeat's clock.
    pub fn new(welcome: Welcome, now: Instant) -> Self {
        Self {
            server: welcome.server,
            nonce: welcome.nonce,
            next_seq: 0,
            heartbeat_interval: Duration::from_millis(welcome.heartbeat_interval_ms).max(MIN_HEARTBEAT_INTERVAL),
            last_sent_at: now,
            last_context: None,
        }
    }

    /// The plugin's `server` id for this connection, which alert deduplication is keyed on.
    pub fn server(&self) -> &str {
        &self.server
    }

    fn take_seq(&mut self, now: Instant) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.last_sent_at = now;
        seq
    }

    /// The next line to send, if any: a `context` if it differs from the last one sent (the first
    /// call after the `welcome` always sends one), otherwise a `heartbeat` if the connection has
    /// been silent for the interval. The sequence only advances for a line actually returned.
    pub fn next_outgoing(&mut self, now: Instant, context: &GameContext) -> Option<String> {
        if self.last_context.as_ref() != Some(context) {
            if let Some(line) = build_context_line(&self.nonce, self.next_seq, context) {
                self.take_seq(now);
                self.last_context = Some(context.clone());
                return Some(line);
            }
        }
        if now.saturating_duration_since(self.last_sent_at) >= self.heartbeat_interval {
            let line = build_heartbeat_line(&self.nonce, self.next_seq)?;
            self.take_seq(now);
            return Some(line);
        }
        None
    }

    /// The `bye` line. Nothing is sent after it.
    pub fn bye(&mut self, reason: ByeReason, now: Instant) -> Option<String> {
        let line = build_bye_line(&self.nonce, self.next_seq, reason)?;
        self.take_seq(now);
        Some(line)
    }
}

/// Handle on the background thread. Dropping it without calling [`ClientHandle::stop`] leaves
/// the thread running; the addon always stops it from `unload`.
pub struct ClientHandle {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl ClientHandle {
    /// Signals the thread to stop and blocks until it has: it sends its `bye`, closes the socket
    /// and exits within about `READ_POLL_INTERVAL`, so nothing is left running against Nexus APIs
    /// of a DLL that is about to be unmapped.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Starts [`run`] on its own named thread.
pub fn spawn<S, H>(state: S, host: H, config: ClientConfig) -> std::io::Result<ClientHandle>
where
    S: Deref<Target = SharedState> + Send + 'static,
    H: Host,
{
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let join = std::thread::Builder::new()
        .name("tyrian-companion-nexus-client".into())
        .spawn(move || run(&state, &host, &config, &thread_stop))?;
    Ok(ClientHandle { stop, join: Some(join) })
}

/// The client loop. Returns only once `stop` is set.
///
/// Also decides, at most once per call (i.e. once per addon load — `spawn` calls this once),
/// whether to open Obsidian: right after this loop's very first `connect()`, whatever the
/// setting or the result, `obsidian_launch::should_launch` sees that attempt's outcome and never
/// gets asked again for the rest of this call. See `obsidian_launch`'s own doc for why a TCP
/// connect failure — and only that — is treated as "Obsidian is closed".
pub fn run(state: &SharedState, host: &dyn Host, config: &ClientConfig, stop: &AtomicBool) {
    let mut backoff = Backoff::new();
    let mut tracker = ContextTracker::new();
    // Settings generation at which the plugin said "do not come back" (`auth_rejected`,
    // `version_unsupported`). Cleared as soon as the user saves the settings again.
    let mut halted_at: Option<u64> = None;
    // Whether the first-connect-of-this-load decision has already run, whatever it decided.
    let mut obsidian_launch_decided = false;

    while !stop.load(Ordering::Relaxed) {
        if host.game_exiting() {
            // The `bye game_exit` has gone out (or there was no connection to send it on).
            // Reconnecting now would only bring the presence back for the last milliseconds.
            state.set_status(Status::GameExiting);
            wait(STOP_CHECK_INTERVAL, stop);
            continue;
        }
        let generation = state.settings_generation();
        if halted_at == Some(generation) {
            wait(STOP_CHECK_INTERVAL, stop);
            continue;
        }
        if halted_at.take().is_some() {
            backoff.record_success();
        }

        let token = state.token();
        if !is_usable_token(&token) {
            state.set_status(Status::MissingToken);
            if state.warn_about_missing_token_once() {
                host.show_alert(TOKEN_MISSING_MESSAGE);
            }
            wait(STOP_CHECK_INTERVAL, stop);
            continue;
        }
        let Some(hello) = build_hello_line(&config.client_version, &config.instance, &token) else {
            // A local bug (a version string the plugin would refuse), not a wire problem.
            log::error!("the hello line breaks the protocol's rules; not connecting");
            halted_at = Some(generation);
            continue;
        };
        drop(token);

        state.set_status(Status::WaitingForPlugin);
        let port = state.port();
        let connect_result = connect(port);
        if !obsidian_launch_decided {
            obsidian_launch_decided = true;
            let first_connect =
                if connect_result.is_ok() { FirstConnectOutcome::Connected } else { FirstConnectOutcome::NoListener };
            if should_launch(state.open_obsidian_on_start(), false, first_connect) {
                state.set_obsidian_launch_outcome(host.open_obsidian());
            }
        }
        let end = match connect_result {
            Ok(stream) => serve(stream, &hello, state, host, &mut tracker, stop),
            Err(_) => ConnectionEnd::default(),
        };
        if end.welcomed {
            backoff.record_success();
            log::info!("disconnected from the Tyrian Companion plugin");
        } else {
            backoff.record_failure();
        }
        if state.status() == Status::Connected {
            state.set_status(Status::WaitingForPlugin);
        }

        match end.error {
            Some(ErrorCode::AuthRejected) => {
                log::warn!("the plugin rejected the token (auth_rejected); waiting for a new one");
                state.set_status(Status::TokenRejected);
                host.show_alert(TOKEN_REJECTED_MESSAGE);
                halted_at = Some(generation);
                continue;
            }
            Some(ErrorCode::VersionUnsupported) => {
                log::warn!("the plugin speaks another protocol version (version_unsupported)");
                state.set_status(Status::UpdateRequired);
                if state.warn_about_version_once() {
                    host.show_alert(UPDATE_ADDON_MESSAGE);
                }
                halted_at = Some(generation);
                continue;
            }
            Some(ref code @ (ErrorCode::AddonFault(_) | ErrorCode::Unknown(_))) => {
                log::error!("the plugin closed the connection over an addon fault: {}", code.as_str());
            }
            Some(ref code) => log::info!("the plugin closed the connection: {}", code.as_str()),
            None => {}
        }
        if stop.load(Ordering::Relaxed) {
            break;
        }
        wait(backoff.delay(), stop);
    }
    if state.status() == Status::Connected {
        state.set_status(Status::WaitingForPlugin);
    }
}

fn connect(port: u16) -> std::io::Result<TcpStream> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
}

/// How one connection ended.
#[derive(Debug, Default)]
struct ConnectionEnd {
    /// The plugin sent a `welcome`: the connection was authenticated at some point.
    welcomed: bool,
    /// The `error` the plugin closed it with, if any.
    error: Option<ErrorCode>,
}

/// Runs one connection until it ends, for any reason: the plugin closes it (with or without an
/// `error`), the socket fails, no `welcome` arrives in time, or the client is told to stop, in
/// which case it sends the `bye` itself. Never retries anything; the caller decides.
fn serve(
    mut stream: TcpStream,
    hello: &str,
    state: &SharedState,
    host: &dyn Host,
    tracker: &mut ContextTracker,
    stop: &AtomicBool,
) -> ConnectionEnd {
    let mut end = ConnectionEnd::default();
    let _ = stream.set_nodelay(true);
    if stream.write_all(hello.as_bytes()).is_err() || stream.set_read_timeout(Some(READ_POLL_INTERVAL)).is_err() {
        return end;
    }
    let started = Instant::now();
    let mut framer = LineFramer::new();
    let mut session: Option<Session> = None;
    let mut buffer = [0u8; 1024];

    loop {
        let exiting = host.game_exiting();
        if stop.load(Ordering::Relaxed) || exiting {
            if let Some(session) = session.as_mut() {
                let reason = if exiting { ByeReason::GameExit } else { ByeReason::AddonUnload };
                if let Some(line) = session.bye(reason, Instant::now()) {
                    let _ = stream.write_all(line.as_bytes());
                }
            }
            let _ = stream.shutdown(Shutdown::Both);
            return end;
        }

        match stream.read(&mut buffer) {
            Ok(0) => return end,
            Ok(read) => {
                for line in framer.push(&buffer[..read]) {
                    if let Some(error) = handle_line(line, &mut session, state, host) {
                        end.error = Some(error);
                        return end;
                    }
                }
            }
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => return end,
        }

        match session.as_mut() {
            Some(session) => {
                end.welcomed = true;
                let reading = host.read_game();
                let now = Instant::now();
                let context = tracker.observe(now, reading.is_gameplay, reading.mumble.as_ref());
                if let Some(line) = session.next_outgoing(now, &context) {
                    if stream.write_all(line.as_bytes()).is_err() {
                        return end;
                    }
                }
            }
            None if started.elapsed() > WELCOME_TIMEOUT => {
                log::warn!("no welcome from the plugin within {} s; reconnecting", WELCOME_TIMEOUT.as_secs());
                return end;
            }
            None => {}
        }
    }
}

/// Acts on one framed line from the plugin. Returns the `error` code if the line was one: the
/// plugin closes the connection right after sending it.
fn handle_line(line: FramedLine, session: &mut Option<Session>, state: &SharedState, host: &dyn Host) -> Option<ErrorCode> {
    let FramedLine::Complete(line) = line else {
        // Over the framer's memory cap: the 512-byte wire cap would have discarded it anyway.
        return None;
    };
    match parse_server_line(&line) {
        ServerLine::Welcome(welcome) => {
            // A second `welcome` on one connection is not something the plugin sends; ignoring it
            // keeps the nonce and sequence the plugin is actually checking.
            if session.is_none() {
                *session = Some(Session::new(welcome, Instant::now()));
                state.set_status(Status::Connected);
                log::info!("connected to the Tyrian Companion plugin on 127.0.0.1:{}", state.port());
            }
        }
        ServerLine::Alert(alert) => {
            let Some(session) = session.as_ref() else { return None };
            if state.accept_alert(session.server(), alert.seq) {
                // The one required delivery: paint `content` exactly as the plugin composed it.
                host.show_alert(&alert.content);
                state.push_history(&alert);
            }
        }
        ServerLine::Error(code) => return Some(code),
        ServerLine::UnsupportedVersion => {
            if state.warn_about_version_once() {
                host.show_alert(UPDATE_ADDON_MESSAGE);
            }
        }
        ServerLine::Ignored | ServerLine::Discard => {}
    }
    None
}

/// Sleeps out `delay`, but in short increments so a stop request lands within
/// `STOP_CHECK_INTERVAL` instead of after the full delay.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::GameState;

    fn welcome(interval_ms: u64) -> Welcome {
        Welcome { server: "Pq0v4c3Wm9Xs1Ya7Tb2NeQ".into(), nonce: "Zk3m1Qw9Lr0aT7yUc2Vb5g".into(), heartbeat_interval_ms: interval_ms }
    }

    fn gameplay(map_id: u32) -> GameContext {
        GameContext { state: GameState::Gameplay, map_id: Some(map_id), character: Some("Astra Uno".into()) }
    }

    #[test]
    fn the_first_line_after_welcome_is_a_context_at_seq_zero() {
        let start = Instant::now();
        let mut session = Session::new(welcome(5000), start);
        let line = session.next_outgoing(start, &GameContext::character_select()).unwrap();
        assert!(line.contains(r#""type":"context""#) && line.contains(r#""seq":0"#), "{line}");
        assert_eq!(session.next_outgoing(start, &GameContext::character_select()), None, "unchanged: nothing to send");
    }

    #[test]
    fn a_change_sends_a_context_and_silence_sends_a_heartbeat_on_one_sequence() {
        let start = Instant::now();
        let mut session = Session::new(welcome(5000), start);
        session.next_outgoing(start, &GameContext::character_select()).unwrap();
        let second = session.next_outgoing(start + Duration::from_secs(1), &gameplay(50)).unwrap();
        assert!(second.contains(r#""seq":1"#) && second.contains(r#""mapId":50"#), "{second}");
        assert_eq!(session.next_outgoing(start + Duration::from_millis(5999), &gameplay(50)), None);
        let heartbeat = session.next_outgoing(start + Duration::from_secs(6), &gameplay(50)).unwrap();
        assert_eq!(heartbeat, "{\"v\":2,\"type\":\"heartbeat\",\"nonce\":\"Zk3m1Qw9Lr0aT7yUc2Vb5g\",\"seq\":2}\n");
        let bye = session.bye(ByeReason::AddonUnload, start + Duration::from_secs(7)).unwrap();
        assert!(bye.contains(r#""seq":3"#) && bye.contains("addon_unload"), "{bye}");
    }

    #[test]
    fn a_context_counts_as_a_sign_of_life() {
        let start = Instant::now();
        let mut session = Session::new(welcome(5000), start);
        session.next_outgoing(start, &gameplay(50)).unwrap();
        session.next_outgoing(start + Duration::from_secs(4), &gameplay(866)).unwrap();
        assert_eq!(session.next_outgoing(start + Duration::from_secs(8), &gameplay(866)), None, "4 s since the last context");
    }

    #[test]
    fn the_heartbeat_interval_has_a_floor() {
        let start = Instant::now();
        let mut session = Session::new(welcome(1), start);
        session.next_outgoing(start, &gameplay(50)).unwrap();
        assert_eq!(session.next_outgoing(start + Duration::from_millis(50), &gameplay(50)), None);
        assert!(session.next_outgoing(start + MIN_HEARTBEAT_INTERVAL, &gameplay(50)).is_some());
    }
}

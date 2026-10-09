//! The client of the plugin's loopback bridge, protocol v3: connect, authenticate with the
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
//! 2. the plugin's `welcome` (`server`, `nonce`, `heartbeatIntervalMs`); a connection that then
//!    lives for 10 s (`STABLE_CONNECTION`) starts the backoff over when it ends;
//! 3. a `context` straight away, and again every time state, map or character changes;
//! 4. a `heartbeat` whenever nothing has been sent for `heartbeatIntervalMs`;
//! 5. `alert` lines in between, shown once per `(server, seq)`, each confirmed with an `alert_ack`
//!    right after it is painted (one per `(server, alertSeq)`; a duplicate is not confirmed
//!    again, and neither is an alert without a `seq`);
//! 6. a `bye` on the way out: `game_exit` only when the game window is closing, `addon_unload`
//!    otherwise.
//!
//! What the plugin's `error` means is in [`ErrorCode::retries`]: after `auth_rejected` or
//! `version_unsupported` this client stops trying until the user saves the settings again.
//! Every other end of a connection, the plugin missing included, is retried forever on the
//! table `[250, 500, 1000, 2000, 5000]` ms, well inside the plugin's 10-minute grace, so a
//! dropped connection comes back as the same presence rather than as a new session. An attempt
//! that fails moves one step up the table before its wait is taken, so the waits after
//! failures are 500 ms, 1 s, 2 s and then 5 s; the 250 ms is only the wait after a connection
//! that had lived those 10 s. A connection the plugin welcomes and closes before them is one
//! more step up, like one that was never welcomed: the retries slow down instead of going on
//! at the fastest step.
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
use crate::obsidian_launch::{should_launch, FirstConnectOutcome, LaunchApp, ObsidianLaunchOutcome};
use crate::protocol::{
    build_alert_ack_line, build_bye_line, build_context_line, build_farming_sub_line, build_heartbeat_line, build_price_sub_line, build_hello_line, is_usable_token,
    parse_server_line, ByeReason, ErrorCode, GameContext, ServerLine, Welcome, TOKEN_MISSING_MESSAGE,
    TOKEN_REJECTED_MESSAGE, UPDATE_ADDON_MESSAGE, UPDATE_PLUGIN_MESSAGE,
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

/// How long a connection has to live after its `welcome` for its end to start the backoff table
/// over. A `welcome` alone is not enough: a plugin that welcomes and closes at once, for a line
/// of this addon it rejects or for a fault of its own, was retried every 250 ms for ever.
///
/// Ten seconds, for two reasons. The plugin asks for a heartbeat every 5 s and closes over a
/// line it rejects as soon as it reads it, so a connection that dies on its context, on its
/// first inventory frames or on its first heartbeat is gone within about five and a half: ten
/// is past all of those, and one that got there has had a heartbeat accepted. And it is not
/// shorter than the slowest step of the table (5 s), so starting over can never make the
/// reconnections more frequent than the table itself allows.
const STABLE_CONNECTION: Duration = Duration::from_secs(10);

/// One reading of what the host exposes about the game.
#[derive(Debug, Clone, Default)]
pub struct GameReading {
    /// `NexusLink::is_gameplay`, or `None` if the link could not be read.
    pub is_gameplay: Option<bool>,
    /// The Mumble Link as Nexus shares it, or `None` if it could not be read.
    pub mumble: Option<MumbleSnapshot>,
}

/// How the source stands when the worker asks whether it can take a sample
/// ([`Host::prepare_inventory`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    /// It can be asked for a sample now. That sample may still fail, and says so itself.
    Ready,
    /// Still getting ready, and nothing has failed: the loop takes no sample, opens no epoch
    /// and tells the plugin nothing, as on a pass in which it is not time to sample yet.
    Pending,
    /// Still getting ready, for longer than it keeps quiet about. The loop takes no sample
    /// either, and says that to the plugin as a reading that could not be completed
    /// (`live_status unavailable`, `read_failed`), once. The source goes on with what it was
    /// doing, a part on each pass, and is ready when that is over.
    Overdue,
}

/// Everything the client needs from the host it runs in.
pub trait Host: Send + 'static {
    /// Paints one line for the player. In Nexus, `GUI_SendAlert`.
    fn show_alert(&self, text: &str);
    /// Reads the game's current state. Called about four times a second while connected.
    fn read_game(&self) -> GameReading;
    /// Passive owned inventory, called only on this worker after v3 live1 negotiation.
    /// Existing/fake hosts keep explicit absence and never perform an API fallback.
    fn read_inventory(&self, _stop: &AtomicBool) -> Result<crate::inventory::InventorySnapshot, crate::inventory::ReadError> {
        Err(crate::inventory::ReadError::RootUnavailable)
    }
    fn inventory_diagnostics(&self) -> crate::inventory::Diagnostics { crate::inventory::Diagnostics::default() }
    /// The loop threw away the cycle [`Host::read_inventory`] just ran, because the game
    /// context changed while it was copying. Whatever the source keeps from one cycle to the
    /// next (the Magic Find content) belongs to a cycle that is gone and
    /// is forgotten here, so the next cycle reads and finds everything again. A host that
    /// keeps nothing has nothing to do.
    fn discard_cycle(&self) {}
    /// Does a bounded part of whatever the source has to do before it can take a sample at all,
    /// and says how that stands ([`Readiness`]). Asked on this worker right before each sample.
    /// The loop calls [`Host::read_inventory`] only on [`Readiness::Ready`]: while the source is
    /// not ready there is no sample to take on this pass, and it is asked again on the next.
    /// The Windows adapter verifies the game's executable here, a slice of its hash at a time.
    /// A host with nothing to prepare is always ready.
    ///
    /// `interrupted` says when that part of the work has to be cut short at once: the worker
    /// was told to stop, or the game is closing and its `bye` is owed. The loop decides that,
    /// not the host; `stop` is the worker's own flag, as [`Host::read_inventory`] gets it.
    fn prepare_inventory(&self, _stop: &AtomicBool, _interrupted: &dyn Fn() -> bool) -> Readiness { Readiness::Ready }
    /// `true` once the game window has received `WM_CLOSE` or `WM_DESTROY`: the only evidence
    /// that allows a `bye` with `game_exit`.
    fn game_exiting(&self) -> bool;
    /// Tries to open or focus `app` (the player's choice, Obsidian or Hebra). Called at most
    /// once per load, only when
    /// [`obsidian_launch::should_launch`](crate::obsidian_launch::should_launch) says so (see
    /// [`run`]'s own doc). Must not block on the child process it starts.
    fn open_app(&self, app: LaunchApp) -> ObsidianLaunchOutcome;
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
    last_context_seq: Option<u64>,
    live_allowed: bool,
    live: crate::live::Channel,
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
            last_context_seq: None,
            live_allowed: true,
            live: crate::live::Channel::new(),
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
        if self.next_seq > crate::live::MAX_SAFE { return None; }
        self.live.context_changed(context);
        if self.last_context.as_ref() != Some(context) {
            if let Some(line) = build_context_line(&self.nonce, self.next_seq, context) {
                self.last_context_seq = Some(self.take_seq(now));
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

    /// The `alert_ack` line for the alert numbered `alert_seq`, on the session's own sequence. Call
    /// it only for an alert that was just painted and only once per `(server, alertSeq)`; the
    /// heartbeat clock restarts, as with any line sent.
    pub fn ack_alert(&mut self, alert_seq: u64, now: Instant) -> Option<String> {
        if self.next_seq > crate::live::MAX_SAFE { return None; }
        let line = build_alert_ack_line(&self.nonce, self.next_seq, alert_seq)?;
        self.take_seq(now);
        Some(line)
    }

    /// Adds a negotiated farming subscription to the same outgoing sequence and heartbeat
    /// clock as context and alert acknowledgements.
    pub fn subscribe_farming(&mut self, now: Instant) -> Option<String> {
        if self.next_seq > crate::live::MAX_SAFE { return None; }
        let line = build_farming_sub_line(&self.nonce, self.next_seq)?;
        self.take_seq(now);
        Some(line)
    }

    /// Same for the `price2` subscription, sent once and only after its `price_cap`.
    pub fn subscribe_price(&mut self, now: Instant) -> Option<String> {
        if self.next_seq > crate::live::MAX_SAFE { return None; }
        let line = build_price_sub_line(&self.nonce, self.next_seq)?;
        self.take_seq(now);
        Some(line)
    }

    /// Serialize an entire bounded batch before advancing the shared TCP sequence.
    /// A framing/numbering failure sends none of the batch.
    fn live_lines(&mut self, frames: Vec<crate::live::Frame>, now: Instant) -> Option<Vec<String>> {
        if frames.len() > 82 { return None; }
        let mut lines = Vec::with_capacity(frames.len());
        let mut bytes = 0;
        for (offset, frame) in frames.into_iter().enumerate() {
            let seq = self.next_seq.checked_add(offset as u64)?;
            let line = crate::live::frame_line(frame, &self.nonce, seq)?;
            bytes += line.len();
            if bytes > 256 * 1024 { return None; }
            lines.push(line);
        }
        for _ in &lines { self.take_seq(now); }
        Some(lines)
    }

    /// The `bye` line. Nothing is sent after it.
    pub fn bye(&mut self, reason: ByeReason, now: Instant) -> Option<String> {
        if self.next_seq > crate::live::MAX_SAFE { return None; }
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
    /// and exits after the current bounded read/poll notices cancellation. Native reads/hash
    /// chunks check this flag too; an OS call already in progress cannot be preempted here.
    /// Nothing is left running against Nexus APIs of a DLL that is about to be unmapped.
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
                state.set_obsidian_launch_outcome(host.open_app(state.launch_app()));
            }
        }
        let end = match connect_result {
            Ok(stream) => serve(stream, &hello, state, host, &mut tracker, stop),
            Err(_) => ConnectionEnd::default(),
        };
        // How long it lived after its `welcome`, if it got one.
        let lasted = end.welcomed_at.map(|welcomed_at| welcomed_at.elapsed());
        state.disconnect_farming();
        state.disconnect_price();
        if !matches!(state.live_status(), crate::live::LiveStatus::NotNegotiated | crate::live::LiveStatus::StorageUnavailable | crate::live::LiveStatus::Conflict) {
            state.set_live_status(crate::live::LiveStatus::Unavailable);
        }
        if lasted.is_some() {
            log::info!("disconnected from the Tyrian Companion plugin");
        }
        record_connection(&mut backoff, lasted);
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
            Some(ref code @ (ErrorCode::VersionUnsupported | ErrorCode::PluginTooOld)) => {
                let plugin_is_old = *code == ErrorCode::PluginTooOld;
                log::warn!(
                    "the plugin speaks another protocol version (version_unsupported, {})",
                    if plugin_is_old { "plugin too old" } else { "addon too old" }
                );
                state.set_status(Status::UpdateRequired);
                if state.warn_about_version_once() {
                    host.show_alert(if plugin_is_old { UPDATE_PLUGIN_MESSAGE } else { UPDATE_ADDON_MESSAGE });
                }
                halted_at = Some(generation);
                continue;
            }
            Some(ErrorCode::Unknown(ref code)) if code == "live_storage_unavailable" => {
                // Persistence failure is not a successful measurement. Keep a five-second floor
                // between rebaseline attempts rather than reset to the fastest reconnect loop.
                wait(Duration::from_secs(5), stop);
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
    state.disconnect_farming();
    state.disconnect_price();
}

fn connect(port: u16) -> std::io::Result<TcpStream> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
}

/// Moves the backoff on after a connection ended. `lasted` is how long it lived after its
/// `welcome`, `None` when it never got one or never connected. Only a connection that lived
/// for [`STABLE_CONNECTION`] starts the table over; every other end is one more step up it.
fn record_connection(backoff: &mut Backoff, lasted: Option<Duration>) {
    if lasted.is_some_and(|lasted| lasted >= STABLE_CONNECTION) {
        backoff.record_success();
    } else {
        backoff.record_failure();
    }
}

/// How one connection ended.
#[derive(Debug, Default)]
struct ConnectionEnd {
    /// When the client first acted on the plugin's `welcome`: the connection was authenticated
    /// from then on. `None` if no `welcome` came.
    welcomed_at: Option<Instant>,
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
    if stream.set_write_timeout(Some(READ_POLL_INTERVAL)).is_err() || stream.write_all(hello.as_bytes()).is_err() || stream.set_read_timeout(Some(READ_POLL_INTERVAL)).is_err() {
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
                let mut outgoing = Vec::new();
                for line in framer.push(&buffer[..read]) {
                    if let Some(error) = handle_line(line, &mut session, state, host, &mut outgoing) {
                        end.error = Some(error);
                        return end;
                    }
                }
                // The acks go out before the next context or heartbeat, in the order they took
                // their `seq`.
                for line in outgoing {
                    if stream.write_all(line.as_bytes()).is_err() {
                        return end;
                    }
                }
            }
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => return end,
        }

        match session.as_mut() {
            Some(session) => {
                end.welcomed_at.get_or_insert_with(Instant::now);
                if session.next_seq > crate::live::MAX_SAFE || session.live.timed_out(Instant::now()) {
                    state.set_live_status(crate::live::LiveStatus::Unavailable);
                    return end;
                }
                let reading = host.read_game();
                let now = Instant::now();
                let context = tracker.observe(now, reading.is_gameplay, reading.mumble.as_ref());
                if let Some(line) = session.next_outgoing(now, &context) {
                    if stream.write_all(line.as_bytes()).is_err() { return end; }
                }
                state.set_live_context(context.clone());
                let ctx = session.last_context_seq.unwrap_or(0);
                let mut frames = session.live.gameplay_status();
                if let Some(pending) = session.live.pending_frames(ctx, now) { frames.extend(pending); }
                else if session.live.wants_sample(now) && session.last_context.as_ref() == Some(&context) {
                    match host.prepare_inventory(stop, &|| stop.load(Ordering::Relaxed) || host.game_exiting()) {
                        // A source that is still getting ready has no sample and no failure to
                        // report: the pass goes by like one in which it is not time to sample,
                        // and it is asked again on the next. Nothing about it is put on the wire.
                        Readiness::Pending => {}
                        // Still getting ready, and for too long to go on saying nothing: no
                        // sample is asked for, and the plugin is told once that the source could
                        // not complete a reading. It is asked again on the next pass all the
                        // same, so what it is getting ready is not held up by having said so.
                        Readiness::Overdue => {
                            if stop.load(Ordering::Relaxed) || host.game_exiting() { continue; }
                            frames.extend(session.live.overdue());
                        }
                        Readiness::Ready => {
                            let sample = host.read_inventory(stop);
                            if stop.load(Ordering::Relaxed) || host.game_exiting() { continue; }
                            let diagnostics = host.inventory_diagnostics();
                            let captured_at = Instant::now();
                            let reading = host.read_game();
                            let after = tracker.observe(captured_at, reading.is_gameplay, reading.mumble.as_ref());
                            if after != context {
                                host.discard_cycle();
                                // The context changed while copying the inventory: discard it
                                // entirely, and with it what the reader says of that cycle. Its
                                // bags and its Magic Find are of a character that was changing,
                                // and the panel would take them, dated with this cycle, for a
                                // reading of the one it has, and for its highest of the session.
                                if let Some(line) = session.next_outgoing(captured_at, &after) {
                                    if stream.write_all(line.as_bytes()).is_err() { return end; }
                                }
                            } else {
                                // Only now: the context was the same before and after the cycle.
                                state.set_inventory_diagnostics(diagnostics, captured_at);
                                let opened = session.live.epochs_opened();
                                frames.extend(session.live.capture(sample, ctx, captured_at));
                                // For the Options window only: how often the source starts an epoch.
                                state.count_live_epochs(session.live.epochs_opened().saturating_sub(opened));
                            }
                        }
                    }
                }
                state.set_live_status(session.live.status);
                let Some(lines) = session.live_lines(frames, Instant::now()) else { return end; };
                let sending_started = Instant::now();
                for line in lines {
                    if sending_started.elapsed() >= crate::live::RESPONSE_TIMEOUT || stream.write_all(line.as_bytes()).is_err() { return end; }
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
/// plugin closes the connection right after sending it. Lines the addon owes the plugin in answer
/// (an `alert_ack` per painted alert) are pushed onto `outgoing`, already numbered.
fn handle_line(
    line: FramedLine,
    session: &mut Option<Session>,
    state: &SharedState,
    host: &dyn Host,
    outgoing: &mut Vec<String>,
) -> Option<ErrorCode> {
    let FramedLine::Complete(line) = line else {
        // Over the framer's memory cap: the 512-byte wire cap would have discarded it anyway.
        return None;
    };
    match parse_server_line(&line) {
        ServerLine::Welcome(welcome) => {
            // A second `welcome` on one connection is not something the plugin sends; ignoring it
            // keeps the nonce and sequence the plugin is actually checking.
            if session.is_none() {
                state.begin_farming_connection(&welcome.nonce);
                state.begin_price_connection(&welcome.nonce);
                state.set_live_status(crate::live::LiveStatus::NotNegotiated);
                let mut new = Session::new(welcome, Instant::now());
                new.live.set_conflict_retry(state.live_conflict_retry());
                // Welcome's public DTO stays backward compatible; live is v3-only on the wire.
                new.live_allowed = serde_json::from_str::<serde_json::Value>(&line).ok().and_then(|r|r["v"].as_u64()) == Some(3);
                *session = Some(new);
                state.set_status(Status::Connected);
                log::info!("connected to the Tyrian Companion plugin on 127.0.0.1:{}", state.port());
            }
        }
        ServerLine::Alert(alert) => {
            let Some(session) = session.as_mut() else { return None };
            if state.accept_alert(session.server(), alert.seq) {
                // The one required delivery: paint `content` exactly as the plugin composed it.
                host.show_alert(&alert.content);
                state.push_history(&alert);
                // Confirm it now that it is painted. A duplicate never gets here, and an alert
                // without a `seq` has nothing to name in `alertSeq`.
                if let Some(line) = alert.seq.and_then(|seq| session.ack_alert(seq, Instant::now())) {
                    outgoing.push(line);
                }
            }
        }
        ServerLine::FarmingCapability(nonce) => {
            let Some(session) = session.as_mut() else { return None };
            if state.enable_farming(&nonce) {
                if let Some(line) = session.subscribe_farming(Instant::now()) {
                    outgoing.push(line);
                }
            }
        }
        ServerLine::FarmingState(reading) => {
            if session.is_some() { state.accept_farming(reading, Instant::now()); }
        }
        ServerLine::PriceCapability(nonce) => {
            let Some(session) = session.as_mut() else { return None };
            if state.enable_price(&nonce) {
                if let Some(line) = session.subscribe_price(Instant::now()) {
                    outgoing.push(line);
                }
            }
        }
        ServerLine::PriceState(reading) => {
            if session.is_some() { state.accept_price(reading, Instant::now()); }
        }
        ServerLine::Live(reply) => {
            let Some(session) = session.as_mut() else { return None; };
            if session.live_allowed && !session.live.accept(reply, &session.nonce, Instant::now()) {
                state.set_live_status(session.live.status);
                return Some(ErrorCode::Unknown("live_storage_unavailable".into()));
            }
            state.set_live_status(session.live.status);
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
        assert_eq!(heartbeat, "{\"v\":3,\"type\":\"heartbeat\",\"nonce\":\"Zk3m1Qw9Lr0aT7yUc2Vb5g\",\"seq\":2}\n");
        let bye = session.bye(ByeReason::AddonUnload, start + Duration::from_secs(7)).unwrap();
        assert!(bye.contains(r#""seq":3"#) && bye.contains("addon_unload"), "{bye}");
    }

    #[test]
    fn an_ack_between_a_context_and_a_heartbeat_keeps_the_sequence_consecutive() {
        let start = Instant::now();
        let mut session = Session::new(welcome(5000), start);
        session.next_outgoing(start, &gameplay(50)).unwrap(); // seq 0
        let ack = session.ack_alert(17, start + Duration::from_secs(1)).unwrap(); // seq 1
        assert_eq!(ack, "{\"v\":3,\"type\":\"alert_ack\",\"nonce\":\"Zk3m1Qw9Lr0aT7yUc2Vb5g\",\"seq\":1,\"alertSeq\":17}\n");
        let context = session.next_outgoing(start + Duration::from_secs(2), &gameplay(866)).unwrap(); // seq 2
        assert!(context.contains(r#""seq":2"#), "{context}");
        let second_ack = session.ack_alert(18, start + Duration::from_secs(3)).unwrap(); // seq 3
        assert!(second_ack.contains(r#""seq":3"#) && second_ack.contains(r#""alertSeq":18"#), "{second_ack}");
        // The ack restarted the heartbeat clock: 4 s after it there is still nothing to send.
        assert_eq!(session.next_outgoing(start + Duration::from_secs(7), &gameplay(866)), None);
        let heartbeat = session.next_outgoing(start + Duration::from_secs(8), &gameplay(866)).unwrap(); // seq 4
        assert!(heartbeat.contains(r#""seq":4"#), "{heartbeat}");
        let bye = session.bye(ByeReason::GameExit, start + Duration::from_secs(9)).unwrap();
        assert!(bye.contains(r#""seq":5"#), "{bye}");
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
    fn only_a_connection_that_lasted_starts_the_backoff_over() {
        let slowest = Duration::from_millis(*crate::backoff::DELAYS_MS.last().unwrap());
        let mut backoff = Backoff::new();
        // Never welcomed, or welcomed and gone before it settled: each one is a step up.
        for lasted in [None, Some(Duration::ZERO), Some(Duration::from_millis(300)), Some(STABLE_CONNECTION - Duration::from_millis(1))] {
            let before = backoff.delay();
            record_connection(&mut backoff, lasted);
            assert!(backoff.delay() > before, "{lasted:?}: {before:?} then {:?}", backoff.delay());
        }
        assert_eq!(backoff.delay(), slowest);
        // One that lived long enough is a server worth retrying quickly again.
        record_connection(&mut backoff, Some(STABLE_CONNECTION));
        assert_eq!(backoff.delay(), Duration::from_millis(250));
        // A reset can never make the reconnections more frequent than the table's slowest step.
        assert!(STABLE_CONNECTION >= slowest);
    }

    /// What the README says about the waits, as the loop takes them: it records how the
    /// attempt ended and then reads the delay.
    #[test]
    fn the_first_wait_after_a_failure_is_500_ms_and_250_only_follows_a_connection_that_lasted() {
        let mut backoff = Backoff::new();
        let mut wait_after = |lasted: Option<Duration>| {
            record_connection(&mut backoff, lasted);
            backoff.delay().as_millis()
        };
        // Nobody listening, from the first attempt on.
        let waits: Vec<u128> = (0..6).map(|_| wait_after(None)).collect();
        assert_eq!(waits, [500, 1_000, 2_000, 5_000, 5_000, 5_000]);
        // A connection that lived its ten seconds, and what comes if the retry after it fails.
        assert_eq!(wait_after(Some(STABLE_CONNECTION)), 250);
        assert_eq!([wait_after(None), wait_after(None)], [500, 1_000]);
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

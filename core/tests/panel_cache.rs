//! The panel computed only when it can have changed (`panel::PanelCache`) against the panel
//! computed from scratch on every frame, which is what the addon did before: the same view and
//! the same memory, frame after frame, through a session with everything that happens in one.
//!
//! Both panels read the same `SharedState`, driven through its own setters the way the client
//! loop drives it, and are asked at the same instants: thousands of frames a few milliseconds
//! apart, and frames exactly on the whole seconds the panel's rules turn on.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tyrian_companion_nexus_core::bags::{BagCoverage, BagSlots};
use tyrian_companion_nexus_core::inventory::Diagnostics;
use tyrian_companion_nexus_core::live::LiveStatus;
use tyrian_companion_nexus_core::magic_find::{MagicFind, MagicFindCoverage};
use tyrian_companion_nexus_core::panel::{self, Frame, PanelCache, PanelMemory, PanelSources, Tone};
use tyrian_companion_nexus_core::passive::Uncovered;
use tyrian_companion_nexus_core::protocol::{parse_server_line, GameContext, GameState, ServerLine};
use tyrian_companion_nexus_core::state::{SharedState, Status};
use tyrian_companion_nexus_core::wallet::{WalletCoverage, WalletError};

const NONCE: &str = "Zk3m1Qw9Lr0aT7yUc2Vb5g";
const OTHER_NONCE: &str = "Yk3m1Qw9Lr0aT7yUc2Vb5g";

fn farming_frame(phase: &str, elapsed: i32, lo: Value, hi: Value) -> Value {
    json!({ "v":3, "type":"farming_state", "tag":"farm1", "nonce":NONCE, "seq":1, "ttl":15,
        "phase":phase, "err":null, "elapsed":elapsed, "observed":143, "net":null,
        "lo":lo, "hi":hi, "age":0, "slots":63, "slotSrc":"ingame", "slotAge":2,
        "goal":"bags", "target":500, "progress":143, "eta":900, "mf":333,
        "mfKind":"partial", "prep":"partial" })
}

fn price_frame(st: &str) -> Value {
    let figures = st == "ok";
    json!({ "v":3, "type":"price_state", "tag":"price2", "nonce":NONCE, "seq":1, "ttl":15, "st":st,
        "sell": if figures { json!(283) } else { Value::Null }, "sellStack": if figures { json!(70762) } else { Value::Null },
        "list": if figures { json!(356) } else { Value::Null }, "listStack": if figures { json!(89037) } else { Value::Null },
        "age": if st == "pending" || st == "idle" { Value::Null } else { json!(50) } })
}

fn magic_find(effects: f32) -> MagicFindCoverage {
    MagicFindCoverage::Read(MagicFind { total: 330.0 + effects, luck: 300, pushed: 30.0, buffs: effects, boon: false })
}

fn bags(free: u32) -> BagCoverage {
    BagCoverage::Read(BagSlots { capacity: 160, occupied: 160 - free, free, bag_slots: 8, bags: 8 })
}

/// Something the client loop does to the shared state.
#[derive(Debug, Clone)]
enum Event {
    /// A `welcome` and the two capabilities.
    Connect(&'static str),
    Disconnect,
    Context(GameState, Option<&'static str>),
    /// A `farm1` frame; the world numbers it.
    Farm(Value),
    Price(&'static str),
    /// A capture that worked, with what its three extra readers got.
    Cycle(BagCoverage, MagicFindCoverage, WalletCoverage),
    /// A capture that failed as a whole: nothing read, the source unavailable for a second.
    CaptureFailed,
    Live(LiveStatus),
    /// A pass of the loop with nothing new: the same context and the same status, set again.
    Repeat,
}

struct World {
    state: SharedState,
    start: Instant,
    nonce: &'static str,
    sequence: i32,
    context: Option<GameContext>,
}

impl World {
    fn new() -> Self {
        Self { state: SharedState::new(), start: Instant::now(), nonce: NONCE, sequence: 0, context: None }
    }

    fn at(&self, millis: u64) -> Instant {
        self.start + Duration::from_millis(millis)
    }

    /// Applies `event` as of `at`, through the setters the client loop uses.
    fn apply(&mut self, event: &Event, at: Instant) {
        let state = &self.state;
        match event {
            Event::Connect(nonce) => {
                self.nonce = nonce;
                state.begin_farming_connection(nonce);
                state.begin_price_connection(nonce);
                state.set_live_status(LiveStatus::NotNegotiated);
                state.set_status(Status::Connected);
                assert!(state.enable_farming(nonce) && state.enable_price(nonce));
                state.set_live_status(LiveStatus::Waiting);
            }
            Event::Disconnect => {
                state.disconnect_farming();
                state.disconnect_price();
                state.set_live_status(LiveStatus::Unavailable);
                state.set_status(Status::WaitingForPlugin);
            }
            Event::Context(game, character) => {
                let context = GameContext { state: *game, map_id: character.map(|_| 866), character: character.map(str::to_string) };
                state.set_live_context(context.clone());
                self.context = Some(context);
            }
            Event::Farm(frame) => {
                self.sequence += 1;
                let mut frame = frame.clone();
                frame["seq"] = json!(self.sequence);
                frame["nonce"] = json!(self.nonce);
                let ServerLine::FarmingState(reading) = parse_server_line(&frame.to_string()) else { panic!("{frame}") };
                assert!(state.accept_farming(reading, at), "{frame}");
            }
            Event::Price(st) => {
                self.sequence += 1;
                let mut frame = price_frame(st);
                frame["seq"] = json!(self.sequence);
                frame["nonce"] = json!(self.nonce);
                let ServerLine::PriceState(reading) = parse_server_line(&frame.to_string()) else { panic!("{frame}") };
                assert!(state.accept_price(reading, at), "{frame}");
            }
            Event::Cycle(bags, magic_find, wallet) => {
                state.set_inventory_diagnostics(Diagnostics { bags: *bags, magic_find: *magic_find, wallet: *wallet, ..Diagnostics::default() }, at);
                state.set_live_status(LiveStatus::Measuring);
            }
            Event::CaptureFailed => {
                state.set_inventory_diagnostics(Diagnostics::default(), at);
                state.set_live_status(LiveStatus::Unavailable);
            }
            Event::Live(live) => state.set_live_status(*live),
            Event::Repeat => {
                let before = state.panel_generation();
                if let Some(context) = self.context.clone() {
                    state.set_live_context(context);
                }
                state.set_live_status(state.live_status());
                state.set_status(state.status());
                assert_eq!(state.panel_generation(), before, "setting what was already there is not a change");
            }
        }
    }
}

/// The two panels: the one that computes every frame, and the one under test.
#[derive(Default)]
struct Panels {
    every_frame: PanelMemory,
    cached: PanelMemory,
    cache: PanelCache,
    frames: u64,
    /// The status dots and how many distinct panels were painted, to see that the session
    /// really went through its states.
    dots: Vec<Tone>,
    distinct: BTreeSet<String>,
}

impl Panels {
    fn frame(&mut self, world: &World, now: Instant, english: bool, frame: Frame) {
        // What the addon did: the whole panel from scratch on every frame, also to paint only
        // the bar of a folded one, and `observe` for a closed one.
        let sources = PanelSources::read(&world.state, now);
        let expected = match frame {
            Frame::Hidden => {
                panel::observe(&sources.input(), &mut self.every_frame);
                None
            }
            Frame::Folded | Frame::Painted => Some(panel::view(&sources.input(), &mut self.every_frame, english)),
        };
        let cached = self.cache.frame(&world.state, &mut self.cached, now, english, frame).cloned();
        let after = now.duration_since(world.start);
        match frame {
            Frame::Painted => assert_eq!(cached, expected, "frame {} at +{after:?}", self.frames),
            Frame::Hidden | Frame::Folded => assert_eq!(cached, None, "nothing to paint at +{after:?}"),
        }
        assert_eq!(self.cached, self.every_frame, "the memory after frame {} at +{after:?} ({frame:?})", self.frames);
        if let Some(view) = expected.filter(|_| frame == Frame::Painted) {
            if self.dots.last() != Some(&view.status_dot) {
                self.dots.push(view.status_dot);
            }
            self.distinct.insert(format!("{view:?}"));
        }
        self.frames += 1;
    }
}

/// A session, in milliseconds from its start: no connection, a connection, a session that
/// prepares and measures, the reader's cycles with their failures, a slow plugin, feeds that go
/// stale, a loading screen, another character, a lost connection and a new one.
fn session() -> Vec<(u64, Event)> {
    let listed = WalletCoverage::Listed(55);
    let mut script = vec![
        (400, Event::Connect(NONCE)),
        (650, Event::Context(GameState::Gameplay, Some("Astra Uno"))),
        (900, Event::Farm(farming_frame("idle", 0, Value::Null, Value::Null))),
        (1_150, Event::Price("pending")),
        (2_000, Event::Farm(farming_frame("starting", 0, Value::Null, Value::Null))),
        (5_050, Event::Farm(farming_frame("active", 3, json!(480), Value::Null))),
        (5_300, Event::Price("ok")),
        // A band in the stretch where the rate keeps the shape it had, which is a range, and a
        // narrow one. Then, with the panel folded, a wide band that turns it into an average
        // and the stretch again: unfolded, what it shows depends on what it was while folded.
        (10_050, Event::Farm(farming_frame("active", 8, json!(400), json!(500)))),
        (15_050, Event::Farm(farming_frame("active", 13, json!(480), json!(560)))),
        (20_050, Event::Farm(farming_frame("active", 18, json!(300), json!(900)))),
        (25_050, Event::Farm(farming_frame("active", 23, json!(400), json!(500)))),
        (25_300, Event::Price("ok")),
        // Then no frame for sixteen seconds: the 15 s of both feeds run out.
        (41_500, Event::Farm(farming_frame("active", 40, json!(37), json!(38)))),
        (41_700, Event::Price("stale")),
        // A loading screen long enough for the status to stop waiting for the last capture, the
        // same character back, character select, another character.
        (44_000, Event::Context(GameState::Loading, Some("Astra Uno"))),
        (44_010, Event::Live(LiveStatus::Unavailable)),
        (47_250, Event::Context(GameState::Gameplay, Some("Astra Uno"))),
        (48_000, Event::Context(GameState::CharacterSelect, None)),
        (48_010, Event::Live(LiveStatus::Unavailable)),
        (55_000, Event::Context(GameState::Gameplay, Some("Bruma Dos"))),
        (56_500, Event::Farm(farming_frame("active", 55, json!(37), json!(38)))),
        (61_500, Event::Farm(farming_frame("complete", 60, json!(37), json!(38)))),
        // The connection is lost and comes back, and another session starts on it.
        (64_000, Event::Disconnect),
        (66_000, Event::Connect(OTHER_NONCE)),
        (66_400, Event::Farm(farming_frame("active", 2, json!(480), json!(560)))),
        (66_600, Event::Price("ok")),
        (70_000, Event::Live(LiveStatus::Conflict)),
        (72_000, Event::Live(LiveStatus::Waiting)),
    ];
    // The reader's cycles, one a second while a character is in a map and the source samples,
    // with what goes wrong in them. Each at its own fraction of a second, so that no two of the
    // instants the panel counts from turn their whole seconds at the same moment.
    let changed = (BagCoverage::Unavailable(Uncovered::Changed), MagicFindCoverage::Unavailable(Uncovered::Changed));
    let no_wallet = WalletCoverage::Unavailable(WalletError::Changed);
    for second in (2..=30).chain(38..=41).chain(47..=47).chain(56..=63).chain(67..=69).chain(73..=78) {
        let at = second * 1_000 + 330 + (second * 37) % 170;
        let event = match second {
            7 => Event::CaptureFailed,
            9 => Event::Cycle(changed.0, MagicFindCoverage::Unavailable(Uncovered::Deadline), listed),
            11 => Event::Cycle(bags(41), magic_find(3.0), no_wallet),
            // Six captures in a row fail, and five wallets after them: past the 5 s of the dot.
            16..=21 => Event::CaptureFailed,
            23..=27 => Event::Cycle(bags(8), magic_find(3.0), no_wallet),
            // The two lines get readings of different ages: the bags are last read at 28, the
            // Magic Find at 29, and the capture of 30 works and reads neither. From then to 37 s
            // the plugin is slow and no cycle runs: each reading ages on its own past the 2 s at
            // which its tooltip says so and the 5 s it is held for.
            28 => Event::Cycle(bags(8), MagicFindCoverage::Unavailable(Uncovered::Deadline), listed),
            29 => Event::Cycle(changed.0, magic_find(3.0), listed),
            30 => Event::Cycle(changed.0, changed.1, listed),
            // The wallet is listed once and not in the three captures after it, the last of
            // which reads neither line; then no cycle until the loading screen. The 5 s of the
            // wallet, of the lines and of the last capture each run out at a moment of its own.
            38 => Event::Cycle(bags(41), magic_find(3.0), listed),
            39 | 40 => Event::Cycle(bags(40), magic_find(3.0), no_wallet),
            41 => Event::Cycle(changed.0, changed.1, no_wallet),
            3..=6 => Event::Cycle(bags(41), magic_find(53.0), listed),
            _ => Event::Cycle(bags(41 - (second % 7) as u32), magic_find(3.0 + (second % 3) as f32), listed),
        };
        script.push((at, event));
    }
    // And on every pass in between, four a second, the client sets what is already there.
    script.extend((0..320).map(|pass| (pass * 250 + 125, Event::Repeat)));
    script.sort_by_key(|(at, _)| *at);
    script
}

/// What the frame at this moment does with the panel, and in which language.
fn frame_at(millis: u64) -> (bool, Frame) {
    match millis {
        12_000..=13_999 => (true, Frame::Painted),
        // Folded while the rate's band widens and comes back to the stretch that depends on its
        // past, and unfolded on that band.
        17_000..=27_999 => (false, Frame::Folded),
        31_000..=33_999 => (false, Frame::Hidden),
        // Closed while a session ends and another starts, and reopened in English.
        60_000..=67_999 => (false, Frame::Hidden),
        68_000..=70_999 => (true, Frame::Painted),
        _ => (false, Frame::Painted),
    }
}

/// Plays the session at the frame rate `seed` gives. Returns the panels for what was seen.
fn play(seed: u64) -> Panels {
    const END: u64 = 80_000;
    let mut world = World::new();
    let script = session();
    // A frame exactly on every moment an event happens, and exactly 1, 2, 5, 6, 15 and 16 s
    // after it: the instants the ages and the holds are counted from, on their whole seconds.
    let mut exact: BTreeSet<u64> = script.iter().flat_map(|(at, _)| [0, 1_000, 2_000, 5_000, 6_000, 15_000, 16_000].map(|after| at + after)).collect();
    exact.retain(|at| *at < END);
    let (mut panels, mut seed, mut micros, mut next_event) = (Panels::default(), seed, 0u64, 0usize);
    while micros < END * 1_000 {
        // Between 4 and 21 ms a frame, never a whole number of milliseconds, and now and then a
        // hitch longer than the quarter of a second after which a frame is computed anyway.
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        let step = if (seed >> 40) % 97 == 0 { 310_007 } else { 4_003 + (seed >> 33) % 17_000 };
        let wanted = micros + step;
        micros = match exact.range((micros / 1_000 + 1)..).next() {
            Some(&at) if at * 1_000 <= wanted => at * 1_000,
            _ => wanted,
        };
        let now = world.start + Duration::from_micros(micros);
        while let Some((at, event)) = script.get(next_event).filter(|(at, _)| *at * 1_000 <= micros) {
            let at = world.at(*at);
            world.apply(event, at);
            next_event += 1;
        }
        let (english, frame) = frame_at(micros / 1_000);
        panels.frame(&world, now, english, frame);
    }
    assert_eq!(next_event, script.len(), "the whole session was played");
    panels
}

#[test]
fn the_cached_panel_is_the_panel_computed_on_every_frame() {
    for seed in [1, 20_261_008, 0x5eed_cafe] {
        let panels = play(seed);
        // The session went through its states: every colour of the dot, and many panels.
        for dot in [Tone::Error, Tone::Muted, Tone::Good, Tone::Warning] {
            assert!(panels.dots.contains(&dot), "seed {seed}: the dot was never {dot:?} in {:?}", panels.dots);
        }
        assert!(panels.distinct.len() > 60, "seed {seed}: only {} distinct panels", panels.distinct.len());
        // Unfolded on the band that depends on its past: an average, because it was one folded.
        assert!(panels.distinct.iter().any(|view| view.contains("~450 b/h")), "seed {seed}: the rate was never the average of 400–500");
        // And most frames reused what was there: that is what the cache is for.
        let computed = panels.cache.computed();
        assert!(panels.frames > 4_000, "seed {seed}: {} frames", panels.frames);
        assert!(computed * 6 < panels.frames, "seed {seed}: {computed} of {} frames were computed", panels.frames);
        // At least once every quarter of a second, whatever else stands still.
        assert!(computed >= 80 * 4, "seed {seed}: {computed} computed in 80 s");
    }
}

/// A reading dated after the frame's own instant: the worker stored it between the moment the
/// frame took its instant and the moment it read the state. Its age is zero until the clock
/// reaches it, and the frames around it are still the ones computed from scratch.
#[test]
fn a_reading_dated_after_the_frame_is_followed_as_well() {
    let mut world = World::new();
    world.apply(&Event::Connect(NONCE), world.at(0));
    world.apply(&Event::Context(GameState::Gameplay, Some("Astra Uno")), world.at(0));
    world.apply(&Event::Farm(farming_frame("active", 3, json!(480), json!(560))), world.at(0));
    let mut panels = Panels::default();
    panels.frame(&world, world.at(100), false, Frame::Painted);
    world.apply(&Event::Cycle(bags(41), magic_find(53.0), WalletCoverage::Listed(55)), world.at(108));
    for micros in (100_500..1_400_000).step_by(3_700).chain([2_108_000, 5_108_000, 5_108_001, 7_000_000]) {
        panels.frame(&world, world.start + Duration::from_micros(micros), false, Frame::Painted);
    }
}

/// The source of the panel's input, as the addon and these tests take it from the state.
#[test]
fn the_character_in_a_map_comes_from_the_reported_context() {
    let world = World::new();
    let in_map = |world: &World| {
        let sources = PanelSources::read(&world.state, world.at(0));
        let input = sources.input();
        (input.character_in_map, input.character.map(str::to_string))
    };
    assert_eq!(in_map(&world), (true, None), "nothing reported yet: not put down to the character");
    world.state.set_live_context(GameContext { state: GameState::Gameplay, map_id: Some(866), character: Some("Astra Uno".into()) });
    assert_eq!(in_map(&world), (true, Some("Astra Uno".to_string())));
    world.state.set_live_context(GameContext { state: GameState::Loading, map_id: Some(866), character: Some("Astra Uno".into()) });
    assert_eq!(in_map(&world), (false, Some("Astra Uno".to_string())), "a loading screen");
    world.state.set_live_context(GameContext::character_select());
    assert_eq!(in_map(&world), (false, None), "character select");
}

//! Whether the game is really closing: the rule behind a `bye` with `game_exit`.
//!
//! The SPEC (`docs/SPEC-puente-ingame.md`, `bye`) accepts only positive evidence that the game
//! is closing, and for Nexus that evidence is the game window's `WM_CLOSE` or `WM_DESTROY`. They
//! are not equally strong:
//!
//! - `WM_DESTROY` is final: the window is gone and the process follows. Nothing takes it back.
//! - `WM_CLOSE` is a *request*. The game can refuse it (a confirmation dialog the player
//!   cancels) and carry on, and Windows sends no message to say so. It still has to count at
//!   once, because a real close can end the process within milliseconds of `WM_CLOSE`, too soon
//!   for anything slower to get the `bye` out.
//!
//! So `WM_CLOSE` is **provisional**: the addon says `game_exit` as soon as it sees it, and takes
//! it back if the game turns out to be alive. The only sign of life that needs nothing from
//! Windows is the Mumble Link's `uiTick`, which the game advances while it renders: if it is
//! still advancing more than [`CLOSE_GRACE`] after the `WM_CLOSE`, the close was cancelled (or
//! has not happened), the addon stops being mute and reconnects. If the window then really
//! closes, `WM_DESTROY` (or a new `WM_CLOSE`) says `game_exit` again.
//!
//! This is plain logic with no clock of its own, so `cargo test` covers it on Linux; the
//! `WndProc` in the addon only reports the two messages.

use std::time::{Duration, Instant};

/// How long after a `WM_CLOSE` the game has to keep ticking before the close counts as
/// cancelled. A game that is shutting down for real stops writing the link well within this;
/// a longer wait only delays the addon's return after a cancelled close.
pub const CLOSE_GRACE: Duration = Duration::from_secs(5);

/// What the window messages said so far, and whether the game has shown signs of life since.
#[derive(Debug, Clone)]
pub struct ExitWatch {
    grace: Duration,
    state: State,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// No close seen, or the last one was taken back.
    Running,
    /// `WM_CLOSE` seen at `since`; `last_tick` is the last `uiTick` read after it.
    CloseRequested { since: Instant, last_tick: Option<u32> },
    /// `WM_DESTROY` seen. Final.
    Destroyed,
}

impl Default for ExitWatch {
    fn default() -> Self {
        Self::new()
    }
}

impl ExitWatch {
    /// A watch with the standard [`CLOSE_GRACE`]; `const` so the addon can keep it in a static.
    pub const fn new() -> Self {
        Self::with_grace(CLOSE_GRACE)
    }

    /// A watch whose provisional close expires `grace` after the `WM_CLOSE` (tests shorten it).
    pub const fn with_grace(grace: Duration) -> Self {
        Self { grace, state: State::Running }
    }

    /// The window received `WM_CLOSE` at `now`. A second one while one is pending (the player
    /// tried again) restarts the grace; after `WM_DESTROY` it changes nothing.
    pub fn close_requested(&mut self, now: Instant) {
        if self.state != State::Destroyed {
            self.state = State::CloseRequested { since: now, last_tick: None };
        }
    }

    /// The window received `WM_DESTROY`: the game is closing, for good.
    pub fn destroyed(&mut self) {
        self.state = State::Destroyed;
    }

    /// `true` while a `WM_CLOSE` is pending: the caller reads the link's `uiTick` only then.
    pub fn watching_ticks(&self) -> bool {
        matches!(self.state, State::CloseRequested { .. })
    }

    /// Feeds one reading of the link's `uiTick` (`None` if it could not be read) taken at
    /// `now`. A provisional close ends when the tick has moved *after* the grace window.
    pub fn observe(&mut self, now: Instant, ui_tick: Option<u32>) {
        let State::CloseRequested { since, last_tick } = self.state else { return };
        let Some(tick) = ui_tick else { return };
        if last_tick.is_some_and(|previous| previous != tick) && now.saturating_duration_since(since) >= self.grace {
            self.state = State::Running;
        } else {
            self.state = State::CloseRequested { since, last_tick: Some(tick) };
        }
    }

    /// `true` while the game counts as closing: a pending `WM_CLOSE` or a `WM_DESTROY`.
    pub fn is_exiting(&self) -> bool {
        self.state != State::Running
    }

    /// The whole step the host runs each time the client asks whether the game is closing:
    /// reads the tick (through `read_tick`, only while a `WM_CLOSE` is pending), feeds it, and
    /// answers [`ExitWatch::is_exiting`].
    pub fn exiting(&mut self, now: Instant, read_tick: impl FnOnce() -> Option<u32>) -> bool {
        if self.watching_ticks() {
            self.observe(now, read_tick());
        }
        self.is_exiting()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRACE: Duration = Duration::from_secs(5);

    fn at(base: Instant, secs: u64) -> Instant {
        base + Duration::from_secs(secs)
    }

    #[test]
    fn nothing_seen_is_not_exiting() {
        let mut watch = ExitWatch::default();
        assert!(!watch.exiting(Instant::now(), || Some(1)));
    }

    #[test]
    fn a_close_counts_at_once() {
        let base = Instant::now();
        let mut watch = ExitWatch::default();
        watch.close_requested(base);
        assert!(watch.exiting(base, || Some(100)));
    }

    #[test]
    fn a_close_with_a_game_that_stops_ticking_stays_exiting() {
        let base = Instant::now();
        let mut watch = ExitWatch::default();
        watch.close_requested(base);
        for secs in 0..30 {
            assert!(watch.exiting(at(base, secs), || Some(100)), "frozen tick at {secs} s");
        }
    }

    #[test]
    fn a_close_with_an_unreadable_link_stays_exiting() {
        let base = Instant::now();
        let mut watch = ExitWatch::default();
        watch.close_requested(base);
        for secs in 0..30 {
            assert!(watch.exiting(at(base, secs), || None));
        }
    }

    #[test]
    fn a_game_that_keeps_ticking_after_the_grace_takes_the_close_back() {
        let base = Instant::now();
        let mut watch = ExitWatch::with_grace(GRACE);
        watch.close_requested(base);
        // Ticking inside the grace is not enough: a closing game can still draw for a moment.
        assert!(watch.exiting(at(base, 0), || Some(100)));
        assert!(watch.exiting(at(base, 2), || Some(180)));
        assert!(watch.exiting(at(base, 4), || Some(260)));
        assert!(!watch.exiting(at(base, 6), || Some(340)));
        assert!(!watch.watching_ticks());
    }

    #[test]
    fn destroy_is_final_whatever_the_game_does_afterwards() {
        let base = Instant::now();
        let mut watch = ExitWatch::default();
        watch.close_requested(base);
        watch.destroyed();
        for secs in 0..30 {
            assert!(watch.exiting(at(base, secs), || Some(secs as u32 * 80)));
        }
        watch.close_requested(at(base, 40));
        assert!(watch.is_exiting());
    }

    #[test]
    fn destroy_without_a_close_counts() {
        let mut watch = ExitWatch::default();
        watch.destroyed();
        assert!(watch.is_exiting());
    }

    #[test]
    fn a_close_after_a_cancelled_one_counts_again() {
        let base = Instant::now();
        let mut watch = ExitWatch::with_grace(GRACE);
        watch.close_requested(base);
        watch.exiting(at(base, 0), || Some(1));
        assert!(!watch.exiting(at(base, 6), || Some(500)));
        watch.close_requested(at(base, 10));
        assert!(watch.exiting(at(base, 10), || Some(900)));
    }

    #[test]
    fn a_second_close_restarts_the_grace() {
        let base = Instant::now();
        let mut watch = ExitWatch::with_grace(GRACE);
        watch.close_requested(base);
        watch.exiting(at(base, 0), || Some(1));
        watch.close_requested(at(base, 4));
        // 6 s after the first close, but only 2 s after the second.
        assert!(watch.exiting(at(base, 5), || Some(2)));
        assert!(watch.exiting(at(base, 6), || Some(3)));
        assert!(!watch.exiting(at(base, 10), || Some(4)));
    }

    #[test]
    fn the_tick_is_only_read_while_a_close_is_pending() {
        let base = Instant::now();
        let mut watch = ExitWatch::default();
        assert!(!watch.exiting(base, || panic!("read with nothing pending")));
        watch.destroyed();
        assert!(watch.exiting(base, || panic!("read after a final destroy")));
    }
}

//! A verdict that is kept for good only once it is final.
//!
//! The addon verifies the game's executable once per load, by hashing it, before any reader
//! touches a game field. That check has two final answers, "the certified build" and "another
//! build", and several ways of not reaching one: the hash did not fit its time, the file could
//! not be opened or read. Keeping whichever came first for the whole load turned a slow disk on
//! the first attempt into "unsupported build" until the addon was loaded again.
//!
//! [`Verdict`] keeps a final answer for ever and a failed attempt only for a wait, after which
//! the next call tries again. An attempt may also do its work a bounded part at a time and say
//! it is not finished: that is neither kept nor waited for, and the next call goes on with it.
//! How long those parts have taken, added up, is kept ([`Verdict::unfinished_for`]), so that the
//! caller can stop keeping quiet about work that goes on for too long without giving it up.
//! It reads no clock of its own: the caller hands one in, which is what lets the tests move
//! time.

use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// How one attempt at a verdict ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attempt<T, E> {
    /// A final verdict, whatever it says.
    Settled(T),
    /// Not there yet, by design: the attempt does a bounded part of the work on each call so
    /// that its caller is never held for long. The error is what to answer meanwhile.
    Unfinished(E),
    /// No verdict could be reached now. The error is what to answer until the next attempt.
    Failed(E),
}

/// A final verdict reached at most once, and a failure to reach one that is tried again after
/// a wait instead of on every call.
#[derive(Debug)]
pub struct Verdict<T, E> {
    settled: OnceLock<T>,
    unsettled: Mutex<Unsettled<E>>,
    wait: Duration,
}

/// What is known while there is no verdict.
#[derive(Debug)]
struct Unsettled<E> {
    /// The last attempt that reached no verdict: when it ended, and why.
    failed: Option<(Instant, E)>,
    /// How long the attempts left unfinished have taken, added up, since the work they are
    /// doing began: since the first call, or since the last attempt that failed.
    unfinished: Duration,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl<T, E: Copy> Verdict<T, E> {
    /// `wait` is how long after an attempt that reached no verdict the next one may run.
    pub const fn new(wait: Duration) -> Self {
        Self { settled: OnceLock::new(), unsettled: Mutex::new(Unsettled { failed: None, unfinished: Duration::ZERO }), wait }
    }

    /// The settled verdict, reaching for it with `attempt` when there is none yet.
    ///
    /// A [`Attempt::Settled`] verdict is kept and `attempt` never runs again. An
    /// [`Attempt::Unfinished`] one is work left half done on purpose: its error is handed back
    /// and the next call goes on with it at once. An [`Attempt::Failed`] one is handed back
    /// as it is, and handed back again without running `attempt` until `wait` has passed since
    /// that attempt ended: `clock` is read again after it, and the wait is between attempts.
    ///
    /// `clock` is read before and after an attempt, which is how the time an unfinished one
    /// took is known ([`Verdict::unfinished_for`]).
    pub fn get_or_try(&self, clock: impl Fn() -> Instant, attempt: impl FnOnce() -> Attempt<T, E>) -> Result<&T, E> {
        if let Some(settled) = self.settled.get() {
            return Ok(settled);
        }
        // Held through the attempt: two callers never verify at the same time.
        let mut unsettled = lock(&self.unsettled);
        if let Some(settled) = self.settled.get() {
            return Ok(settled);
        }
        let began = clock();
        if let Some((ended, error)) = unsettled.failed {
            if began.saturating_duration_since(ended) < self.wait {
                return Err(error);
            }
        }
        match attempt() {
            Attempt::Settled(verdict) => {
                *unsettled = Unsettled { failed: None, unfinished: Duration::ZERO };
                Ok(self.settled.get_or_init(|| verdict))
            }
            Attempt::Unfinished(error) => {
                unsettled.failed = None;
                unsettled.unfinished = unsettled.unfinished.saturating_add(clock().saturating_duration_since(began));
                Err(error)
            }
            Attempt::Failed(error) => {
                // What was under way is lost with the failure: the attempt after the wait
                // starts the work again, and its time is counted from nothing.
                *unsettled = Unsettled { failed: Some((clock(), error)), unfinished: Duration::ZERO };
                Err(error)
            }
        }
    }

    /// How long the work that is under way has taken so far: the time of every attempt left
    /// unfinished since it began, added up. Zero when nothing is under way, which is before the
    /// first attempt, after one that failed and once there is a verdict.
    ///
    /// It is the time spent in the attempts, not the time gone by since the first of them: a
    /// caller that stops asking for a while is not doing the work meanwhile, and that while is
    /// not counted. It only says how long; what is too long is the caller's to say, and nothing
    /// here gives the work up or starts it again for having taken long.
    ///
    /// Waits for an attempt that is running, like [`Verdict::get_or_try`].
    pub fn unfinished_for(&self) -> Duration {
        lock(&self.unsettled).unfinished
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const WAIT: Duration = Duration::from_secs(30);

    /// What the executable check comes to, as the addon uses this: a reader, or another build.
    type Build = Option<&'static str>;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct TimedOut;

    /// A clock the test moves, and how many times the attempt ran.
    struct Follow {
        verdict: Verdict<Build, TimedOut>,
        now: Cell<Instant>,
        attempts: Cell<u32>,
    }

    impl Follow {
        fn new() -> Self {
            Self { verdict: Verdict::new(WAIT), now: Cell::new(Instant::now()), attempts: Cell::new(0) }
        }

        fn after(&self, time: Duration) {
            self.now.set(self.now.get() + time);
        }

        fn ask(&self, outcome: Result<Build, TimedOut>) -> Result<Build, TimedOut> {
            self.asking_for(Duration::ZERO, outcome)
        }

        /// One call whose attempt, if it runs, takes `takes` of the clock and ends for good:
        /// with a verdict, or without one.
        fn asking_for(&self, takes: Duration, outcome: Result<Build, TimedOut>) -> Result<Build, TimedOut> {
            self.attempting(takes, match outcome {
                Ok(build) => Attempt::Settled(build),
                Err(error) => Attempt::Failed(error),
            })
        }

        /// One call whose attempt, if it runs, takes `takes` of the clock and ends as `outcome`.
        fn attempting(&self, takes: Duration, outcome: Attempt<Build, TimedOut>) -> Result<Build, TimedOut> {
            self.verdict
                .get_or_try(
                    || self.now.get(),
                    || {
                        self.attempts.set(self.attempts.get() + 1);
                        self.after(takes);
                        outcome
                    },
                )
                .copied()
        }
    }

    /// The executable is hashed a slice on each call, so that the caller, which also keeps a
    /// connection alive, is never held for long. A slice that leaves work to do is neither a
    /// verdict nor a failure: nothing is kept, and nothing is waited for.
    #[test]
    fn an_unfinished_attempt_goes_on_at_the_next_call_without_a_wait() {
        let follow = Follow::new();
        let slice = Duration::from_millis(250);
        for call in 1..=5 {
            assert_eq!(follow.attempting(slice, Attempt::Unfinished(TimedOut)), Err(TimedOut));
            assert_eq!(follow.attempts.get(), call, "every call does its slice");
            follow.after(Duration::from_secs(1));
        }
        // What the last slice reaches is the verdict, and it is kept.
        assert_eq!(follow.attempting(slice, Attempt::Settled(Some("reader"))), Ok(Some("reader")));
        assert_eq!(follow.attempts.get(), 6);
        assert_eq!(follow.attempting(slice, Attempt::Unfinished(TimedOut)), Ok(Some("reader")));
        assert_eq!(follow.attempts.get(), 6);
    }

    /// A hash that goes on and on, a cold slow disk, used to say nothing for as long as it
    /// took. The time of its slices is added up for the caller to see, and nothing else changes
    /// for it: every call still does its slice, and what the last one reaches is the verdict.
    #[test]
    fn the_time_of_the_unfinished_attempts_adds_up_and_the_work_goes_on_all_the_same() {
        let follow = Follow::new();
        let slice = Duration::from_secs(1);
        assert_eq!(follow.verdict.unfinished_for(), Duration::ZERO, "nothing under way yet");
        for call in 1..=25 {
            assert_eq!(follow.attempting(slice, Attempt::Unfinished(TimedOut)), Err(TimedOut));
            assert_eq!(follow.attempts.get(), call, "every call does its slice, however long it has been");
            assert_eq!(follow.verdict.unfinished_for(), slice * call);
            // Between two slices the caller does something else: that is not time of the work.
            follow.after(Duration::from_millis(250));
        }
        // The caller stops asking for a minute, as on a loading screen. No slice, no time.
        follow.after(Duration::from_secs(60));
        assert_eq!(follow.verdict.unfinished_for(), slice * 25);
        assert_eq!(follow.attempting(slice, Attempt::Settled(Some("reader"))), Ok(Some("reader")));
        assert_eq!(follow.attempts.get(), 26, "one more slice, not the work over again");
        assert_eq!(follow.verdict.unfinished_for(), Duration::ZERO, "nothing under way any more");
    }

    /// Every attempt that fails opens its own wait, and the work after it is another: its time
    /// is counted from nothing, not on top of the one that was lost.
    #[test]
    fn the_work_that_starts_again_after_a_failure_has_its_own_time() {
        let follow = Follow::new();
        let slice = Duration::from_secs(1);
        for _ in 0..12 {
            assert_eq!(follow.attempting(slice, Attempt::Unfinished(TimedOut)), Err(TimedOut));
        }
        assert_eq!(follow.verdict.unfinished_for(), slice * 12);
        // The file cannot be read any more.
        assert_eq!(follow.attempting(slice, Attempt::Failed(TimedOut)), Err(TimedOut));
        assert_eq!(follow.verdict.unfinished_for(), Duration::ZERO);
        // Its wait, during which nothing runs and no time is added.
        follow.after(WAIT - Duration::from_millis(1));
        assert_eq!(follow.attempting(slice, Attempt::Unfinished(TimedOut)), Err(TimedOut));
        assert_eq!((follow.attempts.get(), follow.verdict.unfinished_for()), (13, Duration::ZERO));
        follow.after(Duration::from_millis(1));
        for call in 1..=3 {
            assert_eq!(follow.attempting(slice, Attempt::Unfinished(TimedOut)), Err(TimedOut));
            assert_eq!(follow.verdict.unfinished_for(), slice * call);
        }
    }

    #[test]
    fn a_failure_among_the_slices_is_waited_for_and_the_work_starts_again_after_it() {
        let follow = Follow::new();
        assert_eq!(follow.attempting(Duration::ZERO, Attempt::Unfinished(TimedOut)), Err(TimedOut));
        // The file cannot be read any more: that is a failure, and it is not tried on every call.
        assert_eq!(follow.attempting(Duration::ZERO, Attempt::Failed(TimedOut)), Err(TimedOut));
        assert_eq!(follow.attempts.get(), 2);
        follow.after(WAIT - Duration::from_millis(1));
        assert_eq!(follow.attempting(Duration::ZERO, Attempt::Unfinished(TimedOut)), Err(TimedOut));
        assert_eq!(follow.attempts.get(), 2, "still waiting");
        follow.after(Duration::from_millis(1));
        // After the wait the slices go on one per call again.
        for call in 3..=5 {
            assert_eq!(follow.attempting(Duration::ZERO, Attempt::Unfinished(TimedOut)), Err(TimedOut));
            assert_eq!(follow.attempts.get(), call);
        }
        assert_eq!(follow.attempting(Duration::ZERO, Attempt::Settled(None)), Ok(None), "another build, for good");
    }

    #[test]
    fn a_final_verdict_is_reached_once_and_kept() {
        for build in [Some("reader"), None] {
            let follow = Follow::new();
            assert_eq!(follow.ask(Ok(build)), Ok(build));
            // Whatever a later attempt would say, there is none: the certified build stays
            // certified, and another build stays another build for the life of the load.
            follow.after(Duration::from_secs(3600));
            assert_eq!(follow.ask(Err(TimedOut)), Ok(build));
            assert_eq!(follow.ask(Ok(Some("another"))), Ok(build));
            assert_eq!(follow.attempts.get(), 1);
        }
    }

    /// The fault of the audit: hashing the executable ran out of time once, on a cold disk, and
    /// that first answer was kept for the whole load.
    #[test]
    fn an_attempt_that_reached_no_verdict_is_tried_again_after_the_wait() {
        let follow = Follow::new();
        assert_eq!(follow.ask(Err(TimedOut)), Err(TimedOut));
        assert_eq!(follow.attempts.get(), 1);
        // Not on every call: the reader asks once a second, and an attempt hashes the whole file.
        for _ in 0..29 {
            follow.after(Duration::from_secs(1));
            assert_eq!(follow.ask(Ok(Some("reader"))), Err(TimedOut), "still the failure of before");
        }
        assert_eq!(follow.attempts.get(), 1);
        // After the wait the next call tries again, and what it reaches is kept.
        follow.after(Duration::from_secs(1));
        assert_eq!(follow.ask(Ok(Some("reader"))), Ok(Some("reader")));
        assert_eq!(follow.attempts.get(), 2);
        follow.after(Duration::from_secs(3600));
        assert_eq!(follow.ask(Err(TimedOut)), Ok(Some("reader")));
        assert_eq!(follow.attempts.get(), 2);
    }

    #[test]
    fn every_failed_attempt_starts_its_own_wait_from_the_moment_it_ended() {
        let follow = Follow::new();
        // Ten seconds of hashing that came to nothing: the wait counts from their end.
        assert_eq!(follow.asking_for(Duration::from_secs(10), Err(TimedOut)), Err(TimedOut));
        follow.after(WAIT - Duration::from_millis(1));
        assert_eq!(follow.ask(Ok(None)), Err(TimedOut));
        assert_eq!(follow.attempts.get(), 1);
        follow.after(Duration::from_millis(1));
        // It fails again: another whole wait, not an attempt per call from here on.
        assert_eq!(follow.ask(Err(TimedOut)), Err(TimedOut));
        assert_eq!(follow.attempts.get(), 2);
        follow.after(WAIT - Duration::from_millis(1));
        assert_eq!(follow.ask(Ok(None)), Err(TimedOut));
        assert_eq!(follow.attempts.get(), 2);
        follow.after(Duration::from_millis(1));
        assert_eq!(follow.ask(Ok(None)), Ok(None), "another build, and that is final");
        assert_eq!(follow.attempts.get(), 3);
    }
}

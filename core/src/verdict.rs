//! A verdict that is kept for good only once it is final.
//!
//! The addon verifies the game's executable once per load, by hashing it, before any reader
//! touches a game field. That check has two final answers, "the certified build" and "another
//! build", and several ways of not reaching one: the hash did not fit its time, the file could
//! not be opened or read. Keeping whichever came first for the whole load turned a slow disk on
//! the first attempt into "unsupported build" until the addon was loaded again.
//!
//! [`Verdict`] keeps a final answer for ever and an unfinished attempt only for a wait, after
//! which the next call tries again. It reads no clock of its own: the caller hands one in,
//! which is what lets the tests move time.

use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// A final verdict reached at most once, and a failure to reach one that is tried again after
/// a wait instead of on every call.
#[derive(Debug)]
pub struct Verdict<T, E> {
    settled: OnceLock<T>,
    /// The last attempt that reached no verdict: when it ended, and why.
    failed: Mutex<Option<(Instant, E)>>,
    wait: Duration,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl<T, E: Copy> Verdict<T, E> {
    /// `wait` is how long after an attempt that reached no verdict the next one may run.
    pub const fn new(wait: Duration) -> Self {
        Self { settled: OnceLock::new(), failed: Mutex::new(None), wait }
    }

    /// The settled verdict, reaching for it with `attempt` when there is none yet.
    ///
    /// `attempt` answers `Ok` with a verdict that is final, whatever it says, and `Err` when it
    /// could not reach one. A final verdict is kept and `attempt` never runs again. An `Err` is
    /// handed back as it is, and handed back again without running `attempt` until `wait` has
    /// passed since that attempt ended: `clock` is read again after it, as an attempt can take
    /// long, and the wait is between attempts.
    pub fn get_or_try(&self, clock: impl Fn() -> Instant, attempt: impl FnOnce() -> Result<T, E>) -> Result<&T, E> {
        if let Some(settled) = self.settled.get() {
            return Ok(settled);
        }
        // Held through the attempt: two callers never verify at the same time.
        let mut failed = lock(&self.failed);
        if let Some(settled) = self.settled.get() {
            return Ok(settled);
        }
        if let Some((ended, error)) = *failed {
            if clock().saturating_duration_since(ended) < self.wait {
                return Err(error);
            }
        }
        match attempt() {
            Ok(verdict) => {
                *failed = None;
                Ok(self.settled.get_or_init(|| verdict))
            }
            Err(error) => {
                *failed = Some((clock(), error));
                Err(error)
            }
        }
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

        /// One call whose attempt, if it runs, takes `takes` of the clock.
        fn asking_for(&self, takes: Duration, outcome: Result<Build, TimedOut>) -> Result<Build, TimedOut> {
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

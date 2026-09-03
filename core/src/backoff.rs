//! The reconnect backoff table `docs/SPEC-puente-ingame.md` fixes: `[250, 500, 1000, 2000,
//! 5000]` ms, saturating at the last value, forever. Reused from H8's own table by idea, not
//! by import (`src/platform/` is not a dependency of this addon; see the spec's "Reparto").
//!
//! "Sin servidor" is the expected steady state right after the game launches, since the
//! plugin lives inside Obsidian and the player is free to start either one first, so this
//! module never gives up: it only ever gets slower, and only up to the table's last step.

use std::time::Duration;

/// Delays in milliseconds, in order. The last one repeats forever once reached.
pub const DELAYS_MS: [u64; 5] = [250, 500, 1_000, 2_000, 5_000];

/// Tracks how many consecutive failed connection attempts have happened since the last
/// success, and hands back how long to wait before the next one.
#[derive(Debug, Default, Clone, Copy)]
pub struct Backoff {
    attempt: usize,
}

impl Backoff {
    pub fn new() -> Self {
        Self { attempt: 0 }
    }

    /// The delay to wait before the *next* attempt, given this many attempts have already
    /// failed. Saturates at the table's last entry instead of growing or panicking past it.
    pub fn delay(&self) -> Duration {
        let index = self.attempt.min(DELAYS_MS.len() - 1);
        Duration::from_millis(DELAYS_MS[index])
    }

    /// Records a failed attempt, advancing towards (and then staying at) the slowest delay.
    pub fn record_failure(&mut self) {
        self.attempt = self.attempt.saturating_add(1);
    }

    /// Records a successful connection: the next failure starts the table over from `250ms`,
    /// since a server that was just reachable is worth retrying quickly again.
    pub fn record_success(&mut self) {
        self.attempt = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_at_the_first_delay() {
        assert_eq!(Backoff::new().delay(), Duration::from_millis(250));
    }

    #[test]
    fn walks_the_table_in_order() {
        let mut backoff = Backoff::new();
        let expected = [250, 500, 1_000, 2_000, 5_000];
        for delay_ms in expected {
            assert_eq!(backoff.delay(), Duration::from_millis(delay_ms));
            backoff.record_failure();
        }
    }

    #[test]
    fn saturates_at_the_last_delay_forever() {
        let mut backoff = Backoff::new();
        for _ in 0..50 {
            backoff.record_failure();
        }
        assert_eq!(backoff.delay(), Duration::from_millis(5_000));
    }

    #[test]
    fn a_success_resets_the_table() {
        let mut backoff = Backoff::new();
        for _ in 0..10 {
            backoff.record_failure();
        }
        assert_eq!(backoff.delay(), Duration::from_millis(5_000));
        backoff.record_success();
        assert_eq!(backoff.delay(), Duration::from_millis(250));
    }
}

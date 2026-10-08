//! Counters for the "Reader diagnostics" of the Options window: how long the reader's passes
//! and the panel's frame take, how the captures end, and how many threads the process had.
//!
//! They exist to answer in the game what has only been estimated outside it: whether the
//! passes fit their deadlines and what the panel costs a frame. They are local to the addon.
//! Nothing here is on the wire, in `live1` or in `farm1`, and nothing here reads the game: the
//! Windows adapter takes the clock around the passes it already runs and hands the durations
//! in.
//!
//! Durations are kept in whole microseconds, or nanoseconds for the frame, and written with
//! integer arithmetic only: no rounding function of the C runtime is pulled into the DLL.

use std::time::Duration;

use crate::inventory::ReadError;

fn micros(time: Duration) -> u64 {
    u64::try_from(time.as_micros()).unwrap_or(u64::MAX)
}

/// How long one kind of pass takes: the last cycle's, and the longest since the addon loaded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PassTime {
    /// Microseconds in the last cycle, `None` when that cycle did not reach this pass.
    pub last: Option<u64>,
    /// The longest it has taken, in microseconds.
    pub max: u64,
}

impl PassTime {
    pub const fn new() -> Self {
        Self { last: None, max: 0 }
    }

    /// One cycle's time for this pass, `None` when the cycle ended before it.
    pub fn record(&mut self, took: Option<Duration>) {
        self.last = took.map(micros);
        self.max = self.max.max(self.last.unwrap_or(0));
    }
}

/// What one cycle of the reader took, pass by pass. A pass the cycle did not reach is `None`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CycleTimes {
    /// Finding the game's context among the process's own threads.
    pub threads: Option<Duration>,
    pub inventory: Option<Duration>,
    pub wallet: Option<Duration>,
    pub bags: Option<Duration>,
    pub magic_find: Option<Duration>,
    /// The whole cycle, from its start to its result.
    pub cycle: Duration,
}

/// How the captures ended, counted since the addon loaded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Captures {
    pub ok: u64,
    /// The inventory changed under the copy.
    pub changed: u64,
    /// A copy failed while there was still time.
    pub read_failed: u64,
    /// A copy was refused because the cycle's time had run out.
    pub deadline: u64,
    /// A count, a pointer or a read budget was out of bounds.
    pub bounds: u64,
    /// No character to read, a profile that does not match, or another build.
    pub other: u64,
}

impl Captures {
    pub const fn new() -> Self {
        Self { ok: 0, changed: 0, read_failed: 0, deadline: 0, bounds: 0, other: 0 }
    }

    /// Counts one capture by how it ended. The adapter refuses a copy after the deadline with
    /// the same `ReadFailed` as a copy that fails, so `out_of_time`, the cycle's deadline having
    /// passed when it ended, is what tells the two apart.
    pub fn count(&mut self, error: Option<ReadError>, out_of_time: bool) {
        let counter = match error {
            None => &mut self.ok,
            Some(ReadError::Changed) => &mut self.changed,
            Some(ReadError::ReadFailed) if out_of_time => &mut self.deadline,
            Some(ReadError::ReadFailed) => &mut self.read_failed,
            Some(ReadError::Bounds) => &mut self.bounds,
            Some(ReadError::RootUnavailable | ReadError::ProfileMismatch | ReadError::UnsupportedBuild) => &mut self.other,
        };
        *counter = counter.saturating_add(1);
    }

    pub fn total(&self) -> u64 {
        [self.ok, self.changed, self.read_failed, self.deadline, self.bounds, self.other].into_iter().fold(0, u64::saturating_add)
    }
}

/// Everything the reader counts about its own cycles.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReaderCounters {
    pub threads: PassTime,
    pub inventory: PassTime,
    pub wallet: PassTime,
    pub bags: PassTime,
    pub magic_find: PassTime,
    pub cycle: PassTime,
    pub captures: Captures,
    /// The most threads of its own the process has had in one cycle.
    pub most_threads: u32,
}

impl ReaderCounters {
    pub const fn new() -> Self {
        Self {
            threads: PassTime::new(),
            inventory: PassTime::new(),
            wallet: PassTime::new(),
            bags: PassTime::new(),
            magic_find: PassTime::new(),
            cycle: PassTime::new(),
            captures: Captures::new(),
            most_threads: 0,
        }
    }

    /// One cycle: what each pass took, how the capture ended (`None` for one that worked),
    /// whether its deadline had passed by then, and how many threads of its own the process had.
    pub fn record(&mut self, times: CycleTimes, error: Option<ReadError>, out_of_time: bool, own_threads: u32) {
        self.threads.record(times.threads);
        self.inventory.record(times.inventory);
        self.wallet.record(times.wallet);
        self.bags.record(times.bags);
        self.magic_find.record(times.magic_find);
        self.cycle.record(Some(times.cycle));
        self.captures.count(error, out_of_time);
        self.most_threads = self.most_threads.max(own_threads);
    }

    /// The three lines of the Options window. `epochs` is how many `live_open` the source has
    /// declared since the addon loaded, and `thread_limit` the adapter's cap on own threads.
    pub fn lines(&self, epochs: u64, thread_limit: u32) -> [String; 3] {
        let pass = |name: &str, time: PassTime| match time.last {
            Some(last) => format!("{name} {last} (max {})", time.max),
            None => format!("{name} not run (max {})", time.max),
        };
        let captures = self.captures;
        [
            format!(
                "Pass times in µs, last cycle (max): {}; {}; {}; {}; {}; {}",
                pass("threads", self.threads),
                pass("inventory", self.inventory),
                pass("wallet", self.wallet),
                pass("bags", self.bags),
                pass("Magic Find", self.magic_find),
                pass("whole cycle", self.cycle),
            ),
            format!(
                "Captures: {} ok; {} Changed; {} ReadFailed; {} Deadline; {} Bounds; {} other; epochs opened: {epochs}",
                captures.ok, captures.changed, captures.read_failed, captures.deadline, captures.bounds, captures.other
            ),
            format!("Most own threads in a cycle: {} / {thread_limit}", self.most_threads),
        ]
    }
}

/// How long a frame of the panel takes: the mean and the longest since the addon loaded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameTime {
    frames: u64,
    /// Nanoseconds, as a frame that reuses what was computed takes well under a microsecond.
    total: u64,
    longest: u64,
}

impl FrameTime {
    pub const fn new() -> Self {
        Self { frames: 0, total: 0, longest: 0 }
    }

    pub fn record(&mut self, took: Duration) {
        let nanos = u64::try_from(took.as_nanos()).unwrap_or(u64::MAX);
        self.frames = self.frames.saturating_add(1);
        self.total = self.total.saturating_add(nanos);
        self.longest = self.longest.max(nanos);
    }

    /// The line of the Options window. `computed` is how many of those frames had to work the
    /// panel out (`panel::PanelCache::computed`); the rest reused what was there.
    pub fn line(&self, computed: u64) -> String {
        if self.frames == 0 {
            return "Panel frame: none yet".to_string();
        }
        format!(
            "Panel frame in µs: mean {}, max {}, over {} frames ({computed} computed)",
            tenths(self.total / self.frames),
            tenths(self.longest),
            self.frames
        )
    }
}

/// Nanoseconds as microseconds with one decimal, rounded to the nearest tenth.
fn tenths(nanos: u64) -> String {
    let tenths = nanos.saturating_add(50) / 100;
    format!("{}.{}", tenths / 10, tenths % 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn micros_of(value: u64) -> Option<Duration> {
        Some(Duration::from_micros(value))
    }

    #[test]
    fn a_pass_keeps_its_last_time_and_its_longest() {
        let mut pass = PassTime::new();
        assert_eq!(pass, PassTime { last: None, max: 0 });
        pass.record(micros_of(300));
        pass.record(micros_of(1_900));
        pass.record(micros_of(210));
        assert_eq!(pass, PassTime { last: Some(210), max: 1_900 });
        // A cycle that ended before this pass: no last time, and the longest stays.
        pass.record(None);
        assert_eq!(pass, PassTime { last: None, max: 1_900 });
        // Whole microseconds, rounded down.
        pass.record(Some(Duration::from_nanos(2_999)));
        assert_eq!(pass.last, Some(2));
    }

    #[test]
    fn captures_are_counted_by_how_they_ended() {
        let mut captures = Captures::new();
        for _ in 0..7 {
            captures.count(None, false);
        }
        captures.count(Some(ReadError::Changed), false);
        captures.count(Some(ReadError::Changed), true);
        captures.count(Some(ReadError::Bounds), false);
        // The same error from the adapter: a copy that failed, and one refused by the clock.
        captures.count(Some(ReadError::ReadFailed), false);
        captures.count(Some(ReadError::ReadFailed), true);
        captures.count(Some(ReadError::ReadFailed), true);
        for error in [ReadError::RootUnavailable, ReadError::ProfileMismatch, ReadError::UnsupportedBuild] {
            captures.count(Some(error), false);
        }
        assert_eq!(captures, Captures { ok: 7, changed: 2, read_failed: 1, deadline: 2, bounds: 1, other: 3 });
        assert_eq!(captures.total(), 16);
        // A capture that worked is one that worked, however long it took.
        captures.count(None, true);
        assert_eq!((captures.ok, captures.deadline), (8, 2));
    }

    #[test]
    fn the_reader_lines_say_what_a_screenshot_needs() {
        let mut counters = ReaderCounters::new();
        assert_eq!(
            counters.lines(0, 128),
            [
                "Pass times in µs, last cycle (max): threads not run (max 0); inventory not run (max 0); wallet not run (max 0); \
                 bags not run (max 0); Magic Find not run (max 0); whole cycle not run (max 0)",
                "Captures: 0 ok; 0 Changed; 0 ReadFailed; 0 Deadline; 0 Bounds; 0 other; epochs opened: 0",
                "Most own threads in a cycle: 0 / 128",
            ]
        );
        let whole = CycleTimes {
            threads: micros_of(1_900),
            inventory: micros_of(5_200),
            wallet: micros_of(450),
            bags: micros_of(300),
            magic_find: micros_of(41_000),
            cycle: Duration::from_micros(48_900),
        };
        counters.record(whole, None, false, 61);
        // The next one runs out of time in the inventory: the passes after it did not run.
        let cut = CycleTimes { threads: micros_of(210), inventory: micros_of(749_800), cycle: Duration::from_micros(750_020), ..CycleTimes::default() };
        counters.record(cut, Some(ReadError::ReadFailed), true, 58);
        assert_eq!(
            counters.lines(7, 128),
            [
                "Pass times in µs, last cycle (max): threads 210 (max 1900); inventory 749800 (max 749800); wallet not run (max 450); \
                 bags not run (max 300); Magic Find not run (max 41000); whole cycle 750020 (max 750020)",
                "Captures: 1 ok; 0 Changed; 0 ReadFailed; 1 Deadline; 0 Bounds; 0 other; epochs opened: 7",
                "Most own threads in a cycle: 61 / 128",
            ]
        );
        assert_eq!(counters.captures.total(), 2);
    }

    #[test]
    fn the_frame_time_is_a_mean_and_a_longest_in_tenths_of_a_microsecond() {
        let mut frame = FrameTime::new();
        assert_eq!(frame.line(0), "Panel frame: none yet");
        // Nine frames that reuse what was computed, and one that computes.
        for _ in 0..9 {
            frame.record(Duration::from_nanos(240));
        }
        frame.record(Duration::from_nanos(41_250));
        // (9 × 240 + 41 250) / 10 = 4341 ns.
        assert_eq!(frame.line(1), "Panel frame in µs: mean 4.3, max 41.3, over 10 frames (1 computed)");
        assert_eq!(tenths(0), "0.0");
        assert_eq!(tenths(49), "0.0");
        assert_eq!(tenths(50), "0.1");
        assert_eq!(tenths(999_950), "1000.0");
        assert_eq!(tenths(u64::MAX), "18446744073709551.6");
    }
}

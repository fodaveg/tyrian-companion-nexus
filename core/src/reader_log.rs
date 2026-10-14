//! What the bag and Magic Find readers leave in the addon's log, and what they count for the
//! "Reader diagnostics" of the Options window.
//!
//! The panel shows a figure or `—`, and the reason for a `—` only while someone looks at it. A
//! session in which Magic Find had no figure most of the time left nothing behind to say why
//! (13 October 2026). So each reader's outcome is followed here: a line when it becomes
//! another (read, not read, or no coverage with its reason), never a line per pass, and a
//! count of the passes that ended in each. A Magic Find pass that refuses a record also leaves
//! that record, once for each different one.
//!
//! Everything here is local to the addon, and nothing here reads the game: the adapter hands
//! in what its passes returned. A line carries reasons, counts, ids of effects and the figures
//! the panel already shows. No address, no name and nothing of the settings goes into one.

use std::time::{Duration, Instant};

use crate::bags::BagCoverage;
use crate::magic_find::{Condition, MagicFindCoverage, RefusedRecord, Trace};
use crate::panel::points;
use crate::passive::Uncovered;

/// How many changes of outcome one reader logs in one load. A reader that goes back and forth
/// on every pass would otherwise write a line a second for as long as the game is open; past
/// this many it says so once and stops, and the counters go on.
pub const MAX_CHANGES: u32 = 256;
/// How often the counters of both readers are written to the log.
pub const TALLY_INTERVAL: Duration = Duration::from_secs(600);
/// How many different refused records are logged in one load.
pub const MAX_RECORDS: usize = 32;

/// Every reason a pass can end without a value for, in the order the counters are written.
const REASONS: [Uncovered; 10] = [
    Uncovered::Guard,
    Uncovered::Profile,
    Uncovered::Root,
    Uncovered::Bounds,
    Uncovered::Alignment,
    Uncovered::Integrity,
    Uncovered::Unsupported,
    Uncovered::Changed,
    Uncovered::ReadFailed,
    Uncovered::Deadline,
];

/// How one pass of a reader ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Read,
    /// The cycle did not get to this reader: the inventory capture before it failed.
    NotRead,
    Unavailable(Uncovered),
}
impl From<MagicFindCoverage> for Outcome {
    fn from(coverage: MagicFindCoverage) -> Self {
        match coverage {
            MagicFindCoverage::Read(_) => Self::Read,
            MagicFindCoverage::NotRead => Self::NotRead,
            MagicFindCoverage::Unavailable(reason) => Self::Unavailable(reason),
        }
    }
}
impl From<BagCoverage> for Outcome {
    fn from(coverage: BagCoverage) -> Self {
        match coverage {
            BagCoverage::Read(_) => Self::Read,
            BagCoverage::NotRead => Self::NotRead,
            BagCoverage::Unavailable(reason) => Self::Unavailable(reason),
        }
    }
}
impl Outcome {
    /// As the log and the counters write it: `read`, `not read`, or the reason's own name.
    pub fn label(self) -> String {
        match self {
            Self::Read => "read".to_string(),
            Self::NotRead => "not read".to_string(),
            Self::Unavailable(reason) => format!("{reason:?}"),
        }
    }
}

/// One reader's outcomes since the addon loaded: how many passes ended in each, and the one
/// that is going on.
#[derive(Debug)]
pub struct Track {
    name: &'static str,
    read: u64,
    not_read: u64,
    unavailable: [u64; REASONS.len()],
    /// The outcome of the last pass, since when it has been the outcome and for how many passes.
    current: Option<(Outcome, Instant, u64)>,
    logged: u32,
}
impl Track {
    pub const fn new(name: &'static str) -> Self {
        Self { name, read: 0, not_read: 0, unavailable: [0; REASONS.len()], current: None, logged: 0 }
    }

    /// Counts one pass. Returns the line to log when its outcome is another than the pass
    /// before's, or when it is the first one: what it is now, what it was and for how long,
    /// and `detail`, which is only asked for when there is a line to write.
    pub fn observe(&mut self, outcome: Outcome, now: Instant, detail: impl FnOnce() -> String) -> Option<String> {
        let counter = match outcome {
            Outcome::Read => &mut self.read,
            Outcome::NotRead => &mut self.not_read,
            Outcome::Unavailable(reason) => {
                let at = REASONS.iter().position(|known| *known == reason).unwrap_or(0);
                &mut self.unavailable[at]
            }
        };
        *counter = counter.saturating_add(1);
        let before = match &mut self.current {
            Some((last, _, passes)) if *last == outcome => {
                *passes = passes.saturating_add(1);
                return None;
            }
            before => before.replace((outcome, now, 1)),
        };
        if self.logged > MAX_CHANGES {
            return None;
        }
        self.logged += 1;
        if self.logged > MAX_CHANGES {
            return Some(format!(
                "reader: {}: {MAX_CHANGES} changes logged, no more until the addon is loaded again; the counters in Options go on",
                self.name
            ));
        }
        let was = match before {
            Some((last, since, passes)) => format!(
                "after {} for {passes} passes ({} s)",
                last.label(),
                now.saturating_duration_since(since).as_secs()
            ),
            None => "first pass".to_string(),
        };
        Some(format!("reader: {}: {}, {was}; {}", self.name, outcome.label(), detail()))
    }

    /// The line of the Options window: the passes that ended in each outcome. Reasons no pass
    /// has ended in are left out.
    pub fn tally(&self) -> String {
        let mut line = format!("{} passes: {} read; {} not read", self.name, self.read, self.not_read);
        for (reason, count) in REASONS.iter().zip(self.unavailable) {
            if count != 0 {
                line.push_str(&format!("; {count} {reason:?}"));
            }
        }
        line
    }
}

/// What tells one refused record from another for the log: the effect, the game mode, the
/// formula, the flags and what each of the four conditions names. Whether a named effect is
/// applied is not part of it: it comes and goes, and the record is the same.
type RecordKey = (u32, u32, u32, u32, [(bool, Option<u32>); 4]);
fn record_key(record: &RefusedRecord) -> RecordKey {
    let named = |condition: Condition| (condition.present, condition.word);
    let [trait_a, trait_b] = record.traits;
    let [effect_a, effect_b] = record.effects;
    (
        record.effect,
        record.mode,
        record.formula,
        record.flags,
        [named(trait_a), named(trait_b), named(effect_a), named(effect_b)],
    )
}

/// The two readers' tracks and the refused records already logged. One per load, behind the
/// adapter's lock.
#[derive(Debug)]
pub struct ReaderLog {
    pub magic_find: Track,
    pub bags: Track,
    records: Vec<RecordKey>,
    /// When the next counters line is due; set by the first cycle.
    tally_due: Option<Instant>,
}
impl Default for ReaderLog {
    fn default() -> Self {
        Self::new()
    }
}
impl ReaderLog {
    pub const fn new() -> Self {
        Self { magic_find: Track::new("Magic Find"), bags: Track::new("Bags"), records: Vec::new(), tally_due: None }
    }

    /// The counters of both readers as one line, every [`TALLY_INTERVAL`] counted from the first
    /// cycle: the same figures the Options window shows, for a session nobody was watching.
    fn tally_line(&mut self, now: Instant) -> Option<String> {
        let due = *self.tally_due.get_or_insert(now + TALLY_INTERVAL);
        if now < due {
            return None;
        }
        self.tally_due = Some(now + TALLY_INTERVAL);
        Some(format!("reader: tally: {}; {}", self.bags.tally(), self.magic_find.tally()))
    }

    /// The line for a record a Magic Find pass refused, the first time that record is seen and
    /// while fewer than [`MAX_RECORDS`] different ones have been logged.
    pub fn refused(&mut self, record: &RefusedRecord) -> Option<String> {
        let key = record_key(record);
        if self.records.len() >= MAX_RECORDS || self.records.contains(&key) {
            return None;
        }
        self.records.push(key);
        Some(record_line(record))
    }

    /// One cycle of the reader: counts both passes and returns the lines to log, none on most
    /// cycles. `capture` is how the inventory capture before them ended when it failed, which is
    /// why a reader was not read. The byte and read counts are each reader's own for this
    /// cycle, and the times what its pass took, `None` when the cycle did not reach it.
    #[allow(clippy::too_many_arguments)]
    pub fn cycle(
        &mut self,
        now: Instant,
        capture: Option<&str>,
        bags: BagCoverage,
        bag_cost: (u32, u32, Option<Duration>),
        magic_find: MagicFindCoverage,
        magic_find_cost: (u32, u32, Option<Duration>),
        trace: &Trace,
    ) -> Vec<String> {
        let mut lines = Vec::new();
        lines.extend(self.bags.observe(bags.into(), now, || bags_detail(bags, bag_cost, capture)));
        lines.extend(
            self.magic_find
                .observe(magic_find.into(), now, || magic_find_detail(magic_find, magic_find_cost, trace, capture)),
        );
        if magic_find == MagicFindCoverage::Unavailable(Uncovered::Unsupported) {
            lines.extend(trace.refused.as_ref().and_then(|record| self.refused(record)));
        }
        lines.extend(self.tally_line(now));
        lines
    }
}

fn cost((bytes, reads, took): (u32, u32, Option<Duration>), limit: usize) -> String {
    let took = match took {
        Some(took) => format!("{} us", took.as_micros()),
        None => "not run".to_string(),
    };
    format!("bytes {bytes}/{limit}, reads {reads}, pass {took}")
}

/// What follows the outcome in a bag line: the figures when it read, and what the pass cost.
pub fn bags_detail(coverage: BagCoverage, spent: (u32, u32, Option<Duration>), capture: Option<&str>) -> String {
    match coverage {
        BagCoverage::NotRead => format!("capture {}", capture.unwrap_or("did not reach it")),
        BagCoverage::Read(slots) => format!(
            "{} used of {}, {} free; {}",
            slots.occupied,
            slots.capacity,
            slots.free,
            cost(spent, crate::bags::MAX_BYTES)
        ),
        BagCoverage::Unavailable(_) => cost(spent, crate::bags::MAX_BYTES),
    }
}

/// What follows the outcome in a Magic Find line: the total and its addends when it read, what
/// the pass cost, and the figures its bounds are judged by, as far as the pass got.
pub fn magic_find_detail(
    coverage: MagicFindCoverage,
    spent: (u32, u32, Option<Duration>),
    trace: &Trace,
    capture: Option<&str>,
) -> String {
    let seen = format!(
        "{}; table {}/{}, pushed {}, buffs {}, definitions {}, most records {}",
        cost(spent, crate::magic_find::MAX_BYTES),
        trace.count,
        trace.capacity,
        trace.pushed,
        trace.buffs,
        trace.definitions,
        trace.most_records
    );
    match coverage {
        MagicFindCoverage::NotRead => format!("capture {}", capture.unwrap_or("did not reach it")),
        MagicFindCoverage::Read(read) => format!(
            "total {} = luck {} + pushed {} + buffs {}, boon {}; {seen}",
            points(read.total),
            read.luck,
            points(read.pushed),
            points(read.buffs),
            if read.boon { "yes" } else { "no" }
        ),
        MagicFindCoverage::Unavailable(_) => seen,
    }
}

fn condition(condition: Condition) -> String {
    if !condition.present {
        return "none".to_string();
    }
    let word = match condition.word {
        Some(word) => word.to_string(),
        None => "unread".to_string(),
    };
    match condition.applied {
        Some(true) => format!("{word} applied"),
        Some(false) => format!("{word} not applied"),
        None => word,
    }
}

/// The line for a refused record: the applied effect that carries it and every field of the
/// record the client's sum looks at. The value is written as the panel would and as its bits.
pub fn record_line(record: &RefusedRecord) -> String {
    let [trait_a, trait_b] = record.traits;
    let [effect_a, effect_b] = record.effects;
    format!(
        "reader: Magic Find record refused: effect {} record {} kind {:#x} formula {} value {} ({:#010x}) mode {} flags {:#x} words {:#x}/{:#x}/{:#x} trait+0x20 {} trait+0x30 {} effect+0x38 {} effect+0x40 {}",
        record.effect,
        record.index,
        record.kind,
        record.formula,
        points(record.value),
        record.value.to_bits(),
        record.mode,
        record.flags,
        record.words[0],
        record.words[1],
        record.words[2],
        condition(trait_a),
        condition(trait_b),
        condition(effect_a),
        condition(effect_b),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bags::BagSlots;
    use crate::magic_find::MagicFind;

    fn record(effect: u32, mode: u32) -> RefusedRecord {
        RefusedRecord {
            effect,
            index: 2,
            kind: 0x71,
            formula: 6,
            value: 100.0,
            mode,
            flags: 0,
            words: [0, 7, 0],
            traits: [Condition::default(); 2],
            effects: [
                Condition { present: true, word: Some(34567), applied: Some(false) },
                Condition::default(),
            ],
        }
    }
    const SPENT: (u32, u32, Option<Duration>) = (20112, 201, Some(Duration::from_micros(1432)));
    fn read() -> MagicFindCoverage {
        MagicFindCoverage::Read(MagicFind { total: 363.0, luck: 300, pushed: 13.0, buffs: 50.0, boon: false })
    }
    fn trace() -> Trace {
        Trace { capacity: 256, count: 91, pushed: 4, buffs: 91, definitions: 47, most_records: 5, refused: None }
    }

    #[test]
    fn a_line_is_written_when_the_outcome_becomes_another_and_never_for_a_pass_that_repeats_it() {
        let start = Instant::now();
        let mut track = Track::new("Magic Find");
        let unsupported = Outcome::Unavailable(Uncovered::Unsupported);
        assert_eq!(
            track.observe(unsupported, start, || "detail".into()).as_deref(),
            Some("reader: Magic Find: Unsupported, first pass; detail")
        );
        // The same outcome again, for as long as it lasts: no line, and no detail is built.
        for second in 1..1800 {
            let line = track.observe(unsupported, start + Duration::from_secs(second), || panic!("no line, no detail"));
            assert_eq!(line, None);
        }
        assert_eq!(
            track.observe(Outcome::Read, start + Duration::from_secs(1800), || "total 363".into()).as_deref(),
            Some("reader: Magic Find: read, after Unsupported for 1800 passes (1800 s); total 363")
        );
        // One reason to another is a change as well, and so is a cycle that did not reach it.
        let later = start + Duration::from_secs(1810);
        assert_eq!(track.observe(Outcome::Read, start + Duration::from_secs(1801), String::new), None);
        assert_eq!(
            track.observe(Outcome::Unavailable(Uncovered::Changed), later, || "x".into()).as_deref(),
            Some("reader: Magic Find: Changed, after read for 2 passes (10 s); x")
        );
        assert_eq!(
            track.observe(Outcome::Unavailable(Uncovered::Deadline), later, || "y".into()).as_deref(),
            Some("reader: Magic Find: Deadline, after Changed for 1 passes (0 s); y")
        );
        assert_eq!(
            track.observe(Outcome::NotRead, later, || "capture Changed".into()).as_deref(),
            Some("reader: Magic Find: not read, after Deadline for 1 passes (0 s); capture Changed")
        );
        assert_eq!(track.tally(), "Magic Find passes: 2 read; 1 not read; 1800 Unsupported; 1 Changed; 1 Deadline");
    }

    #[test]
    fn the_counters_count_every_pass_and_name_only_the_reasons_that_happened() {
        let now = Instant::now();
        let mut track = Track::new("Bags");
        assert_eq!(track.tally(), "Bags passes: 0 read; 0 not read");
        for reason in REASONS {
            track.observe(Outcome::Unavailable(reason), now, String::new);
            track.observe(Outcome::Unavailable(reason), now, String::new);
        }
        track.observe(Outcome::Read, now, String::new);
        assert_eq!(
            track.tally(),
            "Bags passes: 1 read; 0 not read; 2 Guard; 2 Profile; 2 Root; 2 Bounds; 2 Alignment; 2 Integrity; \
             2 Unsupported; 2 Changed; 2 ReadFailed; 2 Deadline"
        );
    }

    #[test]
    fn a_reader_that_flaps_stops_logging_after_its_cap_and_goes_on_counting() {
        let now = Instant::now();
        let mut track = Track::new("Magic Find");
        let mut lines = Vec::new();
        for pass in 0..1000u32 {
            let outcome = if pass % 2 == 0 { Outcome::Read } else { Outcome::Unavailable(Uncovered::Changed) };
            lines.extend(track.observe(outcome, now, String::new));
        }
        // Every change up to the cap, then one line that says there will be no more.
        assert_eq!(lines.len(), MAX_CHANGES as usize + 1);
        assert_eq!(
            lines.last().map(String::as_str),
            Some("reader: Magic Find: 256 changes logged, no more until the addon is loaded again; the counters in Options go on")
        );
        assert!(lines[..lines.len() - 1].iter().all(|line| line.contains("Magic Find: read") || line.contains("Magic Find: Changed")));
        assert_eq!(track.tally(), "Magic Find passes: 500 read; 0 not read; 500 Changed");
    }

    #[test]
    fn a_refused_record_is_logged_once_for_each_different_one_and_at_most_32() {
        let mut log = ReaderLog::new();
        let line = log.refused(&record(12345, 4));
        assert_eq!(
            line.as_deref(),
            Some(
                "reader: Magic Find record refused: effect 12345 record 2 kind 0x71 formula 6 value 100 (0x42c80000) \
                 mode 4 flags 0x0 words 0x0/0x7/0x0 trait+0x20 none trait+0x30 none effect+0x38 34567 not applied \
                 effect+0x40 none"
            )
        );
        // The same record on every pass that refuses it: nothing more.
        assert_eq!(log.refused(&record(12345, 4)), None);
        // The effect it asks for being applied or not is the same record.
        let mut applied = record(12345, 4);
        applied.effects[0].applied = Some(true);
        assert_eq!(log.refused(&applied), None);
        // Another mode, another formula, another flag or another condition is another record.
        assert!(log.refused(&record(12345, 0)).is_some());
        assert!(log.refused(&RefusedRecord { formula: 9, ..record(12345, 4) }).is_some());
        assert!(log.refused(&RefusedRecord { flags: 2, ..record(12345, 4) }).is_some());
        let mut by_trait = record(12345, 4);
        by_trait.traits[1] = Condition { present: true, word: None, applied: None };
        let line = log.refused(&by_trait).unwrap();
        assert!(line.ends_with("trait+0x20 none trait+0x30 unread effect+0x38 34567 not applied effect+0x40 none"), "{line}");
        // Five so far. Up to 32 different ones, and then none.
        let logged = (0..100).filter(|effect| log.refused(&record(*effect, 1)).is_some()).count();
        assert_eq!(logged, MAX_RECORDS - 5);
        assert_eq!(log.refused(&record(999, 2)), None);
    }

    #[test]
    fn a_cycle_logs_both_readers_and_the_record_only_when_magic_find_refused_one() {
        let start = Instant::now();
        let mut log = ReaderLog::new();
        let slots = BagSlots { capacity: 414, occupied: 313, free: 101, bag_slots: 16, bags: 16 };
        let bag_cost = (6012, 55, Some(Duration::from_micros(310)));
        let lines = log.cycle(start, None, BagCoverage::Read(slots), bag_cost, read(), SPENT, &trace());
        assert_eq!(
            lines,
            [
                "reader: Bags: read, first pass; 313 used of 414, 101 free; bytes 6012/16384, reads 55, pass 310 us",
                "reader: Magic Find: read, first pass; total 363 = luck 300 + pushed 13 + buffs 50, boon no; \
                 bytes 20112/262144, reads 201, pass 1432 us; table 91/256, pushed 4, buffs 91, definitions 47, most records 5",
            ]
        );
        // Nothing changes: nothing is logged, however many cycles.
        for second in 1..60 {
            let at = start + Duration::from_secs(second);
            assert!(log.cycle(at, None, BagCoverage::Read(slots), bag_cost, read(), SPENT, &trace()).is_empty());
        }
        // Magic Find refuses a record: its line, and the record's, once.
        let refused = Trace { refused: Some(record(12345, 4)), buffs: 92, count: 92, ..trace() };
        let unsupported = MagicFindCoverage::Unavailable(Uncovered::Unsupported);
        let at = start + Duration::from_secs(60);
        let lines = log.cycle(at, None, BagCoverage::Read(slots), bag_cost, unsupported, SPENT, &refused);
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0],
            "reader: Magic Find: Unsupported, after read for 60 passes (60 s); bytes 20112/262144, reads 201, \
             pass 1432 us; table 92/256, pushed 4, buffs 92, definitions 47, most records 5"
        );
        assert!(lines[1].starts_with("reader: Magic Find record refused: effect 12345 record 2 "));
        assert!(log.cycle(at, None, BagCoverage::Read(slots), bag_cost, unsupported, SPENT, &refused).is_empty());
        // A record left in a trace by a pass that ended otherwise is not logged.
        let other = Trace { refused: Some(record(777, 1)), ..trace() };
        let bounds = MagicFindCoverage::Unavailable(Uncovered::Bounds);
        let lines = log.cycle(at, None, BagCoverage::Read(slots), bag_cost, bounds, SPENT, &other);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("reader: Magic Find: Bounds, after Unsupported for 2 passes (0 s); "));
        // A capture that fails reaches neither reader, and says how it ended.
        let none = (0, 0, None);
        let lines = log.cycle(at, Some("Changed"), BagCoverage::NotRead, none, MagicFindCoverage::NotRead, none, &Trace::default());
        assert_eq!(
            lines,
            [
                "reader: Bags: not read, after read for 63 passes (60 s); capture Changed",
                "reader: Magic Find: not read, after Bounds for 1 passes (0 s); capture Changed",
            ]
        );
        assert_eq!(log.bags.tally(), "Bags passes: 63 read; 1 not read");
        assert_eq!(log.magic_find.tally(), "Magic Find passes: 60 read; 1 not read; 1 Bounds; 2 Unsupported");
    }

    #[test]
    fn the_counters_of_both_readers_are_logged_every_ten_minutes_from_the_first_cycle() {
        let start = Instant::now();
        let mut log = ReaderLog::new();
        let slots = BagSlots { capacity: 414, occupied: 313, free: 101, bag_slots: 16, bags: 16 };
        let bag_cost = (6012, 55, Some(Duration::from_micros(310)));
        let mut cycle = |at: Instant| log.cycle(at, None, BagCoverage::Read(slots), bag_cost, read(), SPENT, &trace());
        assert_eq!(cycle(start).len(), 2, "the first cycle writes its two first-pass lines and no counters");
        assert!(cycle(start + Duration::from_secs(599)).is_empty());
        assert_eq!(
            cycle(start + Duration::from_secs(600)),
            ["reader: tally: Bags passes: 3 read; 0 not read; Magic Find passes: 3 read; 0 not read"]
        );
        // The next is due a whole interval later.
        assert!(cycle(start + Duration::from_secs(1199)).is_empty());
        assert_eq!(cycle(start + Duration::from_secs(1200)).len(), 1);
    }

    #[test]
    fn a_deadline_line_says_what_the_pass_took_and_a_pass_that_did_not_run_says_so() {
        let slow = (40000, 300, Some(Duration::from_micros(250_117)));
        let unavailable = MagicFindCoverage::Unavailable(Uncovered::Deadline);
        assert_eq!(
            magic_find_detail(unavailable, slow, &Trace { capacity: 512, count: 301, buffs: 120, definitions: 80, ..Trace::default() }, None),
            "bytes 40000/262144, reads 300, pass 250117 us; table 301/512, pushed 0, buffs 120, definitions 80, most records 0"
        );
        let bags = BagCoverage::Unavailable(Uncovered::Deadline);
        assert_eq!(bags_detail(bags, (900, 12, None), None), "bytes 900/16384, reads 12, pass not run");
    }
}

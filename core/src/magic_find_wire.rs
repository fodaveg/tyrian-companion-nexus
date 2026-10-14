//! The optional `mf1` feed: the Magic Find the reader computes in the game, buffs included (the
//! figure the Labyrinth panel paints), sent to the plugin so its «En vivo» panel can show it next
//! to the account's.
//!
//! Negotiated like `farm1` and `price2`, the other way round: the plugin announces `mf_cap`
//! (`{"v":3,"type":"mf_cap","nonce":…,"tag":"mf1"}`) and only then does this addon send
//! `mf_state` lines on that connection. A plugin that never announces it (0.6.58 and earlier)
//! never gets one: its parser closes the connection on a sequenced type it does not know.
//!
//! What travels is one whole number or nothing: `mf` is the reader's total rounded to the
//! nearest point, or `null` when the last cycle could not cover it (never a partial sum, never a
//! zero that was not read), and `ageMs` is how old that cycle was when the line was written. No
//! character, account, map or addend goes with it. The feed rides the captures the `live1`
//! channel already takes: a line goes out when the figure changes, again every [`REFRESH`] while
//! it holds, and never more than one per [`MIN_GAP`].

use std::time::{Duration, Instant};

use crate::magic_find::MagicFindCoverage;

/// The only tag this addon announces Magic Find under.
pub const MAGIC_FIND_TAG: &str = "mf1";
/// The largest total that travels; a reading above it is no reading. The plugin bounds `mf` the
/// same way. A real total is a few hundred points.
pub const MAX_TOTAL: u32 = 100_000;
/// The oldest `ageMs` a line may carry. Nothing older than [`HOLD`] is ever reported as a figure.
pub const MAX_AGE_MS: u64 = 600_000;
/// The least time between two `mf_state` lines of one connection: a figure that flickers
/// between a value and no coverage every cycle costs one line per this, not one per second.
pub const MIN_GAP: Duration = Duration::from_secs(5);
/// An unchanged figure is sent again this often, so the plugin can tell it is still being read
/// (it lets one go after two minutes without a line).
pub const REFRESH: Duration = Duration::from_secs(60);
/// A figure no capture has confirmed for this long is no longer the game's: the channel stopped
/// sampling (a conflict, an unsupported build, a long loading screen), and the next line says
/// `null`.
pub const HOLD: Duration = Duration::from_secs(30);

/// The total a cycle's coverage puts on the wire: the reader's figure rounded to the nearest
/// whole point, or `None` when it has none. A figure that is not finite, is negative or is above
/// [`MAX_TOTAL`] is `None` too: nothing is clamped into a number the game did not show.
///
/// Rounded with a cast, as `panel::points` does, so the Windows DLL imports no rounding function.
pub fn wire_total(coverage: &MagicFindCoverage) -> Option<u32> {
    let MagicFindCoverage::Read(read) = coverage else { return None };
    let total = f64::from(read.total);
    if !total.is_finite() || total < 0.0 || total > f64::from(MAX_TOTAL) {
        return None;
    }
    let rounded = (total + 0.5) as u32;
    (rounded <= MAX_TOTAL).then_some(rounded)
}

/// One line the feed owes the plugin: the figure (or `None`) and how old its cycle is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Due {
    pub total: Option<u32>,
    pub age_ms: u64,
}

/// What one connection knows and has said about Magic Find. Lives in the session, so a new
/// connection starts with nothing said and waits for its own `mf_cap`.
#[derive(Debug, Default, Clone)]
pub struct Feed {
    enabled: bool,
    /// The latest thing a cycle said, and the instant of that cycle.
    reading: Option<(Option<u32>, Instant)>,
    /// The last figure put on the wire, and when.
    sent: Option<(Option<u32>, Instant)>,
}

impl Feed {
    pub fn new() -> Self {
        Self::default()
    }

    /// The plugin announced `mf1` on this connection. Idempotent.
    pub fn enable(&mut self) {
        self.enabled = true;
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// A capture whose context held from start to end: what its Magic Find pass returned, at
    /// the instant of that capture.
    pub fn observe(&mut self, coverage: &MagicFindCoverage, at: Instant) {
        self.reading = Some((wire_total(coverage), at));
    }

    /// The game left gameplay for character select: whatever comes next is another character's,
    /// so the figure stops being one now.
    pub fn absent(&mut self, at: Instant) {
        self.reading = Some((None, at));
    }

    /// The line to send now, if one is owed, and marks it sent. Nothing before `mf_cap`, nothing
    /// to retract before a figure was sent, at most one per [`MIN_GAP`].
    pub fn due(&mut self, now: Instant) -> Option<Due> {
        if !self.enabled {
            return None;
        }
        let (total, at) = match self.reading? {
            (Some(_), at) if now.saturating_duration_since(at) > HOLD => (None, now),
            reading => reading,
        };
        match self.sent {
            Some((_, sent_at)) if now.saturating_duration_since(sent_at) < MIN_GAP => return None,
            Some((before, sent_at)) if before == total => {
                if total.is_none() || now.saturating_duration_since(sent_at) < REFRESH {
                    return None;
                }
            }
            None if total.is_none() => return None,
            _ => {}
        }
        self.sent = Some((total, now));
        let age_ms = (now.saturating_duration_since(at).as_millis() as u64).min(MAX_AGE_MS);
        Some(Due { total, age_ms })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::magic_find::MagicFind;
    use crate::passive::Uncovered;

    fn read(total: f32) -> MagicFindCoverage {
        MagicFindCoverage::Read(MagicFind { total, luck: 300, pushed: 0.0, buffs: total - 300.0, boon: false })
    }

    #[test]
    fn the_total_is_rounded_to_the_nearest_point_and_anything_else_is_none() {
        assert_eq!(wire_total(&read(333.0)), Some(333));
        assert_eq!(wire_total(&read(362.5)), Some(363));
        assert_eq!(wire_total(&read(362.4)), Some(362));
        assert_eq!(wire_total(&read(0.0)), Some(0));
        assert_eq!(wire_total(&read(-1.0)), None);
        assert_eq!(wire_total(&read(f32::NAN)), None);
        assert_eq!(wire_total(&read(f32::INFINITY)), None);
        assert_eq!(wire_total(&read(100_000.6)), None);
        assert_eq!(wire_total(&MagicFindCoverage::NotRead), None);
        assert_eq!(wire_total(&MagicFindCoverage::Unavailable(Uncovered::Bounds)), None);
    }

    #[test]
    fn nothing_goes_out_before_the_capability() {
        let t0 = Instant::now();
        let mut feed = Feed::new();
        feed.observe(&read(333.0), t0);
        assert_eq!(feed.due(t0), None);
        feed.enable();
        assert_eq!(feed.due(t0 + Duration::from_millis(40)), Some(Due { total: Some(333), age_ms: 40 }));
    }

    #[test]
    fn an_uncovered_first_cycle_says_nothing_and_a_later_loss_says_null_once() {
        let t0 = Instant::now();
        let mut feed = Feed::new();
        feed.enable();
        feed.observe(&MagicFindCoverage::Unavailable(Uncovered::Unsupported), t0);
        assert_eq!(feed.due(t0), None, "nothing was sent, nothing to retract");
        feed.observe(&read(333.0), t0 + Duration::from_secs(1));
        assert_eq!(feed.due(t0 + Duration::from_secs(1)).unwrap().total, Some(333));
        feed.observe(&MagicFindCoverage::Unavailable(Uncovered::Bounds), t0 + Duration::from_secs(7));
        assert_eq!(feed.due(t0 + Duration::from_secs(7)), Some(Due { total: None, age_ms: 0 }));
        feed.observe(&MagicFindCoverage::Unavailable(Uncovered::Bounds), t0 + Duration::from_secs(20));
        assert_eq!(feed.due(t0 + Duration::from_secs(80)), None, "a null is not repeated");
    }

    #[test]
    fn a_change_waits_for_the_gap_and_an_unchanged_figure_is_refreshed_every_minute() {
        let t0 = Instant::now();
        let mut feed = Feed::new();
        feed.enable();
        feed.observe(&read(333.0), t0);
        assert!(feed.due(t0).is_some());
        feed.observe(&read(363.0), t0 + Duration::from_secs(1));
        assert_eq!(feed.due(t0 + Duration::from_secs(1)), None, "inside the gap");
        feed.observe(&read(363.0), t0 + Duration::from_secs(5));
        assert_eq!(feed.due(t0 + Duration::from_secs(5)).unwrap().total, Some(363));
        for second in 6..65 {
            feed.observe(&read(363.0), t0 + Duration::from_secs(second));
            assert_eq!(feed.due(t0 + Duration::from_secs(second)), None, "unchanged at {second} s");
        }
        feed.observe(&read(363.0), t0 + Duration::from_secs(65));
        assert_eq!(feed.due(t0 + Duration::from_secs(65)).unwrap().total, Some(363));
    }

    #[test]
    fn a_figure_no_capture_confirms_for_the_hold_turns_into_null() {
        let t0 = Instant::now();
        let mut feed = Feed::new();
        feed.enable();
        feed.observe(&read(333.0), t0);
        assert!(feed.due(t0).is_some());
        assert_eq!(feed.due(t0 + HOLD), None);
        assert_eq!(feed.due(t0 + HOLD + Duration::from_millis(1)), Some(Due { total: None, age_ms: 0 }));
    }

    #[test]
    fn character_select_says_null_at_once() {
        let t0 = Instant::now();
        let mut feed = Feed::new();
        feed.enable();
        feed.observe(&read(333.0), t0);
        assert!(feed.due(t0).is_some());
        feed.absent(t0 + Duration::from_secs(6));
        assert_eq!(feed.due(t0 + Duration::from_secs(6)), Some(Due { total: None, age_ms: 0 }));
    }
}

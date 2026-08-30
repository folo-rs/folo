//! Wall-clock reading for session timestamps.
//!
//! Session identity and liveness never consult the wall clock, because a clock
//! that moves cannot decide whether a process is alive. The clock is read only
//! to record when a session started and to age that timestamp for display.
//! Ref: docs/implementation.md, "Session age".

use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch.
///
/// A clock set before the epoch reads as the epoch, and one set implausibly far
/// ahead saturates. Neither is reported as a failure: the value feeds a display
/// column, so an unusable clock is worth a meaningless age rather than a
/// command that refuses to run.
#[must_use]
pub(crate) fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    /// 2020-01-01T00:00:00Z. Any machine running these tests is past it.
    const WELL_BEFORE_NOW_MS: u64 = 1_577_836_800_000;

    /// 2100-01-01T00:00:00Z. Any machine running these tests is short of it.
    const WELL_AFTER_NOW_MS: u64 = 4_102_444_800_000;

    #[test]
    #[cfg_attr(miri, ignore)] // Reads the host clock, which Miri's isolation refuses.
    fn the_clock_reads_a_plausible_present() {
        let now = unix_now_ms();
        assert!(now > WELL_BEFORE_NOW_MS, "{now} is not a present-day clock");
        assert!(now < WELL_AFTER_NOW_MS, "{now} is not a present-day clock");
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Reads the host clock, which Miri's isolation refuses.
    fn the_clock_does_not_run_backwards_between_reads() {
        let first = unix_now_ms();
        let second = unix_now_ms();
        assert!(second >= first);
    }
}

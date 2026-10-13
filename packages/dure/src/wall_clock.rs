//! Wall-clock reading for session timestamps.
//!
//! Session identity and liveness never consult the wall clock, because a clock
//! that moves cannot decide whether a process is alive. The clock is read only
//! to record when a session started and to age that timestamp for display.
//! Ref: docs/implementation.md, "Session age".

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch.
// Reading the host clock is an external boundary, not a deterministic unit-test input.
// Conversion and injected-clock routing remain mutation-tested.
#[cfg_attr(test, mutants::skip)]
#[must_use]
pub(crate) fn unix_now_ms() -> u64 {
    unix_ms_at(SystemTime::now())
}

/// Milliseconds since the Unix epoch at `time`.
///
/// A clock set before the epoch reads as the epoch. That is not reported as a
/// failure: the value feeds a display column, so an unusable clock is worth a
/// meaningless age rather than a command that refuses to run.
#[must_use]
fn unix_ms_at(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, since_epoch_ms)
}

/// Converts an elapsed duration to whole milliseconds.
///
/// A duration too large to express saturates, for the same reason: an age is a
/// display value and no command fails over one.
#[must_use]
fn since_epoch_ms(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn whole_milliseconds_are_what_survive_the_conversion() {
        assert_eq!(since_epoch_ms(Duration::ZERO), 0);
        assert_eq!(since_epoch_ms(Duration::from_millis(1_500)), 1_500);
        // Sub-millisecond precision is below what an age column shows.
        assert_eq!(since_epoch_ms(Duration::from_micros(1_999)), 1);
    }

    #[test]
    fn a_post_epoch_system_time_reaches_the_millisecond_conversion() {
        // Cross a whole-second boundary with a fractional millisecond to distinguish
        // seconds, milliseconds and rounding without consulting the host clock.
        let time = UNIX_EPOCH
            .checked_add(Duration::from_micros(1_500_999))
            .unwrap();
        assert_eq!(unix_ms_at(time), 1_500);
        assert_eq!(unix_ms_at(UNIX_EPOCH), 0);
    }

    #[test]
    fn a_duration_too_large_to_express_saturates() {
        assert_eq!(since_epoch_ms(Duration::MAX), u64::MAX);
    }

    #[test]
    fn a_clock_set_before_the_epoch_reads_as_the_epoch() {
        let before = UNIX_EPOCH.checked_sub(Duration::from_secs(1)).unwrap();
        assert_eq!(unix_ms_at(before), 0);
    }
}

/// Justification for `.expect()` on mutex lock: we guarantee that we never
/// panic while holding any of our mutexes, so they can never be poisoned.
pub(crate) const NEVER_POISONED: &str = "we never panic while holding this lock";

/// Justification for `.expect()` when narrowing an event count from `u64` to `usize`: every event
/// occupies at least one byte, so there can never be more live events than addressable bytes.
pub(crate) const EVENT_COUNT_FITS_IN_USIZE: &str =
    "live events cannot outnumber the addressable bytes of memory";

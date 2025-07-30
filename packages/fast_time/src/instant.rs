use std::time::Duration;

use crate::Clock;

/// An instant returned by the fast-time clock.
///
/// Call `elapsed()` or `saturating_duration_since()` to calculate elapsed time.
///
/// You may convert the instant into an `std::time::Instant` to perform arithmetic and
/// for interoperability purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instant {
    inner: std::time::Instant,
}

impl Instant {
    #[must_use]
    pub fn elapsed(&self, clock: &Clock) -> Duration {
        clock.now().saturating_duration_since(*self)
    }

    #[must_use]
    pub fn saturating_duration_since(&self, earlier: Instant) -> Duration {
        self.inner.saturating_duration_since(earlier.inner)
    }
}

impl From<std::time::Instant> for Instant {
    fn from(inner: std::time::Instant) -> Self {
        Self { inner }
    }
}

impl From<Instant> for std::time::Instant {
    fn from(instant: Instant) -> Self {
        instant.inner
    }
}

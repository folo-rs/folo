use std::time::Duration;

use crate::Clock;

/// An instant returned by the fast-time clock.
///
/// Represents a specific point in time captured from a [`Clock`]. Use this to measure
/// elapsed time or to compare with other instants from the same clock.
///
/// You may convert the instant into an [`std::time::Instant`] for arithmetic operations
/// and interoperability with standard library code.
///
/// # Examples
///
/// Basic elapsed time measurement:
///
/// ```rust
/// use std::time::Duration;
///
/// use fast_time::Clock;
///
/// let clock = Clock::new();
/// let start = clock.now();
///
/// // Simulate some work
/// std::thread::sleep(Duration::from_millis(10));
///
/// let elapsed = start.elapsed(&clock);
/// // Note: fast_time prioritizes efficiency over precision
/// assert!(elapsed <= Duration::from_secs(1)); // Generous upper bound
/// ```
///
/// Comparing instants:
///
/// ```rust
/// use fast_time::Clock;
///
/// let clock = Clock::new();
/// let instant1 = clock.now();
/// let instant2 = clock.now();
///
/// // instant2 should be later than instant1
/// assert!(instant2 >= instant1);
///
/// let duration = instant2.saturating_duration_since(instant1);
/// println!("Time between captures: {:?}", duration);
/// ```
///
/// Converting to standard library types:
///
/// ```rust
/// use std::time::Instant as StdInstant;
///
/// use fast_time::Clock;
///
/// let clock = Clock::new();
/// let fast_instant = clock.now();
///
/// // Convert to std::time::Instant
/// let std_instant: StdInstant = fast_instant.into();
///
/// // Convert back
/// let converted_back: fast_time::Instant = std_instant.into();
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Instant {
    inner: std::time::Instant,
}

impl Instant {
    /// Calculates the elapsed time since this instant using the provided clock.
    ///
    /// This method captures a new timestamp from the clock and calculates the duration
    /// that has passed since this instant was created.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::time::Duration;
    ///
    /// use fast_time::Clock;
    ///
    /// let clock = Clock::new();
    /// let start = clock.now();
    ///
    /// // Simulate some work
    /// std::thread::sleep(Duration::from_millis(5));
    ///
    /// let elapsed = start.elapsed(&clock);
    /// // Note: fast_time prioritizes efficiency over precision
    /// assert!(elapsed <= Duration::from_secs(1)); // Generous upper bound
    /// ```
    #[must_use]
    pub fn elapsed(&self, clock: &Clock) -> Duration {
        clock.now().saturating_duration_since(*self)
    }

    /// Calculates the duration since an earlier instant.
    ///
    /// Returns the amount of time that passed between the `earlier` instant and this instant.
    /// If the earlier instant is actually later than this instant, returns a duration of zero
    /// (saturating behavior).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::time::Duration;
    ///
    /// use fast_time::Clock;
    ///
    /// let clock = Clock::new();
    /// let start = clock.now();
    ///
    /// // Simulate some work
    /// std::thread::sleep(Duration::from_millis(8));
    ///
    /// let end = clock.now();
    /// let duration = end.saturating_duration_since(start);
    /// // Note: fast_time prioritizes efficiency over precision
    /// assert!(duration <= Duration::from_secs(1)); // Generous upper bound
    /// ```
    ///
    /// Saturating behavior with reversed order:
    ///
    /// ```rust
    /// use std::time::Duration;
    ///
    /// use fast_time::Clock;
    ///
    /// let clock = Clock::new();
    /// let instant1 = clock.now();
    /// let instant2 = clock.now();
    ///
    /// // instant1 is earlier, so this returns zero
    /// let duration = instant1.saturating_duration_since(instant2);
    /// assert_eq!(duration, Duration::ZERO);
    /// ```
    #[must_use]
    pub fn saturating_duration_since(&self, earlier: Self) -> Duration {
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

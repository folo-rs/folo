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

#[cfg(test)]
mod tests {
    use mockall::Sequence;

    use super::*;
    use crate::pal::{MockPlatform, MockTimeSource};

    #[test]
    fn saturating_duration_since_can_math() {
        use std::time::Duration;

        // Create synthetic test data using arbitrary durations added to a base instant
        // Note: In real code, we would use Clock::now(), but for testing we create arbitrary instants
        let base_instant = std::time::Instant::now();
        let instant1 = Instant::from(base_instant);
        let instant2 = Instant::from(base_instant + Duration::from_secs(10));
        let instant3 = Instant::from(base_instant + Duration::from_secs(50));

        // Test normal case: later instant minus earlier instant
        let duration_1_to_2 = instant2.saturating_duration_since(instant1);
        assert_eq!(duration_1_to_2, Duration::from_secs(10));

        let duration_1_to_3 = instant3.saturating_duration_since(instant1);
        assert_eq!(duration_1_to_3, Duration::from_secs(50));

        let duration_2_to_3 = instant3.saturating_duration_since(instant2);
        assert_eq!(duration_2_to_3, Duration::from_secs(40));

        // Test saturating behavior: earlier instant minus later instant should return zero
        let duration_reverse_1 = instant1.saturating_duration_since(instant2);
        assert_eq!(duration_reverse_1, Duration::ZERO);

        let duration_reverse_2 = instant1.saturating_duration_since(instant3);
        assert_eq!(duration_reverse_2, Duration::ZERO);

        let duration_reverse_3 = instant2.saturating_duration_since(instant3);
        assert_eq!(duration_reverse_3, Duration::ZERO);

        // Test same instant
        let duration_same = instant1.saturating_duration_since(instant1);
        assert_eq!(duration_same, Duration::ZERO);
    }

    #[test]
    fn duration_since_can_math() {
        let a = std::time::Instant::now();
        let b = a.checked_add(Duration::from_millis(100)).unwrap();

        let mut time_source = MockTimeSource::new();

        let mut seq = Sequence::new();

        time_source
            .expect_now()
            .once()
            .in_sequence(&mut seq)
            .return_once(move || a);

        time_source
            .expect_now()
            .once()
            .in_sequence(&mut seq)
            .return_once(move || b);

        let mut platform = MockPlatform::new();

        platform
            .expect_new_time_source()
            .once()
            .return_once(move || time_source);

        let clock = Clock::from_pal(platform.into());

        let instant1 = clock.now();
        let instant2 = clock.now();

        // instant1 is earlier, so this returns zero
        let duration = instant1.saturating_duration_since(instant2);
        assert_eq!(duration, Duration::ZERO);

        let duration = instant2.saturating_duration_since(instant1);
        assert_eq!(duration, Duration::from_millis(100));
    }

    #[test]
    fn elapsed_can_math() {
        let a = std::time::Instant::now();
        let b = a.checked_add(Duration::from_millis(100)).unwrap();

        let mut time_source = MockTimeSource::new();

        let mut seq = Sequence::new();

        time_source
            .expect_now()
            .once()
            .in_sequence(&mut seq)
            .return_once(move || a);

        time_source
            .expect_now()
            .once()
            .in_sequence(&mut seq)
            .return_once(move || b);

        let mut platform = MockPlatform::new();

        platform
            .expect_new_time_source()
            .once()
            .return_once(move || time_source);

        let clock = Clock::from_pal(platform.into());

        let instant1 = clock.now();

        let duration = instant1.elapsed(&clock);
        assert_eq!(duration, Duration::from_millis(100));
    }
}

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
/// let mut clock = Clock::new();
/// let start = clock.now();
///
/// // Simulate some work
/// std::thread::sleep(Duration::from_millis(10));
///
/// let elapsed = start.elapsed(&mut clock);
/// // Note: fast_time prioritizes efficiency over precision
/// assert!(elapsed <= Duration::from_secs(1)); // Generous upper bound
/// ```
///
/// Comparing instants:
///
/// ```rust
/// use fast_time::Clock;
///
/// let mut clock = Clock::new();
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
/// let mut clock = Clock::new();
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
    /// let mut clock = Clock::new();
    /// let start = clock.now();
    ///
    /// // Simulate some work
    /// std::thread::sleep(Duration::from_millis(5));
    ///
    /// let elapsed = start.elapsed(&mut clock);
    /// // Note: fast_time prioritizes efficiency over precision
    /// assert!(elapsed <= Duration::from_secs(1)); // Generous upper bound
    /// ```
    #[must_use]
    pub fn elapsed(&self, clock: &mut Clock) -> Duration {
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
    /// let mut clock = Clock::new();
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
    /// let mut clock = Clock::new();
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

    /// Calculates the duration since an earlier instant.
    ///
    /// This is an alias for [`saturating_duration_since`](Self::saturating_duration_since)
    /// to maintain compatibility with `std::time::Instant`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::time::Duration;
    ///
    /// use fast_time::Clock;
    ///
    /// let mut clock = Clock::new();
    /// let start = clock.now();
    /// let end = clock.now();
    ///
    /// let duration = end.duration_since(start);
    /// ```
    #[must_use]
    pub fn duration_since(&self, earlier: Self) -> Duration {
        self.saturating_duration_since(earlier)
    }

    /// Calculates the duration since an earlier instant, returning `None` if the earlier
    /// instant is actually later than this instant.
    ///
    /// Unlike [`saturating_duration_since`](Self::saturating_duration_since), this method
    /// returns `None` instead of zero when the earlier instant is later.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::time::Duration;
    ///
    /// use fast_time::Clock;
    ///
    /// let mut clock = Clock::new();
    /// let start = clock.now();
    ///
    /// // Simulate some work to ensure time passes
    /// std::thread::sleep(Duration::from_millis(10));
    ///
    /// let end = clock.now();
    ///
    /// // Normal case: end should be later than start
    /// match end.checked_duration_since(start) {
    ///     Some(duration) => println!("Elapsed: {:?}", duration),
    ///     None => println!("Time went backwards"),
    /// }
    ///
    /// // Test with the same instant - should return Some(Duration::ZERO)
    /// let same_instant = clock.now();
    /// assert_eq!(same_instant.checked_duration_since(same_instant), Some(Duration::ZERO));
    /// ```
    #[must_use]
    pub fn checked_duration_since(&self, earlier: Self) -> Option<Duration> {
        self.inner.checked_duration_since(earlier.inner)
    }

    /// Adds a duration to this instant, returning `None` if overflow occurred.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::time::Duration;
    ///
    /// use fast_time::Clock;
    ///
    /// let mut clock = Clock::new();
    /// let instant = clock.now();
    ///
    /// let later = instant.checked_add(Duration::from_secs(5)).unwrap();
    /// assert!(later > instant);
    ///
    /// // This would overflow
    /// assert!(instant.checked_add(Duration::MAX).is_none());
    /// ```
    #[must_use]
    pub fn checked_add(&self, duration: Duration) -> Option<Self> {
        self.inner.checked_add(duration).map(Self::from)
    }

    /// Subtracts a duration from this instant, returning `None` if underflow occurred.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::time::Duration;
    ///
    /// use fast_time::Clock;
    ///
    /// let mut clock = Clock::new();
    /// let instant = clock.now();
    ///
    /// let earlier = instant.checked_sub(Duration::from_secs(5)).unwrap();
    /// assert!(earlier < instant);
    ///
    /// // This would underflow
    /// assert!(instant.checked_sub(Duration::MAX).is_none());
    /// ```
    #[must_use]
    pub fn checked_sub(&self, duration: Duration) -> Option<Self> {
        self.inner.checked_sub(duration).map(Self::from)
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

        let mut clock = Clock::from_pal(platform.into());

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

        let mut clock = Clock::from_pal(platform.into());

        let instant1 = clock.now();

        let duration = instant1.elapsed(&mut clock);
        assert_eq!(duration, Duration::from_millis(100));
    }

    #[test]
    fn checked_duration_since_can_math() {
        use std::time::Duration;

        // Create synthetic test data using arbitrary durations added to a base instant
        let base_instant = std::time::Instant::now();
        let instant1 = Instant::from(base_instant);
        let instant2 = Instant::from(base_instant + Duration::from_secs(10));

        // Test normal case: later instant minus earlier instant
        let duration = instant2.checked_duration_since(instant1);
        assert_eq!(duration, Some(Duration::from_secs(10)));

        // Test reverse case: earlier instant minus later instant should return None
        let duration_reverse = instant1.checked_duration_since(instant2);
        assert_eq!(duration_reverse, None);

        // Test same instant
        let duration_same = instant1.checked_duration_since(instant1);
        assert_eq!(duration_same, Some(Duration::ZERO));
    }

    #[test]
    fn duration_since_is_alias_for_saturating_duration_since() {
        use std::time::Duration;

        let base_instant = std::time::Instant::now();
        let instant1 = Instant::from(base_instant);
        let instant2 = Instant::from(base_instant + Duration::from_secs(5));

        // Both methods should return the same result
        let duration_saturating = instant2.saturating_duration_since(instant1);
        let duration_alias = instant2.duration_since(instant1);
        assert_eq!(duration_saturating, duration_alias);

        // Test reverse case: both should return zero
        let duration_saturating_reverse = instant1.saturating_duration_since(instant2);
        let duration_alias_reverse = instant1.duration_since(instant2);
        assert_eq!(duration_saturating_reverse, duration_alias_reverse);
        assert_eq!(duration_alias_reverse, Duration::ZERO);
    }

    #[test]
    fn checked_add_can_math() {
        use std::time::Duration;

        let base_instant = std::time::Instant::now();
        let instant = Instant::from(base_instant);

        // Test normal addition
        let duration = Duration::from_secs(5);
        let later_instant = instant.checked_add(duration).unwrap();
        let expected = Instant::from(base_instant + duration);
        assert_eq!(later_instant, expected);

        // Test that the result is indeed later
        assert!(later_instant > instant);

        // Test zero addition
        let same_instant = instant.checked_add(Duration::ZERO).unwrap();
        assert_eq!(same_instant, instant);
    }

    #[test]
    fn checked_sub_can_math() {
        use std::time::Duration;

        let base_instant = std::time::Instant::now();
        let duration = Duration::from_secs(10);
        let later_instant = base_instant + duration;
        let instant = Instant::from(later_instant);

        // Test normal subtraction
        let earlier_instant = instant.checked_sub(Duration::from_secs(5)).unwrap();
        let expected = Instant::from(later_instant.checked_sub(Duration::from_secs(5)).unwrap());
        assert_eq!(earlier_instant, expected);

        // Test that the result is indeed earlier
        assert!(earlier_instant < instant);

        // Test zero subtraction
        let same_instant = instant.checked_sub(Duration::ZERO).unwrap();
        assert_eq!(same_instant, instant);

        // Test subtracting the full duration
        let base_again = instant.checked_sub(duration).unwrap();
        let expected_base = Instant::from(base_instant);
        assert_eq!(base_again, expected_base);
    }
}

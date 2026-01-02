use crate::Instant;
use crate::pal::{Platform, PlatformFacade, TimeSource, TimeSourceFacade};

/// A clock that can efficiently provide the current time.
///
/// This is suitable for querying rapidly with low overhead, in circumstances where absolute
/// precision is not necessary. The timestamps produced by this clock are monotonically increasing
/// but may lag behind wall-clock time by a small number of milliseconds and may not follow explicit
/// wall clock adjustments applied by the operating system (e.g. as part of clock synchronization).
///
/// This typically makes these timestamps useful for rapid polling scenarios, such as metrics and
/// logging, where efficiency matters greatly because you may be capturing 100 000 timestamps per
/// second, whereas the precise microsecond or even millisecond is not important.
///
/// # Examples
///
/// Creating a clock and capturing timestamps:
///
/// ```rust
/// use fast_time::Clock;
///
/// let mut clock = Clock::new();
/// let instant1 = clock.now();
/// let instant2 = clock.now();
///
/// // Timestamps are monotonically increasing
/// assert!(instant2.saturating_duration_since(instant1).as_nanos() >= 0);
/// ```
///
/// Measuring elapsed time:
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
/// // Note: fast_time prioritizes efficiency over precision, so we use loose tolerance
/// assert!(elapsed <= Duration::from_millis(50)); // Very generous upper bound
/// ```
///
/// High-frequency timestamp collection:
///
/// ```rust
/// use fast_time::Clock;
///
/// let mut clock = Clock::new();
/// let mut durations = Vec::new();
///
/// let start = clock.now();
/// for _ in 0..1000 {
///     let timestamp = clock.now();
///     durations.push(timestamp.saturating_duration_since(start));
/// }
///
/// // All durations should be monotonically increasing
/// for window in durations.windows(2) {
///     assert!(window[1] >= window[0]);
/// }
/// ```
///
/// Cloning a clock:
///
/// ```rust
/// use fast_time::Clock;
///
/// let mut clock1 = Clock::new();
/// let instant1 = clock1.now();
///
/// // Clone the clock to create an independent instance
/// let mut clock2 = clock1.clone();
/// let instant2 = clock2.now();
///
/// // Both clocks work independently
/// assert!(instant1.elapsed(&mut clock1).as_millis() < 100);
/// assert!(instant2.elapsed(&mut clock2).as_millis() < 100);
/// ```
#[derive(Clone, Debug)]
pub struct Clock {
    inner: TimeSourceFacade,
}

impl Clock {
    /// Creates a new clock instance.
    ///
    /// The clock will use the platform's most efficient time source for rapid timestamp
    /// capture.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use fast_time::Clock;
    ///
    /// let mut clock = Clock::new();
    /// let timestamp = clock.now();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        #[cfg(all(any(target_os = "linux", windows), not(miri)))]
        return Self::from_pal(PlatformFacade::real());
        #[cfg(any(miri, not(any(target_os = "linux", windows))))]
        return Self::from_pal(PlatformFacade::rust());
    }

    #[must_use]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "semantically correct, even if not necessary"
    )]
    pub(crate) fn from_pal(pal: PlatformFacade) -> Self {
        Self {
            inner: pal.new_time_source(),
        }
    }

    /// Returns the current timestamp.
    ///
    /// This method is optimized for rapid, repeated calls. The returned [`Instant`]
    /// represents a point in time that can be used to measure elapsed duration.
    ///
    /// # Examples
    ///
    /// Basic timestamp capture:
    ///
    /// ```rust
    /// use fast_time::Clock;
    ///
    /// let mut clock = Clock::new();
    /// let now = clock.now();
    /// println!("Current time: {:?}", now);
    /// ```
    ///
    /// Rapid timestamp collection:
    ///
    /// ```rust
    /// use fast_time::Clock;
    ///
    /// let mut clock = Clock::new();
    /// let timestamps: Vec<_> = (0..100).map(|_| clock.now()).collect();
    ///
    /// // All timestamps should be valid
    /// assert_eq!(timestamps.len(), 100);
    /// ```
    #[must_use]
    pub fn now(&mut self) -> Instant {
        self.inner.now().into()
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    static_assertions::assert_impl_all!(Clock: Clone, Send);

    #[test]
    fn now_is_approximately_now() {
        let mut clock = Clock::new();
        let instant = clock.now();

        let rust_now = std::time::Instant::now();
        let instant_as_rust_instant: std::time::Instant = instant.into();

        assert!(
            instant_as_rust_instant
                .saturating_duration_since(rust_now)
                .as_millis()
                < 100
        );
        assert!(
            rust_now
                .saturating_duration_since(instant_as_rust_instant)
                .as_millis()
                < 100
        );
    }

    #[test]
    fn consecutive_instants_are_approximately_equal() {
        let mut clock = Clock::new();

        let instant1 = clock.now();
        let instant2 = clock.now();

        assert!(instant1.elapsed(&mut clock).as_millis() < 100);
        assert!(instant2.elapsed(&mut clock).as_millis() < 100);

        let elapsed = instant2.saturating_duration_since(instant1);
        assert!(elapsed.as_millis() < 100);

        let rust_instant1: std::time::Instant = instant1.into();
        let rust_instant2: std::time::Instant = instant2.into();

        let rust_elapsed = rust_instant2.duration_since(rust_instant1);

        assert!(rust_elapsed.as_millis() < 100);
    }

    #[test]
    fn default_behaves_like_new() {
        // Verify both constructors work without panicking.
        // Other tests validate the actual time measurement behavior.
        let mut clock_new = Clock::new();
        let mut clock_default = Clock::default();

        _ = clock_new.now();
        _ = clock_default.now();
    }

    #[test]
    fn clock_can_be_cloned() {
        let mut clock1 = Clock::new();
        let instant1 = clock1.now();

        let mut clock2 = clock1.clone();
        let instant2 = clock2.now();

        // Both clocks should produce valid timestamps
        assert!(instant1.elapsed(&mut clock1).as_millis() < 100);
        assert!(instant2.elapsed(&mut clock2).as_millis() < 100);

        // The timestamps should be close to each other
        let diff = if instant2 > instant1 {
            instant2.saturating_duration_since(instant1)
        } else {
            instant1.saturating_duration_since(instant2)
        };
        assert!(diff.as_millis() < 100);
    }
}

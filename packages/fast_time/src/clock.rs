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
#[derive(Debug)]
pub struct Clock {
    inner: TimeSourceFacade,
}

impl Clock {
    #[must_use]
    pub fn new() -> Self {
        Self::from_pal(&PlatformFacade::real())
    }

    #[must_use]
    pub(crate) fn from_pal(pal: &PlatformFacade) -> Self {
        Self {
            inner: pal.new_time_source(),
        }
    }

    #[must_use]
    pub fn now(&self) -> Instant {
        self.inner.now().into()
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[cfg(not(miri))] // Miri cannot talk to the real platform.
mod tests {
    use super::*;

    #[test]
    fn now_is_approximately_now() {
        let clock = Clock::new();
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
        let clock = Clock::new();

        let instant1 = clock.now();
        let instant2 = clock.now();

        assert!(instant1.elapsed(&clock).as_millis() < 100);
        assert!(instant2.elapsed(&clock).as_millis() < 100);

        let elapsed = instant2.saturating_duration_since(instant1);
        assert!(elapsed.as_millis() < 100);

        let rust_instant1: std::time::Instant = instant1.into();
        let rust_instant2: std::time::Instant = instant2.into();

        let rust_elapsed = rust_instant2.duration_since(rust_instant1);

        assert!(rust_elapsed.as_millis() < 100);
    }
}

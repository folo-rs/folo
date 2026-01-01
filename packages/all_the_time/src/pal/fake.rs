//! Fake platform implementation for testing.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::pal::abstractions::Platform;

/// Internal state for the fake platform that can be shared between clones.
#[derive(Debug)]
#[cfg(test)]
struct FakePlatformState {
    thread_time: Duration,
    process_time: Duration,
}

/// Fake implementation of the platform abstraction for testing.
///
/// This implementation allows tests to control the processor time values
/// instead of relying on actual system calls. Multiple clones of the same
/// `FakePlatform` share the same underlying time state, allowing tests to
/// modify time values after platform creation to simulate time progression.
#[derive(Clone, Debug)]
#[cfg(test)]
pub(crate) struct FakePlatform {
    state: Arc<Mutex<FakePlatformState>>,
}

#[cfg(test)]
impl FakePlatform {
    /// Creates a new fake platform with zero time values.
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(FakePlatformState {
                thread_time: Duration::ZERO,
                process_time: Duration::ZERO,
            })),
        }
    }

    /// Sets the thread processor time value.
    ///
    /// This affects all clones of this platform, allowing tests to simulate
    /// time progression during measurement.
    pub(crate) fn set_thread_time(&self, time: Duration) {
        self.state
            .lock()
            .expect("FakePlatform state lock should not be poisoned")
            .thread_time = time;
    }

    /// Sets the process processor time value.
    ///
    /// This affects all clones of this platform, allowing tests to simulate
    /// time progression during measurement.
    pub(crate) fn set_process_time(&self, time: Duration) {
        self.state
            .lock()
            .expect("FakePlatform state lock should not be poisoned")
            .process_time = time;
    }
}

#[cfg(test)]
impl Platform for FakePlatform {
    fn thread_time(&self) -> Duration {
        self.state
            .lock()
            .expect("FakePlatform state lock should not be poisoned")
            .thread_time
    }

    fn process_time(&self) -> Duration {
        self.state
            .lock()
            .expect("FakePlatform state lock should not be poisoned")
            .process_time
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn initializes_with_zero_time() {
        let platform = FakePlatform::new();
        assert_eq!(platform.thread_time(), Duration::ZERO);
        assert_eq!(platform.process_time(), Duration::ZERO);
    }

    #[test]
    fn sets_thread_time() {
        let platform = FakePlatform::new();
        platform.set_thread_time(Duration::from_millis(150));

        assert_eq!(platform.thread_time(), Duration::from_millis(150));
    }

    #[test]
    fn sets_process_time() {
        let platform = FakePlatform::new();
        platform.set_process_time(Duration::from_millis(250));

        assert_eq!(platform.process_time(), Duration::from_millis(250));
    }

    #[test]
    fn shared_state_between_clones() {
        let platform1 = FakePlatform::new();
        let platform2 = platform1.clone();

        // Setting time on one clone affects the other
        platform1.set_thread_time(Duration::from_millis(100));
        assert_eq!(platform2.thread_time(), Duration::from_millis(100));

        platform2.set_process_time(Duration::from_millis(200));
        assert_eq!(platform1.process_time(), Duration::from_millis(200));
    }
}

//! Fake platform implementation for testing.

use std::time::Duration;

use crate::pal::abstractions::Platform;

/// Fake implementation of the platform abstraction for testing.
///
/// This implementation allows tests to control the CPU time values
/// instead of relying on actual system calls.
#[derive(Clone, Debug)]
#[cfg(test)]
pub(crate) struct FakePlatform {
    thread_time: Duration,
    process_time: Duration,
}

#[cfg(test)]
impl FakePlatform {
    /// Creates a new fake platform with zero time values.
    pub(crate) fn new() -> Self {
        Self {
            thread_time: Duration::ZERO,
            process_time: Duration::ZERO,
        }
    }

    /// Sets the thread CPU time value.
    pub(crate) fn set_thread_time(&mut self, time: Duration) {
        self.thread_time = time;
    }

    /// Sets the process CPU time value.
    pub(crate) fn set_process_time(&mut self, time: Duration) {
        self.process_time = time;
    }
}

#[cfg(test)]
impl Platform for FakePlatform {
    fn thread_time(&self) -> Duration {
        self.thread_time
    }

    fn process_time(&self) -> Duration {
        self.process_time
    }
}

#[cfg(test)]
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
        let mut platform = FakePlatform::new();
        platform.set_thread_time(Duration::from_millis(150));

        assert_eq!(platform.thread_time(), Duration::from_millis(150));
    }

    #[test]
    fn sets_process_time() {
        let mut platform = FakePlatform::new();
        platform.set_process_time(Duration::from_millis(250));

        assert_eq!(platform.process_time(), Duration::from_millis(250));
    }
}

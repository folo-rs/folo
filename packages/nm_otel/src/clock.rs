use std::future::Future;
use std::time::Duration;

/// A clock abstraction that can be mocked for testing.
///
/// This allows us to control time progression in tests without using real delays.
pub trait Clock: Send {
    /// Sleep for the specified duration.
    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + Send;
}

/// Real clock implementation using `tokio::time`.
#[non_exhaustive]
#[derive(Debug, Clone, Default)]
pub struct RealClock;

impl Clock for RealClock {
    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}

#[cfg(test)]
pub(crate) mod mock {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Mock clock for testing that tracks how many times sleep was called
    /// and what durations were requested.
    #[derive(Debug, Clone)]
    pub(crate) struct MockClock {
        sleep_count: Arc<AtomicU64>,
        total_sleep_duration: Arc<AtomicU64>,
    }

    impl MockClock {
        /// Creates a new mock clock.
        #[expect(dead_code, reason = "will be used in future integration tests")]
        pub(crate) fn new() -> Self {
            Self {
                sleep_count: Arc::new(AtomicU64::new(0)),
                total_sleep_duration: Arc::new(AtomicU64::new(0)),
            }
        }

        /// Returns the number of times sleep was called.
        #[expect(dead_code, reason = "will be used in future integration tests")]
        pub(crate) fn sleep_count(&self) -> u64 {
            self.sleep_count.load(Ordering::Relaxed)
        }

        /// Returns the total sleep duration in milliseconds.
        #[expect(dead_code, reason = "will be used in future integration tests")]
        pub(crate) fn total_sleep_duration_ms(&self) -> u64 {
            self.total_sleep_duration.load(Ordering::Relaxed)
        }
    }

    impl Clock for MockClock {
        async fn sleep(&self, duration: Duration) {
            self.sleep_count.fetch_add(1, Ordering::Relaxed);

            #[expect(
                clippy::cast_possible_truncation,
                reason = "truncation is acceptable for mock clock tracking"
            )]
            self.total_sleep_duration
                .fetch_add(duration.as_millis() as u64, Ordering::Relaxed);
        }
    }
}

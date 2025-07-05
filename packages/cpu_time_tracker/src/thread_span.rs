//! Thread-specific CPU time tracking spans.

use crate::Operation;
use crate::pal::Platform;
use std::time::Duration;
/// A tracked span of code that tracks thread CPU time between creation and drop.
///
/// This span tracks CPU time consumed by the current thread only.
///
/// # Examples
///
/// ```
/// use cpu_time_tracker::Session;
///
/// let mut session = Session::new();
/// let operation = session.operation("test");
/// {
///     let _span = operation.iterations(1).thread_span();
///     // Perform some CPU-intensive operation
///     let mut sum = 0;
///     for i in 0..1000 {
///         sum += i;
///     }
/// } // Thread CPU time is automatically tracked and recorded here
/// ```
///
/// For benchmarks with many iterations:
///
/// ```
/// use cpu_time_tracker::Session;
///
/// let mut session = Session::new();
/// let operation = session.operation("test");
/// {
///     let _span = operation.iterations(1000).thread_span();
///     for i in 0..1000 {
///         // Perform the operation being benchmarked
///         let mut sum = 0;
///         sum += i;
///     }
/// } // CPU time is measured once and divided by 1000
/// ```
#[derive(Debug)]
pub struct ThreadSpan<'a> {
    operation: &'a mut Operation,
    start_time: Duration,
    iterations: u64,
}

impl<'a> ThreadSpan<'a> {
    /// Creates a new thread span for the given operation and iteration count.
    ///
    /// # Panics
    ///
    /// Panics if `iterations` is zero.
    #[must_use]
    pub(crate) fn new(operation: &'a mut Operation, iterations: u64) -> Self {
        assert!(iterations != 0, "Iterations cannot be zero");

        let start_time = operation.platform().thread_time();

        Self {
            operation,
            start_time,
            iterations,
        }
    }

    /// Calculates the thread CPU time delta since this span was created.
    #[must_use]
    fn to_duration(&self) -> Duration {
        let current_time = self.operation.platform().thread_time();
        let total_duration = current_time.saturating_sub(self.start_time);

        if self.iterations > 1 {
            Duration::from_nanos(
                total_duration
                    .as_nanos()
                    .checked_div(u128::from(self.iterations))
                    .unwrap_or(0)
                    .try_into()
                    .unwrap_or(0),
            )
        } else {
            total_duration
        }
    }
}

impl Drop for ThreadSpan<'_> {
    fn drop(&mut self) {
        let duration = self.to_duration();

        // Add the per-iteration duration, but record the number of iterations
        for _ in 0..self.iterations {
            self.operation.add(duration);
        }
    }
}

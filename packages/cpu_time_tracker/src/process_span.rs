//! Process-wide CPU time tracking spans.

use crate::Operation;
use crate::pal::Platform;
use std::time::Duration;
/// A tracked span of code that tracks process CPU time between creation and drop.
///
/// This span tracks CPU time consumed by the entire process (all threads).
///
/// # Examples
///
/// ```
/// use cpu_time_tracker::Session;
///
/// let mut session = Session::new();
/// let operation = session.operation("test");
/// {
///     let _span = operation.iterations(1).process_span();
///     // Perform some CPU-intensive operation
///     let mut sum = 0;
///     for i in 0..1000 {
///         sum += i;
///     }
/// } // Process CPU time is automatically tracked and recorded here
/// ```
///
/// For benchmarks with many iterations:
///
/// ```
/// use cpu_time_tracker::Session;
///
/// let mut session = Session::new();
/// let operation = session.operation("benchmark");
/// {
///     let _span = operation.iterations(1000).process_span();
///     for i in 0..1000 {
///         // Perform the operation being benchmarked
///         let mut sum = 0;
///         sum += i;
///     }
/// } // CPU time is measured once and divided by 1000
/// ```
#[derive(Debug)]
pub struct ProcessSpan<'a> {
    operation: &'a mut Operation,
    start_time: Duration,
    iterations: u64,
}

impl<'a> ProcessSpan<'a> {
    /// Creates a new process span for the given operation and iteration count.
    ///
    /// # Panics
    ///
    /// Panics if `iterations` is zero.
    #[must_use]
    pub(crate) fn new(operation: &'a mut Operation, iterations: u64) -> Self {
        assert!(iterations != 0, "Iterations cannot be zero");

        let start_time = operation.platform().process_time();

        Self {
            operation,
            start_time,
            iterations,
        }
    }

    /// Calculates the process CPU time delta since this span was created.
    #[must_use]
    fn to_duration(&self) -> Duration {
        let current_time = self.operation.platform().process_time();
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

impl Drop for ProcessSpan<'_> {
    fn drop(&mut self) {
        let duration = self.to_duration();

        // Add the per-iteration duration, but record the number of iterations
        for _ in 0..self.iterations {
            self.operation.add(duration);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Session;

    #[test]
    fn process_span_new() {
        let mut session = Session::new();
        let operation = session.operation("test");
        let span = operation.iterations(5).process_span();
        assert_eq!(span.iterations, 5);
    }

    #[test]
    #[should_panic(expected = "Iterations cannot be zero")]
    fn process_span_new_zero_iterations() {
        let mut session = Session::new();
        let operation = session.operation("test");
        let _span = operation.iterations(0).process_span();
    }

    #[test]
    fn process_span_tracks_time() {
        let mut session = Session::new();
        let operation = session.operation("test");
        {
            let _span = operation.iterations(1).process_span();
            // Perform some work
            let mut sum = 0;
            for i in 0..1000 {
                sum += i;
            }
            std::hint::black_box(sum);
        }

        // Verify that at least one measurement was recorded
        assert!(operation.spans() > 0);
    }
}

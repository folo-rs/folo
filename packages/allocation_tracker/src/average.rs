//! Average memory allocation tracking.

use crate::delta::MemoryDeltaTracker;
use crate::session::AllocationTrackingSession;

/// Calculates average memory allocation per operation across multiple iterations.
///
/// This utility is particularly useful for benchmarking scenarios where you want
/// to understand the average memory footprint of repeated operations.
///
/// # Examples
///
/// ```
/// use allocation_tracker::{AllocationTrackingSession, AverageMemoryDelta};
///
/// let session = AllocationTrackingSession::new().unwrap();
/// let mut average = AverageMemoryDelta::new("string_allocations".to_string());
///
/// // Simulate multiple operations
/// for i in 0..5 {
///     let _contributor = average.contribute(&session);
///     let _data = vec![0; i + 1]; // Allocate different amounts
/// }
///
/// let avg_bytes = average.average();
/// println!("Average allocation: {} bytes per operation", avg_bytes);
/// ```
#[derive(Debug)]
pub struct AverageMemoryDelta {
    name: String,
    total_bytes_allocated: u64,
    iterations: u64,
}

impl AverageMemoryDelta {
    /// Creates a new average memory delta calculator with the given name.
    #[must_use]
    pub fn new(name: String) -> Self {
        Self {
            name,
            total_bytes_allocated: 0,
            iterations: 0,
        }
    }

    /// Returns the name of this measurement.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Adds a memory delta measurement to the average calculation.
    ///
    /// This method is typically called by [`AverageMemoryDeltaContributor`] when it is dropped.
    pub fn add(&mut self, delta: u64) {
        // Never going to overflow u64, so no point doing slower checked arithmetic here.
        self.total_bytes_allocated = self.total_bytes_allocated.wrapping_add(delta);
        self.iterations = self.iterations.wrapping_add(1);
    }

    /// Creates a contributor that will automatically measure and add a memory delta
    /// when it is dropped.
    ///
    /// # Examples
    ///
    /// ```
    /// use allocation_tracker::{AllocationTrackingSession, AverageMemoryDelta};
    ///
    /// let session = AllocationTrackingSession::new().unwrap();
    /// let mut average = AverageMemoryDelta::new("test".to_string());
    /// {
    ///     let _contributor = average.contribute(&session);
    ///     let _data = vec![1, 2, 3]; // This allocation will be measured
    /// } // Contributor is dropped here, measurement is added to average
    /// ```
    pub fn contribute<'a>(
        &'a mut self,
        session: &'a AllocationTrackingSession,
    ) -> AverageMemoryDeltaContributor<'a> {
        AverageMemoryDeltaContributor::new(self, session)
    }

    /// Calculates the average bytes allocated per iteration.
    ///
    /// Returns 0 if no iterations have been recorded.
    #[expect(clippy::integer_division, reason = "we accept loss of precision")]
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "division by zero excluded via if-else"
    )]
    #[must_use]
    pub fn average(&self) -> u64 {
        if self.iterations == 0 {
            0
        } else {
            self.total_bytes_allocated / self.iterations
        }
    }

    /// Returns the total number of iterations recorded.
    #[must_use]
    pub fn iterations(&self) -> u64 {
        self.iterations
    }

    /// Returns the total bytes allocated across all iterations.
    #[must_use]
    pub fn total_bytes_allocated(&self) -> u64 {
        self.total_bytes_allocated
    }
}

/// A contributor to average memory delta calculation that automatically measures
/// memory allocation when dropped.
///
/// This type is created by [`AverageMemoryDelta::contribute()`] and should not be
/// constructed directly. When dropped, it automatically measures the memory delta
/// since its creation and adds it to the associated [`AverageMemoryDelta`].
///
/// # Examples
///
/// ```
/// use allocation_tracker::{AllocationTrackingSession, AverageMemoryDelta};
///
/// let session = AllocationTrackingSession::new().unwrap();
/// let mut average = AverageMemoryDelta::new("test".to_string());
/// {
///     let _contributor = average.contribute(&session);
///     // Perform some operation that allocates memory
///     let _data = String::from("Hello, world!");
/// } // Memory delta is automatically measured and recorded here
/// ```
#[derive(Debug)]
pub struct AverageMemoryDeltaContributor<'a> {
    average_memory_delta: &'a mut AverageMemoryDelta,
    memory_delta_tracker: MemoryDeltaTracker<'a>,
}

impl<'a> AverageMemoryDeltaContributor<'a> {
    pub(crate) fn new(
        average_memory_delta: &'a mut AverageMemoryDelta,
        session: &'a AllocationTrackingSession,
    ) -> Self {
        let memory_delta_tracker = MemoryDeltaTracker::new(session);

        Self {
            average_memory_delta,
            memory_delta_tracker,
        }
    }
}

impl Drop for AverageMemoryDeltaContributor<'_> {
    fn drop(&mut self) {
        let delta = self.memory_delta_tracker.to_delta();
        self.average_memory_delta.add(delta);
    }
}

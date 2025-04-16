use std::fmt::Debug;

use nonempty::NonEmpty;

use crate::{MemoryRegionId, ProcessorId, pal::ProcessorFacade};

pub(crate) trait Platform: Debug + Send + Sync + 'static {
    /// Returns all processors available to the current process.
    ///
    /// The returned set will exclude processors that are not active or are forbidden from
    /// being used due to resource constraints enforced by the operating system.
    ///
    /// The returned collection of processors is sorted by the processor ID, ascending.
    #[must_use]
    fn get_all_processors(&self) -> NonEmpty<ProcessorFacade>;

    fn pin_current_thread_to<P>(&self, processors: &NonEmpty<P>)
    where
        P: AsRef<ProcessorFacade>;

    /// Gets the ID of the processor currently executing this thread.
    #[must_use]
    fn current_processor_id(&self) -> ProcessorId;

    /// Gets the IDs of all processors that the current thread is allowed to execute on.
    ///
    /// Note: this may be a superset of `get_all_processors()` because it may include processors
    /// that our process is in fact forbidden to use due to resource constraints enforced by
    /// the operating system. The filtering to only see what we are allowed to use is performed
    /// by `get_all_processors()` but not by this function.
    #[must_use]
    fn current_thread_processors(&self) -> NonEmpty<ProcessorId>;

    /// Gets the maximum (inclusive) processor ID of any processor that could possibly
    /// be present on the system (including processors that are not currently active).
    ///
    /// The value also covers processors that are not available to the current process
    /// due to resource constraints enforced by the operating system.
    ///
    /// This value is a constant and will not change over time.
    #[must_use]
    fn max_processor_id(&self) -> ProcessorId;

    /// Gets the maximum (inclusive) memory region ID of any memory region that could possibly
    /// be present on the system (including memory regions that are not currently active).
    ///
    /// The value also covers memory regions that are not available to the current process
    /// due to resource constraints enforced by the operating system.
    ///
    /// This value is a constant and will not change over time.
    #[must_use]
    fn max_memory_region_id(&self) -> MemoryRegionId;

    /// Gets the maximum processor time that the process is allowed to use per second of real time,
    /// in seconds of processor time. This must be a positive number and will never be greater than
    /// the number of processors available to the current process.
    #[must_use]
    fn max_processor_time(&self) -> f64;

    /// Gets the total number of active processors on the system, including ones that are not
    /// necessarily available to the current process (if any such are known).
    ///
    /// We generally avoid relying on system-scoped data like this but because some platform APIs
    /// speak in terms of system-scoped data, we occasionally need to access such values.
    #[must_use]
    fn active_processor_count(&self) -> ProcessorId;
}

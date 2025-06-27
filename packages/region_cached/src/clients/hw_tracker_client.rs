use many_cpus::{HardwareTracker, MemoryRegionId};

#[cfg_attr(test, mockall::automock)]
pub(crate) trait HardwareTrackerClient {
    fn current_memory_region_id(&self) -> MemoryRegionId;
    fn is_thread_memory_region_pinned(&self) -> bool;
}

#[derive(Debug)]
pub(crate) struct HardwareTrackerClientImpl;

impl HardwareTrackerClient for HardwareTrackerClientImpl {
    #[cfg_attr(test, mutants::skip)] // Trivial fn, tested on lower levels - skip mutating.
    fn current_memory_region_id(&self) -> MemoryRegionId {
        HardwareTracker::current_memory_region_id()
    }

    #[cfg_attr(test, mutants::skip)] // Trivial fn, tested on lower levels - skip mutating.
    fn is_thread_memory_region_pinned(&self) -> bool {
        HardwareTracker::is_thread_memory_region_pinned()
    }
}

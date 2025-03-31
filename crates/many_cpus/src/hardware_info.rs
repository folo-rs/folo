use std::marker::PhantomData;

use crate::{
    MemoryRegionId, ProcessorId,
    pal::{BUILD_TARGET_PLATFORM, Platform},
};

/// Reports non-changing information about the system hardware.
///
/// To inspect information that may change over time, use [`HardwareTracker`][1].
///
/// Functions exposed by this type represent the system hardware and are not limited by the
/// current system or process configuration. That is, this type will still count processors and
/// memory regions that are currently inactive (e.g. some processors are physically disconnected)
/// or are not available to this process (e.g. because of cgroups policy).
///
/// # Example
///
/// ```
/// use many_cpus::HardwareInfo;
///
/// let max_processor_id = HardwareInfo::max_processor_id();
/// println!("The maximum processor ID is: {max_processor_id}");
/// ```
///
/// [1]: crate::HardwareTracker
#[derive(Debug)]
pub struct HardwareInfo {
    _no_ctor: PhantomData<()>,
}

impl HardwareInfo {
    /// Gets the maximum (inclusive) processor ID of any processor that could possibly
    /// be present on the system at any point in time.
    ///
    /// This includes processors that are not currently active and processors that are active
    /// but not available to the current process.
    #[cfg_attr(test, mutants::skip)] // Trivial layer, we only test the underlying logic.
    #[inline]
    pub fn max_processor_id() -> ProcessorId {
        BUILD_TARGET_PLATFORM.max_processor_id()
    }

    /// Gets the maximum (inclusive) memory region ID of any memory region that could possibly
    /// be present on the system at any point in time.
    ///
    /// This includes memory regions that are not currently active and memory regions that
    /// are active but not available to the current process.
    #[cfg_attr(test, mutants::skip)] // Trivial layer, we only test the underlying logic.
    #[inline]
    pub fn max_memory_region_id() -> MemoryRegionId {
        BUILD_TARGET_PLATFORM.max_memory_region_id()
    }

    /// Gets the maximum number of processors that could possibly be present on the system
    /// at any point in time.
    ///
    /// This includes processors that are not currently active and processors that are active
    /// but not available to the current process.
    #[cfg_attr(test, mutants::skip)] // Trivial layer, we only test the underlying logic.
    #[inline]
    pub fn max_processor_count() -> usize {
        (Self::max_processor_id() as usize)
            .checked_add(1)
            .expect("overflow when counting processors - this can only result from a critical error in the PAL")
    }

    /// Gets the maximum number of memory regions that could possibly be present on the system
    /// at any point in time.
    ///
    /// This includes memory regions that are not currently active and memory regions that
    /// are active but not available to the current process.
    #[cfg_attr(test, mutants::skip)] // Trivial layer, we only test the underlying logic.
    #[inline]
    pub fn max_memory_region_count() -> usize {
        (Self::max_memory_region_id() as usize)
            .checked_add(1)
            .expect("overflow when counting memory regions - this can only result from a critical error in the PAL")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(miri))] // Real platform is not supported under Miri.
    #[test]
    fn count_is_id_plus_one_real() {
        assert_eq!(
            HardwareInfo::max_processor_count(),
            HardwareInfo::max_processor_id() as usize + 1
        );
        assert_eq!(
            HardwareInfo::max_memory_region_count(),
            HardwareInfo::max_memory_region_id() as usize + 1
        );
    }
}

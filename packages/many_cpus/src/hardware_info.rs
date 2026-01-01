use std::marker::PhantomData;

use crate::pal::{BUILD_TARGET_PLATFORM, Platform};
use crate::{MemoryRegionId, ProcessorId};

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
    #[must_use]
    pub fn max_processor_id() -> ProcessorId {
        BUILD_TARGET_PLATFORM.max_processor_id()
    }

    /// Gets the maximum (inclusive) memory region ID of any memory region that could possibly
    /// be present on the system at any point in time.
    ///
    /// This includes memory regions that are not currently active and memory regions that
    /// are active but not available to the current process.
    ///
    /// # Example
    ///
    /// ```
    /// use many_cpus::HardwareInfo;
    ///
    /// let max_memory_region_id = HardwareInfo::max_memory_region_id();
    /// let max_memory_region_count = HardwareInfo::max_memory_region_count();
    ///
    /// println!("Memory region IDs range from 0 to {max_memory_region_id}");
    /// println!("System can have up to {max_memory_region_count} memory regions");
    ///
    /// // Useful for creating arrays indexed by memory region ID
    /// let mut data_per_region = vec![0; max_memory_region_count];
    /// println!(
    ///     "Created array with {} slots for memory region data",
    ///     data_per_region.len()
    /// );
    /// ```
    #[cfg_attr(test, mutants::skip)] // Trivial layer, we only test the underlying logic.
    #[inline]
    #[must_use]
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
    #[must_use]
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
    ///
    /// # Example
    ///
    /// ```
    /// use many_cpus::HardwareInfo;
    ///
    /// let max_regions = HardwareInfo::max_memory_region_count();
    /// let max_processors = HardwareInfo::max_processor_count();
    ///
    /// println!("System architecture supports:");
    /// println!("  Up to {max_processors} processors");
    /// println!("  Up to {max_regions} memory regions");
    ///
    /// let avg_processors_per_region = max_processors as f64 / max_regions as f64;
    /// println!(
    ///     "  Average of {:.1} processors per memory region",
    ///     avg_processors_per_region
    /// );
    /// ```
    #[cfg_attr(test, mutants::skip)] // Trivial layer, we only test the underlying logic.
    #[inline]
    #[must_use]
    pub fn max_memory_region_count() -> usize {
        (Self::max_memory_region_id() as usize)
            .checked_add(1)
            .expect("overflow when counting memory regions - this can only result from a critical error in the PAL")
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
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

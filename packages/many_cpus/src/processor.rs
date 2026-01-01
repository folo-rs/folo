use std::fmt::{Debug, Display};
use std::hash::{Hash, Hasher};

use derive_more::derive::AsRef;

use crate::pal::{AbstractProcessor, ProcessorFacade};
use crate::{EfficiencyClass, MemoryRegionId, ProcessorId};

/// A processor present on the system and available to the current process.
#[derive(AsRef, Clone)]
pub struct Processor {
    #[as_ref]
    inner: ProcessorFacade,
}

impl Processor {
    #[must_use]
    pub(crate) fn new(inner: ProcessorFacade) -> Self {
        Self { inner }
    }

    /// The unique numeric ID of the processor, matching the ID used by operating system tools.
    ///
    /// You can obtain the upper bound via [`HardwareInfo::max_processor_id()`][1].
    ///
    /// # Example
    ///
    /// ```
    /// use many_cpus::ProcessorSet;
    ///
    /// let processors = ProcessorSet::default();
    ///
    /// for processor in processors.processors() {
    ///     let id = processor.id();
    ///     println!("Default processor set includes processor {id}");
    /// }
    /// ```
    ///
    /// [1]: crate::HardwareInfo::max_processor_id
    #[cfg_attr(test, mutants::skip)] // Trivial delegation, do not waste time on mutation.
    #[inline]
    #[must_use]
    pub fn id(&self) -> ProcessorId {
        self.inner.id()
    }

    /// The unique numeric ID of the memory region, matching the ID used by operating system tools.
    ///
    /// You can obtain the upper bound via [`HardwareInfo::max_memory_region_id()`][1].
    ///
    /// [1]: crate::HardwareInfo::max_memory_region_id
    #[cfg_attr(test, mutants::skip)] // Trivial delegation, do not waste time on mutation.
    #[inline]
    #[must_use]
    pub fn memory_region_id(&self) -> MemoryRegionId {
        self.inner.memory_region_id()
    }

    /// The [efficiency class][EfficiencyClass] of the processor.
    ///
    /// This is a relative measure - the fastest processors on any given system are always
    /// considered performance processors, while any that are slower are considered efficiency
    /// processors.
    ///
    /// # Example
    ///
    /// ```
    /// use many_cpus::{EfficiencyClass, ProcessorSet};
    ///
    /// let processors = ProcessorSet::default();
    /// let mut performance_count = 0;
    /// let mut efficiency_count = 0;
    ///
    /// for processor in processors.processors() {
    ///     match processor.efficiency_class() {
    ///         EfficiencyClass::Performance => {
    ///             performance_count += 1;
    ///             println!("Processor {} is a performance processor", processor.id());
    ///         }
    ///         EfficiencyClass::Efficiency => {
    ///             efficiency_count += 1;
    ///             println!("Processor {} is an efficiency processor", processor.id());
    ///         }
    ///     }
    /// }
    ///
    /// println!(
    ///     "System has {} performance and {} efficiency processors",
    ///     performance_count, efficiency_count
    /// );
    /// ```
    #[cfg_attr(test, mutants::skip)] // Trivial delegation, do not waste time on mutation.
    #[inline]
    #[must_use]
    pub fn efficiency_class(&self) -> EfficiencyClass {
        self.inner.efficiency_class()
    }
}

impl PartialEq for Processor {
    #[cfg_attr(test, mutants::skip)] // Trivial delegation, do not waste time on mutation.
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

impl Eq for Processor {}

impl Hash for Processor {
    #[cfg_attr(test, mutants::skip)] // Trivial delegation, do not waste time on mutation.
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.inner.hash(state);
    }
}

impl Display for Processor {
    #[cfg_attr(test, mutants::skip)] // Trivial delegation, do not waste time on mutation.
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.inner, f)
    }
}

impl Debug for Processor {
    #[cfg_attr(test, mutants::skip)] // Trivial delegation, do not waste time on mutation.
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Debug::fmt(&self.inner, f)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::hash::DefaultHasher;

    use super::*;
    use crate::pal::FakeProcessor;

    #[test]
    fn smoke_test() {
        let pal_processor = FakeProcessor {
            index: 42,
            memory_region: 13,
            efficiency_class: EfficiencyClass::Efficiency,
        };

        let processor = Processor::new(pal_processor.into());

        // Getters appear to get the expected values.
        assert_eq!(processor.id(), 42);
        assert_eq!(processor.memory_region_id(), 13);
        assert_eq!(processor.efficiency_class(), EfficiencyClass::Efficiency);

        // A clone is a legit clone.
        let processor_clone = processor.clone();
        assert_eq!(processor, processor_clone);

        // Clones have the same hash.
        let mut hasher1 = DefaultHasher::new();
        processor.hash(&mut hasher1);
        let hash1 = hasher1.finish();

        let mut hasher2 = DefaultHasher::new();
        processor_clone.hash(&mut hasher2);
        let hash2 = hasher2.finish();

        assert_eq!(hash1, hash2);

        // Display writes something (anything - as long as it writes something and does not panic).
        let displayed = format!("{processor}");
        assert!(!displayed.is_empty());

        // Debug writes something (anything - as long as it writes something and does not panic).
        let debugged = format!("{processor:?}");
        assert!(!debugged.is_empty());
    }
}

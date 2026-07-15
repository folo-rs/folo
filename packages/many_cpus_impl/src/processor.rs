use std::fmt::{Debug, Display};
use std::hash::{Hash, Hasher};

use derive_more::derive::AsRef;

use crate::pal::{AbstractProcessor, ProcessorFacade};
use crate::{EfficiencyClass, MemoryRegionId, ProcessorId, RelativeSpeed};

/// Model string reported for a processor whose model the operating system does not
/// disclose. The value is metadata for identification only, so a fixed placeholder
/// spares every caller from inventing its own substitute.
const UNKNOWN_MODEL: &str = "unknown";

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
    /// You can obtain the upper bound via [`SystemHardware::max_processor_id()`][1].
    ///
    /// # Example
    ///
    /// ```
    /// use many_cpus::SystemHardware;
    ///
    /// let processors = SystemHardware::current().processors();
    ///
    /// for processor in processors {
    ///     let id = processor.id();
    ///     println!("Default processor set includes processor {id}");
    /// }
    /// ```
    ///
    /// [1]: crate::SystemHardware::max_processor_id
    #[cfg_attr(test, mutants::skip)] // Trivial delegation, do not waste time on mutation.
    #[inline]
    #[must_use]
    pub fn id(&self) -> ProcessorId {
        self.inner.id()
    }

    /// The unique numeric ID of the memory region, matching the ID used by operating system tools.
    ///
    /// You can obtain the upper bound via [`SystemHardware::max_memory_region_id()`][1].
    ///
    /// [1]: crate::SystemHardware::max_memory_region_id
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
    /// use many_cpus::{EfficiencyClass, SystemHardware};
    ///
    /// let processors = SystemHardware::current().processors();
    /// let mut performance_count = 0;
    /// let mut efficiency_count = 0;
    ///
    /// for processor in processors {
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
    ///     "System has {performance_count} performance and {efficiency_count} efficiency processors",
    /// );
    /// ```
    #[cfg_attr(test, mutants::skip)] // Trivial delegation, do not waste time on mutation.
    #[inline]
    #[must_use]
    pub fn efficiency_class(&self) -> EfficiencyClass {
        self.inner.efficiency_class()
    }

    /// A relative indicator of the processor's nominal speed.
    ///
    /// This refines [`efficiency_class()`][Self::efficiency_class] with a finer-grained value that
    /// helps distinguish processors of different underlying types on the same system. Processors
    /// of the same type report the same value, so it does not uniquely identify a processor - use
    /// [`id()`][Self::id] for that.
    ///
    /// The value is only meaningful within a single system and is **not** comparable across
    /// systems or operating systems. See [`RelativeSpeed`] for details.
    ///
    /// # Example
    ///
    /// ```
    /// use many_cpus::SystemHardware;
    ///
    /// let processors = SystemHardware::current().processors();
    ///
    /// for processor in processors {
    ///     println!(
    ///         "Processor {} has relative speed {}",
    ///         processor.id(),
    ///         processor.relative_speed(),
    ///     );
    /// }
    /// ```
    #[cfg_attr(test, mutants::skip)] // Trivial delegation, do not waste time on mutation.
    #[inline]
    #[must_use]
    pub fn relative_speed(&self) -> RelativeSpeed {
        self.inner.relative_speed()
    }

    /// The best-effort model string of the processor.
    ///
    /// This is intended for *identification* and diagnostics, not comparison. The exact text is
    /// platform-specific:
    ///
    /// * On Linux it is the `model name` field of `/proc/cpuinfo`.
    /// * On Windows it is the `ProcessorNameString` the firmware records in the registry.
    ///
    /// When the operating system does not disclose a model (for example many ARM Linux systems
    /// report no `model name`, and the fallback platform and Miri never report one), this returns
    /// `"unknown"` so callers always have a string to key on.
    ///
    /// Because the value originates from different sources per platform, never compare a model read
    /// on one operating system to one read on another.
    ///
    /// # Example
    ///
    /// ```
    /// use many_cpus::SystemHardware;
    ///
    /// let processors = SystemHardware::current().processors();
    ///
    /// for processor in processors {
    ///     println!("Processor {} is a {}", processor.id(), processor.model());
    /// }
    /// ```
    #[inline]
    #[must_use]
    pub fn model(&self) -> &str {
        self.inner.model().unwrap_or(UNKNOWN_MODEL)
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

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
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
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::assert_impl_all;

    use super::*;
    use crate::fake::platform::FakeProcessor;

    assert_impl_all!(Processor: UnwindSafe, RefUnwindSafe);

    #[test]
    fn smoke_test() {
        let pal_processor = FakeProcessor::new(
            42,
            13,
            EfficiencyClass::Efficiency,
            RelativeSpeed::from_raw(3600),
            Some(std::sync::Arc::from("Example Model")),
        );

        let processor = Processor::new(pal_processor.into());

        // Getters appear to get the expected values.
        assert_eq!(processor.id(), 42);
        assert_eq!(processor.memory_region_id(), 13);
        assert_eq!(processor.efficiency_class(), EfficiencyClass::Efficiency);
        assert_eq!(processor.relative_speed().as_u64(), 3600);
        assert_eq!(processor.model(), "Example Model");

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

    #[test]
    fn model_falls_back_to_unknown_when_absent() {
        let pal_processor = FakeProcessor::new(
            0,
            0,
            EfficiencyClass::Performance,
            RelativeSpeed::from_raw(1),
            None,
        );

        let processor = Processor::new(pal_processor.into());

        assert_eq!(processor.model(), "unknown");
    }
}

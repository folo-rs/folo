//! Builder for configuring fake hardware.

use std::collections::HashSet;
use std::num::NonZero;

use crate::fake::ProcessorBuilder;
use crate::{EfficiencyClass, MemoryRegionId, ProcessorId};

/// A processor with a resolved ID, ready to be used by the backend.
#[derive(Clone, Debug)]
pub(crate) struct ResolvedProcessor {
    pub(crate) id: ProcessorId,
    pub(crate) memory_region_id: MemoryRegionId,
    pub(crate) efficiency_class: EfficiencyClass,
}

/// Builder for configuring fake hardware.
///
/// Fake hardware simulates hardware configurations for testing purposes. You can specify
/// the number of processors, their distribution across memory regions, efficiency classes,
/// and resource quotas.
///
/// # Construction modes
///
/// There are two mutually exclusive ways to configure processors:
///
/// 1. **Quick mode** via [`from_counts()`][Self::from_counts]: Creates a specified number of
///    performance-class processors distributed across memory regions. This mode does not allow
///    adding individual processors via [`processor()`][Self::processor].
///
/// 2. **Custom mode** via [`new()`][Self::new] + [`processor()`][Self::processor]: Allows
///    adding individual processors with custom configurations. This mode does not allow
///    using [`from_counts()`][Self::from_counts].
///
/// Mixing these modes will cause a panic when attempting to consume the builder.
///
/// # Memory region assignment
///
/// When using [`from_counts()`][Self::from_counts], processors are distributed round-robin
/// across memory regions. When using [`processor()`][Self::processor], memory regions default
/// to 0 unless explicitly configured via [`ProcessorBuilder::memory_region()`].
///
/// # Processor ID assignment
///
/// Processor IDs can be assigned automatically or explicitly. When adding processors via
/// [`processor()`][Self::processor], you can optionally specify an explicit ID using
/// [`ProcessorBuilder::id()`].
///
/// Automatic IDs are assigned sequentially, starting from 0 and skipping any IDs that are
/// explicitly assigned. For example, if you have one processor with explicit ID 1, automatic
/// assignment will assign IDs 0, 2, 3, etc. to the remaining processors.
///
/// Duplicate explicit IDs will cause a panic when attempting to consume the builder.
///
/// # Processor ID gaps
///
/// Processor IDs are not required to be sequential. Gaps in the ID sequence are allowed and
/// do not affect functionality. For example, you can configure processors with IDs 0, 2, and 5
/// without any issues. The `max_processor_id()` will reflect the highest configured ID.
///
/// Leaving gaps requires manual ID assignment, as automatic ID assignment will always
/// produce a contiguous sequence of IDs.
///
/// # Example (quick mode)
///
/// ```
/// use many_cpus::SystemHardware;
/// use many_cpus::fake::HardwareBuilder;
/// use new_zealand::nz;
///
/// // 8 performance processors distributed across 2 memory regions.
/// let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(8), nz!(2)));
/// ```
///
/// # Example (custom mode)
///
/// ```
/// use many_cpus::fake::{HardwareBuilder, ProcessorBuilder};
/// use many_cpus::{EfficiencyClass, SystemHardware};
///
/// // Custom processor configuration with mixed automatic and explicit IDs.
/// let hardware = SystemHardware::fake(
///     HardwareBuilder::new()
///         .processor(
///             ProcessorBuilder::new()
///                 .id(0) // Explicit ID.
///                 .memory_region(0)
///                 .efficiency_class(EfficiencyClass::Performance),
///         )
///         .processor(
///             ProcessorBuilder::new()
///                 .id(5) // Gap in IDs is allowed.
///                 .memory_region(0)
///                 .efficiency_class(EfficiencyClass::Efficiency),
///         )
///         .processor(
///             ProcessorBuilder::new() // Automatic ID (will be 1).
///                 .memory_region(1),
///         )
///         .max_processor_time(2.5),
/// );
/// ```
#[derive(Clone, Debug)]
pub struct HardwareBuilder {
    pub(crate) processors: Vec<ProcessorBuilder>,
    pub(crate) max_processor_time: Option<f64>,
    /// If true, this builder was created via `from_counts()` and `processor()` is forbidden.
    from_counts: bool,
}

impl Default for HardwareBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl HardwareBuilder {
    /// Creates a new empty hardware builder in custom mode.
    ///
    /// Use [`processor()`][Self::processor] to add processors individually with custom
    /// configurations. If no processors are added, a default single processor will be used.
    #[must_use]
    pub fn new() -> Self {
        Self {
            processors: Vec::new(),
            max_processor_time: None,
            from_counts: false,
        }
    }

    /// Creates a hardware builder in quick mode with the specified processor and memory
    /// region counts.
    ///
    /// All processors are created with [`EfficiencyClass::Performance`] and are distributed
    /// round-robin across the memory regions.
    ///
    /// This constructor creates the builder in quick mode, which does not allow adding
    /// individual processors via [`processor()`][Self::processor].
    ///
    /// # Panics
    ///
    /// Panics if [`processor()`][Self::processor] is called on a builder created with this
    /// constructor.
    #[must_use]
    pub fn from_counts(
        processor_count: NonZero<usize>,
        memory_region_count: NonZero<usize>,
    ) -> Self {
        let mut processors = Vec::with_capacity(processor_count.get());

        for i in 0..processor_count.get() {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "unrealistic to have more than u32::MAX memory regions"
            )]
            #[expect(clippy::arithmetic_side_effects, reason = "modulo cannot overflow")]
            let region = (i % memory_region_count.get()) as u32;

            processors.push(ProcessorBuilder::new().memory_region(region));
        }

        Self {
            processors,
            max_processor_time: None,
            from_counts: true,
        }
    }

    /// Adds a custom-configured processor.
    ///
    /// Use [`ProcessorBuilder`] to configure the processor's ID, memory region, and
    /// efficiency class. Processor IDs do not need to be sequential - gaps are allowed.
    ///
    /// # Panics
    ///
    /// Panics if this builder was created via [`from_counts()`][Self::from_counts]. These
    /// two modes are mutually exclusive to prevent accidental misconfiguration.
    #[must_use]
    pub fn processor(mut self, proc: ProcessorBuilder) -> Self {
        assert!(
            !self.from_counts,
            "cannot add individual processors to a builder created with from_counts() - \
             use new() instead to add processors manually"
        );
        self.processors.push(proc);
        self
    }

    /// Sets the maximum processor time (resource quota).
    ///
    /// This simulates a container or cgroup resource limit. A value of 2.5 means the process
    /// is allowed to consume up to 2.5 processor-seconds per wall-clock second.
    #[must_use]
    pub fn max_processor_time(mut self, time: f64) -> Self {
        self.max_processor_time = Some(time);
        self
    }

    /// Returns the configured processors with resolved IDs.
    ///
    /// Automatic IDs are assigned sequentially starting from 0, skipping any explicitly
    /// assigned IDs. Duplicate explicit IDs will cause a panic.
    ///
    /// # Panics
    ///
    /// Panics if two or more processors have the same explicit ID.
    pub(crate) fn build_processors(&self) -> Vec<ResolvedProcessor> {
        // First, collect all explicit IDs to detect duplicates and reserve them.
        let mut used_ids: HashSet<ProcessorId> = HashSet::new();

        for p in &self.processors {
            if let Some(id) = p.explicit_id {
                assert!(
                    used_ids.insert(id),
                    "duplicate processor ID {id} - each processor must have a unique ID"
                );
            }
        }

        // Assign automatic IDs, skipping any that are already used.
        let mut next_auto_id: ProcessorId = 0;
        let mut resolved = Vec::with_capacity(self.processors.len());

        for p in &self.processors {
            let id = match p.explicit_id {
                Some(id) => id,
                None => {
                    // Find the next available ID.
                    while used_ids.contains(&next_auto_id) {
                        next_auto_id = next_auto_id
                            .checked_add(1)
                            .expect("too many processors for automatic ID assignment");
                    }
                    let id = next_auto_id;
                    used_ids.insert(id);
                    next_auto_id = next_auto_id
                        .checked_add(1)
                        .expect("too many processors for automatic ID assignment");
                    id
                }
            };

            resolved.push(ResolvedProcessor {
                id,
                memory_region_id: p.memory_region_id,
                efficiency_class: p.efficiency_class,
            });
        }

        resolved
    }

    /// Returns the configured max processor time.
    pub(crate) fn build_max_processor_time(&self) -> Option<f64> {
        self.max_processor_time
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::indexing_slicing, reason = "test code, panics are acceptable")]
mod tests {
    use new_zealand::nz;

    use super::*;

    #[test]
    fn default_equals_new() {
        let default_builder = HardwareBuilder::default();
        let new_builder = HardwareBuilder::new();

        // Both should have no processors and no max_processor_time set.
        assert_eq!(default_builder.processors.len(), new_builder.processors.len());
        assert_eq!(
            default_builder.max_processor_time,
            new_builder.max_processor_time
        );
        assert_eq!(default_builder.from_counts, new_builder.from_counts);
    }

    #[test]
    fn from_counts_creates_correct_count() {
        let builder = HardwareBuilder::from_counts(nz!(4), nz!(1));

        assert_eq!(builder.processors.len(), 4);
        for p in &builder.processors {
            assert_eq!(p.explicit_id, None);
            assert_eq!(p.memory_region_id, 0);
            assert_eq!(p.efficiency_class, EfficiencyClass::Performance);
        }
    }

    #[test]
    fn from_counts_assigns_sequential_ids() {
        let builder = HardwareBuilder::from_counts(nz!(4), nz!(1));
        let resolved = builder.build_processors();

        assert_eq!(resolved.len(), 4);
        for (i, p) in resolved.iter().enumerate() {
            #[expect(clippy::cast_possible_truncation, reason = "test data is small enough")]
            let expected_id = i as ProcessorId;
            assert_eq!(p.id, expected_id);
        }
    }

    #[test]
    fn from_counts_distributes_round_robin() {
        let builder = HardwareBuilder::from_counts(nz!(6), nz!(2));

        // Processors 0, 2, 4 in region 0; processors 1, 3, 5 in region 1.
        assert_eq!(builder.processors[0].memory_region_id, 0);
        assert_eq!(builder.processors[1].memory_region_id, 1);
        assert_eq!(builder.processors[2].memory_region_id, 0);
        assert_eq!(builder.processors[3].memory_region_id, 1);
        assert_eq!(builder.processors[4].memory_region_id, 0);
        assert_eq!(builder.processors[5].memory_region_id, 1);
    }

    #[test]
    #[should_panic]
    fn from_counts_forbids_processor() {
        let builder = HardwareBuilder::from_counts(nz!(4), nz!(1));
        drop(builder.processor(ProcessorBuilder::new()));
    }

    #[test]
    fn explicit_id_is_respected() {
        let builder = HardwareBuilder::new().processor(ProcessorBuilder::new().id(42));
        let resolved = builder.build_processors();

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].id, 42);
    }

    #[test]
    fn auto_ids_skip_explicit_ids() {
        // Add processors: explicit 1, auto, explicit 0, auto.
        // Auto IDs should skip 0 and 1, assigning 2 and 3.
        let builder = HardwareBuilder::new()
            .processor(ProcessorBuilder::new().id(1))
            .processor(ProcessorBuilder::new())
            .processor(ProcessorBuilder::new().id(0))
            .processor(ProcessorBuilder::new());
        let resolved = builder.build_processors();

        assert_eq!(resolved.len(), 4);
        assert_eq!(resolved[0].id, 1); // Explicit.
        assert_eq!(resolved[1].id, 2); // Auto, skipped 0 and 1.
        assert_eq!(resolved[2].id, 0); // Explicit.
        assert_eq!(resolved[3].id, 3); // Auto, next available.
    }

    #[test]
    fn mixed_auto_and_explicit_with_gaps() {
        let builder = HardwareBuilder::new()
            .processor(ProcessorBuilder::new().id(10))
            .processor(ProcessorBuilder::new())
            .processor(ProcessorBuilder::new());
        let resolved = builder.build_processors();

        assert_eq!(resolved.len(), 3);
        assert_eq!(resolved[0].id, 10); // Explicit.
        assert_eq!(resolved[1].id, 0); // Auto.
        assert_eq!(resolved[2].id, 1); // Auto.
    }

    #[test]
    #[should_panic]
    fn duplicate_explicit_ids_panics() {
        let builder = HardwareBuilder::new()
            .processor(ProcessorBuilder::new().id(5))
            .processor(ProcessorBuilder::new().id(5));
        drop(builder.build_processors());
    }
}

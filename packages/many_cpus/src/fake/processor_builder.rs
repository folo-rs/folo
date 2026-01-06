//! Builder for configuring individual fake processors.

use crate::{EfficiencyClass, MemoryRegionId, ProcessorId};

/// Builder for configuring an individual fake processor.
///
/// Each fake processor has an ID, a memory region, and an efficiency class.
/// By default, processors are placed in memory region 0 with [`EfficiencyClass::Performance`].
///
/// Processor IDs are assigned automatically by default. If you need a specific ID,
/// use [`id()`][Self::id] to set it explicitly.
///
/// # Example
///
/// ```
/// use many_cpus::EfficiencyClass;
/// use many_cpus::fake::ProcessorBuilder;
///
/// // Automatic ID assignment (recommended).
/// let processor = ProcessorBuilder::new()
///     .memory_region(1)
///     .efficiency_class(EfficiencyClass::Efficiency);
///
/// // Explicit ID assignment (when needed).
/// let processor_with_id = ProcessorBuilder::new().id(42).memory_region(0);
/// ```
#[derive(Clone, Debug)]
pub struct ProcessorBuilder {
    pub(crate) explicit_id: Option<ProcessorId>,
    pub(crate) memory_region_id: MemoryRegionId,
    pub(crate) efficiency_class: EfficiencyClass,
}

impl Default for ProcessorBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessorBuilder {
    /// Creates a new processor builder with automatic ID assignment.
    ///
    /// The processor is placed in memory region 0 with [`EfficiencyClass::Performance`] by default.
    /// The ID will be automatically assigned when the processor is added to a
    /// [`HardwareBuilder`][super::HardwareBuilder].
    #[must_use]
    pub fn new() -> Self {
        Self {
            explicit_id: None,
            memory_region_id: 0,
            efficiency_class: EfficiencyClass::Performance,
        }
    }

    /// Sets an explicit ID for this processor.
    ///
    /// If not called, the ID will be automatically assigned based on the order of processors
    /// and any explicitly assigned IDs.
    ///
    /// # Panics
    ///
    /// When the processor is added to a [`HardwareBuilder`][super::HardwareBuilder],
    /// it will panic if this ID is already used by another processor.
    #[must_use]
    pub fn id(mut self, id: ProcessorId) -> Self {
        self.explicit_id = Some(id);
        self
    }

    /// Sets the memory region ID for this processor.
    #[must_use]
    pub fn memory_region(mut self, memory_region_id: MemoryRegionId) -> Self {
        self.memory_region_id = memory_region_id;
        self
    }

    /// Sets the efficiency class for this processor.
    #[must_use]
    pub fn efficiency_class(mut self, efficiency_class: EfficiencyClass) -> Self {
        self.efficiency_class = efficiency_class;
        self
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn default_equals_new() {
        let default_builder = ProcessorBuilder::default();
        let new_builder = ProcessorBuilder::new();

        assert_eq!(default_builder.explicit_id, new_builder.explicit_id);
        assert_eq!(default_builder.memory_region_id, new_builder.memory_region_id);
        assert_eq!(default_builder.efficiency_class, new_builder.efficiency_class);
    }

    #[test]
    fn default_values() {
        let builder = ProcessorBuilder::new();

        assert_eq!(builder.explicit_id, None);
        assert_eq!(builder.memory_region_id, 0);
        assert_eq!(builder.efficiency_class, EfficiencyClass::Performance);
    }

    #[test]
    fn explicit_id() {
        let builder = ProcessorBuilder::new().id(5);

        assert_eq!(builder.explicit_id, Some(5));
    }

    #[test]
    fn builder_chaining() {
        let builder = ProcessorBuilder::new()
            .id(3)
            .memory_region(2)
            .efficiency_class(EfficiencyClass::Efficiency);

        assert_eq!(builder.explicit_id, Some(3));
        assert_eq!(builder.memory_region_id, 2);
        assert_eq!(builder.efficiency_class, EfficiencyClass::Efficiency);
    }
}

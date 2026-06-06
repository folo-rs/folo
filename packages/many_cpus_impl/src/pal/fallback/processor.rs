use derive_more::derive::Display;

use crate::pal::AbstractProcessor;
use crate::{EfficiencyClass, MemoryRegionId, ProcessorId};

/// A fallback processor implementation for unsupported platforms.
///
/// This processor always reports itself as being in memory region 0 with Performance efficiency
/// class. The actual hardware topology is not inspected on unsupported platforms.
#[derive(Clone, Copy, Debug, Display, Eq, Hash, PartialEq)]
#[display("ProcessorImpl({id})")]
pub(crate) struct ProcessorImpl {
    id: ProcessorId,
}

impl ProcessorImpl {
    pub(crate) fn new(id: ProcessorId) -> Self {
        Self { id }
    }
}

impl AbstractProcessor for ProcessorImpl {
    fn id(&self) -> ProcessorId {
        self.id
    }

    #[cfg_attr(test, mutants::skip)] // Some mutations are not testable due to simulated nature of this PAL.
    fn memory_region_id(&self) -> MemoryRegionId {
        // All processors are in the same memory region on the fallback platform.
        0
    }

    fn efficiency_class(&self) -> EfficiencyClass {
        // We do not have real topology information, so we assume all processors
        // are performance processors.
        EfficiencyClass::Performance
    }
}

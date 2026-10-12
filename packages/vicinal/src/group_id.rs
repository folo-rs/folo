use std::fmt;

use many_cpus::{MemoryRegionId, Processor, ProcessorId};

/// Carries the sampled destination through allocation, startup, queueing and diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GroupId {
    Processor(ProcessorId),
    MemoryRegion(MemoryRegionId),
}

impl GroupId {
    pub(crate) fn index(self) -> usize {
        match self {
            Self::Processor(id) | Self::MemoryRegion(id) => id as usize,
        }
    }

    pub(crate) fn contains(self, processor: &Processor) -> bool {
        match self {
            Self::Processor(id) => processor.id() == id,
            Self::MemoryRegion(id) => processor.memory_region_id() == id,
        }
    }
}

impl fmt::Display for GroupId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // Preserve the default policy's established thread names.
            Self::Processor(id) => write!(f, "{id}"),
            Self::MemoryRegion(id) => write!(f, "region-{id}"),
        }
    }
}

/// A destination and its submitting CPU, derived from one topology observation.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SubmissionTarget {
    pub(crate) group: GroupId,
    pub(crate) processor: ProcessorId,
}

use std::fmt;

use many_cpus::{MemoryRegionId, Processor, ProcessorId};

/// Carries the sampled destination through allocation, startup, queueing and diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GroupId {
    Processor(ProcessorId),
    MemoryRegion(MemoryRegionId),
    System,
}

impl GroupId {
    pub(crate) fn contains(self, processor: &Processor) -> bool {
        match self {
            Self::Processor(id) => processor.id() == id,
            Self::MemoryRegion(id) => processor.memory_region_id() == id,
            Self::System => true,
        }
    }
}

impl fmt::Display for GroupId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Processor(id) => write!(f, "p{id}"),
            Self::MemoryRegion(id) => write!(f, "r{id}"),
            Self::System => f.write_str("s"),
        }
    }
}

/// A destination and its submitting CPU, derived from one topology observation.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SubmissionTarget {
    pub(crate) group: GroupId,
    pub(crate) processor: ProcessorId,
}

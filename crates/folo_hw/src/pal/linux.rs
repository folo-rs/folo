use std::fmt::Display;

use crate::pal::{
    EfficiencyClass, MemoryRegionIndex, PlatformCommon, ProcessorCommon, ProcessorGlobalIndex,
};

/// A processor present on the system and available to the current process.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Processor {}

impl Display for Processor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "processor (linux)",)
    }
}

impl ProcessorCommon for Processor {
    fn index(&self) -> ProcessorGlobalIndex {
        todo!()
    }

    fn memory_region_index(&self) -> MemoryRegionIndex {
        todo!()
    }

    fn efficiency_class(&self) -> EfficiencyClass {
        todo!()
    }
}

pub(crate) struct Platform;

impl PlatformCommon for Platform {
    type Processor = Processor;

    fn get_all_processors() -> nonempty::NonEmpty<Self::Processor> {
        todo!()
    }

    fn pin_current_thread_to<P>(_processors: &nonempty::NonEmpty<P>)
    where
        P: AsRef<Self::Processor>,
    {
        todo!()
    }
}

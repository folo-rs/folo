use std::fmt::Display;

use derive_more::derive::AsRef;

use crate::{pal, EfficiencyClass, MemoryRegionId, ProcessorCore, ProcessorId};

// This is a specialization of the *Core type for the build target platform. It is the only
// specialization available via the crate's public API surface - other specializations
// exist only for unit testing purposes where the platform is mocked, in which case the
// *Core type is used directly instead of using a newtype wrapper like we have here.

/// A processor present on the system and available to the current process.
#[derive(AsRef, Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Processor {
    #[as_ref(ProcessorCore<pal::BuildTargetPlatform>, pal::ProcessorImpl)]
    pub(crate) core: ProcessorCore<pal::BuildTargetPlatform>,
}

impl Processor {
    pub(crate) fn new(core: ProcessorCore<pal::BuildTargetPlatform>) -> Self {
        Self { core }
    }

    pub fn id(&self) -> ProcessorId {
        self.core.id()
    }

    pub fn memory_region_id(&self) -> MemoryRegionId {
        self.core.memory_region_id()
    }

    pub fn efficiency_class(&self) -> EfficiencyClass {
        self.core.efficiency_class()
    }
}

impl Display for Processor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.core, f)
    }
}

impl From<ProcessorCore<pal::BuildTargetPlatform>> for Processor {
    fn from(value: ProcessorCore<pal::BuildTargetPlatform>) -> Self {
        Self::new(value)
    }
}

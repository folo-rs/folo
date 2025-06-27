#![expect(clippy::same_name_method, reason = "mock magic")]

use derive_more::derive::Display;
use mockall::mock;
use nonempty::NonEmpty;

use crate::pal::{AbstractProcessor, Platform, ProcessorFacade};
use crate::{EfficiencyClass, MemoryRegionId, ProcessorId};

#[derive(Clone, Copy, Debug, Display, Eq, Hash, PartialEq)]
#[display("FakeProcessor({index} in node {memory_region}, {efficiency_class:?})")]
pub(crate) struct FakeProcessor {
    pub(crate) index: ProcessorId,
    pub(crate) memory_region: MemoryRegionId,
    pub(crate) efficiency_class: EfficiencyClass,
}

impl FakeProcessor {
    pub(crate) fn with_index(index: ProcessorId) -> Self {
        Self {
            index,
            memory_region: 0,
            efficiency_class: EfficiencyClass::Performance,
        }
    }
}

impl AbstractProcessor for FakeProcessor {
    fn id(&self) -> ProcessorId {
        self.index
    }

    fn memory_region_id(&self) -> MemoryRegionId {
        self.memory_region
    }

    fn efficiency_class(&self) -> EfficiencyClass {
        self.efficiency_class
    }
}

// Mockall is not able to express all methods on the trait (due to generics deficiency), so we mock
// similar-enough methods that it does know how to mock and simply call these from a manual
// implementation of the trait that translates between the two forms.
mock! {
    #[derive(Debug)]
    pub Platform {
        pub fn get_all_processors_core(&self) -> NonEmpty<ProcessorFacade>;
        pub fn pin_current_thread_to_core(&self, processors: Vec<ProcessorFacade>);
        pub fn current_processor_id(&self) -> ProcessorId;
        pub fn max_processor_id(&self) -> ProcessorId;
        pub fn max_memory_region_id(&self) -> MemoryRegionId;
        pub fn current_thread_processors(&self) -> NonEmpty<ProcessorId>;
        pub fn max_processor_time(&self) -> f64;
        pub fn active_processor_count(&self) -> usize;
    }
}

impl Platform for MockPlatform {
    fn get_all_processors(&self) -> NonEmpty<ProcessorFacade> {
        self.get_all_processors_core()
    }

    fn pin_current_thread_to<P>(&self, processors: &NonEmpty<P>)
    where
        P: AsRef<ProcessorFacade>,
    {
        let processors = processors.iter().map(|p| *p.as_ref()).collect();
        self.pin_current_thread_to_core(processors);
    }

    fn current_processor_id(&self) -> ProcessorId {
        self.current_processor_id()
    }

    fn max_processor_id(&self) -> ProcessorId {
        self.max_processor_id()
    }

    fn max_memory_region_id(&self) -> MemoryRegionId {
        self.max_memory_region_id()
    }

    fn current_thread_processors(&self) -> NonEmpty<ProcessorId> {
        self.current_thread_processors()
    }

    fn max_processor_time(&self) -> f64 {
        self.max_processor_time()
    }

    fn active_processor_count(&self) -> usize {
        self.active_processor_count()
    }
}

use derive_more::derive::Display;
use mockall::mock;
use nonempty::NonEmpty;

use crate::pal::{EfficiencyClass, MemoryRegionIndex, Platform, AbstractProcessor, ProcessorGlobalIndex};

#[derive(Clone, Copy, Debug, Display, Eq, Hash, PartialEq)]
#[display("FakeProcessor({index} in node {memory_region}, {efficiency_class:?})")]
pub(crate) struct FakeProcessor {
    pub(crate) index: ProcessorGlobalIndex,
    pub(crate) memory_region: MemoryRegionIndex,
    pub(crate) efficiency_class: EfficiencyClass,
}

impl AbstractProcessor for FakeProcessor {
    fn index(&self) -> ProcessorGlobalIndex {
        self.index
    }

    fn memory_region(&self) -> super::MemoryRegionIndex {
        self.memory_region
    }

    fn efficiency_class(&self) -> super::EfficiencyClass {
        self.efficiency_class
    }
}

// Mockall is not able to express all methods on the trait (due to generics deficiency), so we mock
// similar-enough methods that it does know how to mock and simply call these from a manual
// implementation of the trait that translates between the two forms.
mock! {
    #[derive(Debug)]
    pub Platform {
        pub fn get_all_processors_core(&self) -> NonEmpty<FakeProcessor>;
        pub fn pin_current_thread_to_core(&self, processors: Vec<FakeProcessor>);
    }
}

impl Platform for MockPlatform {
    type Processor = FakeProcessor;

    fn get_all_processors(&self) -> NonEmpty<FakeProcessor> {
        self.get_all_processors_core()
    }

    fn pin_current_thread_to<P>(&self, processors: &NonEmpty<P>)
    where
        P: AsRef<FakeProcessor>,
    {
        let processors = processors.iter().map(|p| *p.as_ref()).collect();
        self.pin_current_thread_to_core(processors);
    }
}

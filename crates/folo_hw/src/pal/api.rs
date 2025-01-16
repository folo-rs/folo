use std::{
    fmt::{Debug, Display},
    hash::Hash,
};

use nonempty::NonEmpty;

pub(crate) type ProcessorGlobalIndex = u32;
pub(crate) type MemoryRegionIndex = u32;

/// Differentiates processors by their efficiency class, allowing work requiring high
/// performance to be placed on the most performant processors at the expense of energy usage.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum EfficiencyClass {
    /// A processor that is optimized for energy efficiency at the expense of performance.
    Efficiency,

    /// A processor that is optimized for performance at the expense of energy efficiency.
    Performance,
}

pub(crate) trait Processor:
    Clone + Copy + Debug + Display + Eq + Hash + PartialEq + Send
{
    /// The global index of the processor, uniquely identifying it on the current system.
    fn index(&self) -> ProcessorGlobalIndex;

    /// The index of the memory region that the processor belongs to,
    /// uniquely identifying a specific memory region on the current system.
    fn memory_region(&self) -> MemoryRegionIndex;

    /// The efficiency class of the processor.
    fn efficiency_class(&self) -> EfficiencyClass;
}

pub(crate) trait Platform:
    Clone + Copy + Debug + Eq + Ord + Hash + PartialEq + PartialOrd + Send + Sync + 'static
{
    type Processor: Processor;

    fn get_all_processors(&self) -> NonEmpty<Self::Processor>;

    fn pin_current_thread_to<P>(&self, processors: &NonEmpty<P>)
    where
        P: AsRef<Self::Processor>;
}

use std::{
    fmt::{Debug, Display},
    hash::Hash,
};

use nonempty::NonEmpty;

use crate::{pal::ProcessorFacade, EfficiencyClass, MemoryRegionId, ProcessorId};

pub(crate) trait AbstractProcessor:
    Clone + Copy + Debug + Display + Eq + Hash + PartialEq + Send
{
    fn id(&self) -> ProcessorId;
    fn memory_region_id(&self) -> MemoryRegionId;
    fn efficiency_class(&self) -> EfficiencyClass;
}

pub(crate) trait Platform: Debug + Send + Sync + 'static {
    /// Returns all currently available processors.
    ///
    /// The returned collection of processors is sorted by the processor ID for ease of testing.
    /// TODO: Should we just sort in tests instead to avoid baking in some sorting assumptions?
    fn get_all_processors(&self) -> NonEmpty<ProcessorFacade>;

    fn pin_current_thread_to<P>(&self, processors: &NonEmpty<P>)
    where
        P: AsRef<ProcessorFacade>;
}

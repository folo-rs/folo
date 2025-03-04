use std::{
    fmt::{Debug, Display},
    hash::Hash,
};

use crate::{EfficiencyClass, MemoryRegionId, ProcessorId};

pub(crate) trait AbstractProcessor:
    Clone + Copy + Debug + Display + Eq + Hash + PartialEq + Send
{
    fn id(&self) -> ProcessorId;
    fn memory_region_id(&self) -> MemoryRegionId;
    fn efficiency_class(&self) -> EfficiencyClass;
}

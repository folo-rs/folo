use std::fmt::{Debug, Display};
use std::hash::Hash;

use crate::{EfficiencyClass, MemoryRegionId, ProcessorId, RelativeSpeed};

pub(crate) trait AbstractProcessor:
    Clone + Debug + Display + Eq + Hash + PartialEq + Send
{
    fn id(&self) -> ProcessorId;
    fn memory_region_id(&self) -> MemoryRegionId;
    fn efficiency_class(&self) -> EfficiencyClass;
    fn relative_speed(&self) -> RelativeSpeed;

    /// The best-effort model string of the processor, or `None` when the platform does not
    /// report one.
    fn model(&self) -> Option<&str>;
}

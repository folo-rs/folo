use std::fmt::{Debug, Display};

use crate::{EfficiencyClass, MemoryRegionId, ProcessorId, RelativeSpeed};

pub(crate) trait AbstractProcessor: Clone + Debug + Display + Send {
    fn id(&self) -> ProcessorId;
    fn memory_region_id(&self) -> MemoryRegionId;
    fn efficiency_class(&self) -> EfficiencyClass;
    fn relative_speed(&self) -> RelativeSpeed;

    /// Best-effort model of the processor, or `None` when the platform identifies it in no way
    /// we recognize. Implementations may assemble this from whatever identifying information the
    /// platform offers, so it carries no guaranteed format.
    fn model(&self) -> Option<&str>;
}

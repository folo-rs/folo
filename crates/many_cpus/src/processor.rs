use std::{
    fmt::{Debug, Display},
    hash::{Hash, Hasher},
};

use derive_more::derive::AsRef;

use crate::{
    EfficiencyClass, MemoryRegionId, ProcessorId,
    pal::{AbstractProcessor, PlatformFacade, ProcessorFacade},
};

/// A processor present on the system and available to the current process.
#[derive(AsRef, Clone)]
pub struct Processor {
    #[as_ref]
    inner: ProcessorFacade,

    pub(crate) pal: PlatformFacade,
}

impl Processor {
    pub(crate) fn new(inner: ProcessorFacade, pal: PlatformFacade) -> Self {
        Self { inner, pal }
    }

    pub fn id(&self) -> ProcessorId {
        self.inner.id()
    }

    pub fn memory_region_id(&self) -> MemoryRegionId {
        self.inner.memory_region_id()
    }

    pub fn efficiency_class(&self) -> EfficiencyClass {
        self.inner.efficiency_class()
    }
}

impl PartialEq for Processor {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}
impl Eq for Processor {}

impl Hash for Processor {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.inner.hash(state)
    }
}

impl Display for Processor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.inner, f)
    }
}

impl Debug for Processor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Debug::fmt(&self.inner, f)
    }
}

#[cfg(test)]
mod tests {
    use crate::pal::{FakeProcessor, MockPlatform};

    use super::*;

    #[test]
    fn smoke_test() {
        let pal_processor = FakeProcessor {
            index: 42,
            memory_region: 13,
            efficiency_class: EfficiencyClass::Efficiency,
        };

        let processor = Processor::new(pal_processor.into(), MockPlatform::new().into());

        // Getters appear to get the expected values.
        assert_eq!(processor.id(), 42);
        assert_eq!(processor.memory_region_id(), 13);
        assert_eq!(processor.efficiency_class(), EfficiencyClass::Efficiency);

        // A clone is a legit clone.
        let processor_clone = processor.clone();
        assert_eq!(processor, processor_clone);

        // Clones have the same hash.
        let mut hasher1 = std::collections::hash_map::DefaultHasher::new();
        processor.hash(&mut hasher1);
        let hash1 = hasher1.finish();

        let mut hasher2 = std::collections::hash_map::DefaultHasher::new();
        processor_clone.hash(&mut hasher2);
        let hash2 = hasher2.finish();

        assert_eq!(hash1, hash2);

        // Display writes something (anything - as long as it writes something and does not panic).
        let displayed = format!("{processor}");
        assert!(!displayed.is_empty());

        // Debug writes something (anything - as long as it writes something and does not panic).
        let debugged = format!("{processor:?}");
        assert!(!debugged.is_empty());
    }
}

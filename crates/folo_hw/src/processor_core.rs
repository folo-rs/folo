use std::{
    fmt::Display,
    hash::{Hash, Hasher},
};

use derive_more::derive::AsRef;

use crate::pal::{EfficiencyClass, MemoryRegionIndex, Platform, AbstractProcessor, ProcessorGlobalIndex};

#[derive(AsRef, Debug)]
pub(crate) struct ProcessorCore<PAL: Platform> {
    #[as_ref]
    inner: PAL::Processor,

    pub(crate) pal: &'static PAL,
}

impl<PAL: Platform> ProcessorCore<PAL> {
    pub(crate) fn new(inner: PAL::Processor, pal: &'static PAL) -> Self {
        Self { inner, pal }
    }

    pub(crate) fn index(&self) -> ProcessorGlobalIndex {
        self.inner.index()
    }

    pub(crate) fn memory_region(&self) -> MemoryRegionIndex {
        self.inner.memory_region()
    }

    pub(crate) fn efficiency_class(&self) -> EfficiencyClass {
        self.inner.efficiency_class()
    }
}

impl<PAL: Platform> Copy for ProcessorCore<PAL> {}
impl<PAL: Platform> Clone for ProcessorCore<PAL> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<PAL: Platform> PartialEq for ProcessorCore<PAL> {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}
impl<PAL: Platform> Eq for ProcessorCore<PAL> {}

impl<PAL: Platform> Hash for ProcessorCore<PAL> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.inner.hash(state)
    }
}

impl<PAL: Platform> Display for ProcessorCore<PAL> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.inner, f)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::LazyLock;

    use crate::pal::{FakeProcessor, MockPlatform};

    use super::*;

    #[test]
    fn smoke_test() {
        let pal_processor = FakeProcessor {
            index: 42,
            memory_region: 13,
            efficiency_class: EfficiencyClass::Efficiency,
        };

        static PAL: LazyLock<MockPlatform> = LazyLock::new(MockPlatform::new);

        let processor = ProcessorCore::new(pal_processor, &*PAL);

        // Getters appear to get the expected values.
        assert_eq!(processor.index(), 42);
        assert_eq!(processor.memory_region(), 13);
        assert_eq!(processor.efficiency_class(), EfficiencyClass::Efficiency);

        // A clone is a legit clone.
        #[expect(clippy::clone_on_copy)] // Intentional.
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

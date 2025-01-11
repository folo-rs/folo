/// A processor present on the system and available to the current process.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Processor {}

impl Display for Processor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "processor (linux)",)
    }
}

impl ProcessorPal for Processor {
    fn index(&self) -> ProcessorGlobalIndex {
        todo!()
    }

    fn memory_region_index(&self) -> MemoryRegionIndex {
        todo!()
    }

    fn efficiency_class(&self) -> EfficiencyClass {
        todo!()
    }
}

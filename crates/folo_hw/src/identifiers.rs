/// Uniquely identifies a processor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProcessorId {
    group: u16,
    index_in_group: u8,
}

impl ProcessorId {
    pub(crate) fn new(group: u16, index_in_group: u8) -> Self {
        Self {
            group,
            index_in_group,
        }
    }

    /// The 0-based index of the processor group.
    pub fn group(&self) -> u16 {
        self.group
    }

    /// The 0-based index of the processor in the processor group.
    pub fn index_in_group(&self) -> u8 {
        self.index_in_group
    }
}

/// Uniquely identifies a memory region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MemoryRegionId {
    raw: u32,
}

impl MemoryRegionId {
    pub(crate) fn new(raw: u32) -> Self {
        Self { raw }
    }

    pub(crate) fn as_raw(&self) -> u32 {
        self.raw
    }
}

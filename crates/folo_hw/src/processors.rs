use std::{
    fmt::{Debug, Display},
    num::NonZeroUsize,
    sync::LazyLock,
};

use windows::{
    core::HRESULT,
    Win32::{
        Foundation::ERROR_INSUFFICIENT_BUFFER,
        System::{
            SystemInformation::{
                GetLogicalProcessorInformationEx, RelationNumaNodeEx, RelationProcessorCore,
                LOGICAL_PROCESSOR_RELATIONSHIP, SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
            },
            Threading::{GetMaximumProcessorCount, GetMaximumProcessorGroupCount},
        },
    },
};

pub(crate) type ProcessorGroupIndex = u16;
pub(crate) type ProcessorIndexInGroup = u8;
pub(crate) type ProcessorGlobalIndex = u32;
pub(crate) type MemoryRegionIndex = u32;

/// A processor present on the system and available to the current process.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Processor {
    group_index: ProcessorGroupIndex,
    index_in_group: ProcessorIndexInGroup,

    // Cumulative index when counting across all groups.
    global_index: ProcessorGlobalIndex,
}

impl Processor {
    fn new(
        group_index: ProcessorGroupIndex,
        index_in_group: ProcessorIndexInGroup,
        global_index: ProcessorGlobalIndex,
    ) -> Self {
        Self {
            group_index,
            index_in_group,
            global_index,
        }
    }

    /// The 0-based index of the processor group the processor belongs to.
    pub(crate) fn group_index(&self) -> ProcessorGroupIndex {
        self.group_index
    }

    /// The 0-based index of the processor in the processor group.
    pub(crate) fn index_in_group(&self) -> ProcessorIndexInGroup {
        self.index_in_group
    }

    /// The 0-based index of the processor in the system.
    /// This is a cumulative index across all processor groups.
    pub(crate) fn global_index(&self) -> ProcessorGlobalIndex {
        self.global_index
    }
}

impl Display for Processor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "processor {} [{}-{}]",
            self.global_index, self.group_index, self.index_in_group
        )
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct MemoryRegion {
    index: MemoryRegionIndex,
}

impl MemoryRegion {
    fn new(id: MemoryRegionIndex) -> Self {
        Self { index: id }
    }
}

/// One or more processors present on the system.
///
/// You can obtain the full set of processors via `ProcessorSet::all()` or specify more
/// fine-grained selection criteria via `ProcessorSet::builder()`.
///
/// One you have a `ProcessorSet`, you can iterate over it to inspect the individual processors.
///
/// # Changes at runtime
///
/// It is possible that a system will have processors added or removed at runtime. This is not
/// supported - any hardware changes made at runtime will not be visible to the `ProcessorSet`
/// instances. Operations attempted on removed processors may fail with an error or panic.
#[derive(Clone, Debug)]
pub struct ProcessorSet {
    by_memory_regions: Box<[(MemoryRegion, Box<[Processor]>)]>,
}

impl ProcessorSet {
    /// Gets a `ProcessorSet` referencing all processors on the system.
    pub fn all() -> &'static Self {
        &ALL_PROCESSORS
    }

    pub fn builder() -> ProcessorSetBuilder {
        ProcessorSetBuilder::default()
    }

    #[expect(clippy::len_without_is_empty)] // Never empty by definition.
    pub fn len(&self) -> usize {
        todo!()
    }
}

#[derive(Clone, Debug, Default)]
pub struct ProcessorSetBuilder {
    processor_type_selector: ProcessorTypeSelector,
    memory_region_selector: MemoryRegionSelector,

    except_global_indexes: Vec<ProcessorGlobalIndex>,
}

impl ProcessorSetBuilder {
    pub fn performance_processors_only(mut self) -> Self {
        self.processor_type_selector = ProcessorTypeSelector::Performance;
        self
    }

    pub fn efficiency_processors_only(mut self) -> Self {
        self.processor_type_selector = ProcessorTypeSelector::Efficiency;
        self
    }

    pub fn different_memory_regions(mut self) -> Self {
        self.memory_region_selector = MemoryRegionSelector::RequireDifferent;
        self
    }

    pub fn same_memory_region(mut self) -> Self {
        self.memory_region_selector = MemoryRegionSelector::RequireSame;
        self
    }

    pub fn except<I>(mut self, processors: I) -> Self
    where
        I: IntoIterator<Item = Processor>,
    {
        for processor in processors {
            self.except_global_indexes.push(processor.global_index());
        }
        
        self
    }

    /// Picks a specific number of processors from the set of candidates and returns a
    /// processor set with the requested number of processors that match specified criteria.
    ///
    /// Returns `None` if there were not enough matching processors to satisfy the request.
    pub fn take(self, count: NonZeroUsize) -> Option<ProcessorSet> {
        todo!()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
enum MemoryRegionSelector {
    /// The default - processors are picked by default from the same memory region but if there
    /// are not enough processors in the same region, processors from different regions may be
    /// used to satisfy the request.
    #[default]
    PreferSame,

    /// Processors are all from the same memory region. If there are not enough processors in any
    /// memory region, the request will fail.
    RequireSame,

    /// Processors are all from the different memory regions. If there are not enough memory
    /// regions, the request will fail.
    RequireDifferent,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
enum ProcessorTypeSelector {
    /// The default - all processors are valid candidates.
    #[default]
    Any,

    /// Only performance processors (faster, energy-hungry) are valid candidates.
    Performance,

    /// Only efficiency processors (slower, energy-efficient) are valid candidates.
    Efficiency,
}

static ALL_PROCESSORS: LazyLock<ProcessorSet> = LazyLock::new(get_all);

/// Returns the max number of processors in each processor group.
/// This is used to calculate the global index of a processor.
fn get_processor_group_sizes() -> Box<[u8]> {
    // SAFETY: No safety requirements, just an FFI call.
    let group_count = unsafe { GetMaximumProcessorGroupCount() };

    let mut group_sizes = Vec::with_capacity(group_count as usize);

    for group_index in 0..group_count {
        // SAFETY: No safety requirements, just an FFI call.
        let processor_count = unsafe { GetMaximumProcessorCount(group_index) };

        // The OS says there are up to 64, so this is guaranteed but let's be explicit.
        assert!(processor_count <= u8::MAX as u32);
        let processor_count = processor_count as u8;

        group_sizes.push(processor_count);
    }

    group_sizes.into_boxed_slice()
}

fn get_logical_processor_information_raw(
    relationship: LOGICAL_PROCESSOR_RELATIONSHIP,
) -> Box<[u8]> {
    loop {
        let mut required_length: u32 = 0;

        // SAFETY: No safety requirements beyond passing valid inputs, just an FFI call.
        let probe_result = unsafe {
            GetLogicalProcessorInformationEx(relationship, None, &raw mut required_length)
        };
        let e = probe_result.expect_err("GetLogicalProcessorInformationEx with null buffer must always fail and return required buffer size");
        assert_eq!(
            e.code(),
            HRESULT::from_win32(ERROR_INSUFFICIENT_BUFFER.0),
            "GetLogicalProcessorInformationEx size probe failed with unexpected error code {e}",
        );

        let mut buffer: Vec<u8> = Vec::with_capacity(required_length as usize);
        let mut final_length = required_length;

        // SAFETY: No safety requirements beyond passing valid inputs, just an FFI call.
        let real_result = unsafe {
            GetLogicalProcessorInformationEx(
                relationship,
                Some(buffer.as_mut_ptr().cast()),
                &raw mut final_length,
            )
        };

        // In theory, it could still have failed with "insufficient buffer" because the set of
        // processors available to us can change at any time. Super unlikely but let's be safe.
        if let Err(e) = real_result {
            if e.code() == HRESULT::from_win32(ERROR_INSUFFICIENT_BUFFER.0) {
                // Well, we just have to try again then.
                continue;
            }

            panic!("GetLogicalProcessorInformationEx failed with unexpected error code: {e}",);
        }

        // Signal the buffer that we wrote into it.
        // SAFETY: We must be sure that the specified number of bytes have actually been written.
        // We are sure, obviously, because the operating system just told us it did that.
        unsafe {
            buffer.set_len(final_length as usize);
        }

        return buffer.into_boxed_slice();
    }
}

// Gets the global index of every performance processor.
// Implicitly, anything not on this list is an efficiency processor.
fn get_performance_processor_global_indexes() -> Box<[ProcessorGlobalIndex]> {
    let core_relationships_raw = get_logical_processor_information_raw(RelationProcessorCore);

    let mut result_zero = Vec::new();
    let mut result_nonzero = Vec::new();

    // The structures returned by the OS are dynamically sized so we only have various
    // disgusting options for parsing/processing them. Pointer wrangling is the most readable.
    let raw_range = core_relationships_raw.as_ptr_range();
    let mut next: *const SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX = raw_range.start.cast();
    let end = raw_range.end.cast();

    while next < end {
        // SAFETY: We just process the data in the form the OS promises to give it to us.
        let info = unsafe { &*next };

        // SAFETY: We just process the data in the form the OS promises to give it to us.
        next = unsafe { next.byte_add(info.Size as usize) };

        assert_eq!(info.Relationship, RelationProcessorCore);

        let details = unsafe { &info.Anonymous.Processor };

        // API docs: If the PROCESSOR_RELATIONSHIP structure represents a processor core,
        // the GroupCount member is always 1.
        assert_eq!(details.GroupCount, 1);

        // There may be 1 or more bits set in the mask because one core might have multiple logical
        // processors via SMT (hyperthreading). We just iterate over all the bits to check them
        // individually without worrying about SMT logic.
        let group_index: ProcessorGroupIndex = details.GroupMask[0].Group;

        let processors_in_group = unsafe { GetMaximumProcessorCount(group_index) };

        // Minimum effort approach for WOW64 support - we only see the first 32 in a group.
        let processors_in_group = processors_in_group.min(usize::BITS);

        let mask = details.GroupMask[0].Mask;

        for index_in_group in 0..processors_in_group {
            if mask & (1 << index_in_group) == 0 {
                continue;
            }

            let global_index: ProcessorGlobalIndex =
                group_index as u32 * processors_in_group + index_in_group;

            if details.EfficiencyClass == 0 {
                result_nonzero.push(global_index);
            } else {
                result_zero.push(global_index);
            }
        }
    }

    // If all processors are equal, they might all be in the "zero" list.
    if result_nonzero.is_empty() {
        result_zero.into_boxed_slice()
    } else {
        result_nonzero.into_boxed_slice()
    }
}

fn get_all() -> ProcessorSet {
    let processor_group_sizes = get_processor_group_sizes();

    // This will give us the grouping by memory regions.
    let memory_region_relationships_raw = get_logical_processor_information_raw(RelationNumaNodeEx);

    todo!()
}

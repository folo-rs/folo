use std::{collections::HashMap, fmt::Display};

use nonempty::NonEmpty;
use windows::{
    core::HRESULT,
    Win32::{
        Foundation::ERROR_INSUFFICIENT_BUFFER,
        System::{
            SystemInformation::{
                GetLogicalProcessorInformationEx, RelationNumaNode, RelationNumaNodeEx,
                RelationProcessorCore, GROUP_AFFINITY, LOGICAL_PROCESSOR_RELATIONSHIP,
                SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
            },
            Threading::{
                GetActiveProcessorCount, GetCurrentThread, GetMaximumProcessorCount,
                GetMaximumProcessorGroupCount, SetThreadGroupAffinity,
            },
        },
    },
};

use crate::pal::{
    EfficiencyClass, MemoryRegionIndex, PlatformCommon, ProcessorCommon, ProcessorGlobalIndex,
};

type ProcessorGroupIndex = u16;
type ProcessorIndexInGroup = u8;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct Processor {
    group_index: ProcessorGroupIndex,
    index_in_group: ProcessorIndexInGroup,

    // Cumulative index when counting across all groups.
    global_index: ProcessorGlobalIndex,

    memory_region_index: MemoryRegionIndex,

    efficiency_class: EfficiencyClass,
}

impl Processor {
    fn new(
        group_index: ProcessorGroupIndex,
        index_in_group: ProcessorIndexInGroup,
        global_index: ProcessorGlobalIndex,
        memory_region_index: MemoryRegionIndex,
        efficiency_class: EfficiencyClass,
    ) -> Self {
        Self {
            group_index,
            index_in_group,
            global_index,
            memory_region_index,
            efficiency_class,
        }
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

impl ProcessorCommon for Processor {
    fn index(&self) -> ProcessorGlobalIndex {
        self.global_index
    }

    fn memory_region(&self) -> MemoryRegionIndex {
        self.memory_region_index
    }

    fn efficiency_class(&self) -> EfficiencyClass {
        self.efficiency_class
    }
}

#[derive(Copy, Clone, Debug, Default, Eq, Ord, Hash, PartialEq, PartialOrd)]
pub(crate) struct Platform;

impl PlatformCommon for Platform {
    type Processor = Processor;

    fn get_all_processors(&self) -> NonEmpty<Self::Processor> {
        get_all()
    }

    fn pin_current_thread_to<P>(&self, processors: &NonEmpty<P>)
    where
        P: AsRef<Self::Processor>,
    {
        // TODO: Figure out the details on how to handle multi-member sets in terms of fairness.

        // We need to do two things here, for each processor group:
        // 1. Set the affinity mask to allow execution on the indicated processors.
        // 2. Set the affinity mask to disallow execution on all other processors.
        //
        // The order does not super matter because setting a thread as non-affine to all processors
        // seems to, in practice, simply allow it to be executed on all of them. Therefore we can
        // implement this as a single "clear all + set desired" pass without worrying about
        // any potential intermediate state where everything is cleared.

        // This is a pseudo handle and does not need to be closed.
        // SAFETY: Nothing required, just an FFI call.
        let current_thread = unsafe { GetCurrentThread() };

        let processor_group_sizes = get_processor_group_max_sizes();

        for group_index in 0..processor_group_sizes.len() {
            let mut affinity = GROUP_AFFINITY {
                Group: group_index as ProcessorGroupIndex,
                Mask: 0,
                ..Default::default()
            };

            // We started with all bits clear and now set any that we do want to allow.
            for processor in processors
                .iter()
                .filter(|p| p.as_ref().group_index == group_index as ProcessorGroupIndex)
            {
                affinity.Mask |= 1 << processor.as_ref().index_in_group;
            }

            // We do not expect this to ever fail - these flags should be settable
            // for all processor groups at all times for the current thread, even if
            // the hardware dynamically changes (the affinity masks are still valid, after all,
            // even if they point to non-existing processors).
            unsafe { SetThreadGroupAffinity(current_thread, &affinity, None) }
                .expect("SetThreadGroupAffinity failed unexpectedly");
        }
    }
}

/// Returns the max number of processors in each processor group.
/// This is used to calculate the global index of a processor.
fn get_processor_group_max_sizes() -> Box<[u8]> {
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

/// Returns the active number of processors in each processor group.
/// This is used to identify which processors actually exist.
fn get_processor_group_active_sizes() -> Box<[u8]> {
    // We always consider all groups, even if they have 0 processors.
    // SAFETY: No safety requirements, just an FFI call.
    let group_count = unsafe { GetMaximumProcessorGroupCount() };

    let mut group_sizes = Vec::with_capacity(group_count as usize);

    for group_index in 0..group_count {
        // SAFETY: No safety requirements, just an FFI call.
        let processor_count = unsafe { GetActiveProcessorCount(group_index) };

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

fn get_all() -> NonEmpty<Processor> {
    let memory_region_relationships_raw = get_logical_processor_information_raw(RelationNumaNodeEx);

    // This is the data we want to extract - a mapping of which processor group belongs to
    // which NUMA node. Implicitly, each group can only belong to one NUMA node, though multiple
    // groups may belong to the same NUMA node.
    let mut processor_group_to_numa_node = HashMap::new();

    // The structures returned by the OS are dynamically sized so we only have various
    // disgusting options for parsing/processing them. Pointer wrangling is the most readable.
    let raw_range = memory_region_relationships_raw.as_ptr_range();
    let mut next: *const SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX = raw_range.start.cast();
    let end = raw_range.end.cast();

    while next < end {
        // SAFETY: We just process the data in the form the OS promises to give it to us.
        let info = unsafe { &*next };

        // SAFETY: We just process the data in the form the OS promises to give it to us.
        next = unsafe { next.byte_add(info.Size as usize) };

        // Even though we request NumaNodeEx, it returns entries with NumaNode because... it does.
        // TODO: Does it ever return NumaNodeEx? What about when returning multiple groups?
        assert_eq!(info.Relationship, RelationNumaNode);

        let details = unsafe { &info.Anonymous.NumaNode };

        let numa_node_number = details.NodeNumber;

        // In the struct definition, this is a 1-element array because Rust has no notion
        // of dynamic-size arrays. We use pointer arithmetic to access the real array elements.
        //
        // INSANITY: this & is crucial here because otherwise the compiler will give us a pointer
        // to some temporary value on the stack that does not actually contain the correct data.
        // This is because without the &, the Rust compiler will happily try to copy out the value
        // of GroupMasks and use that. This would be a guaranteed problem if GroupCount>1 since the
        // compiler did not know it would have to copy more than one element. However, for some
        // unclear reason it is also a problem with GroupCount==1 because the compiler apparently
        // copies out some garbage onto the stack instead of the correct value. Reason unknown.
        // Just make sure to keep the & here and anywhere else unions are used by reference/ptr.
        //
        // SAFETY: RelationNumaNodeEx guarantees that this union member is present.
        let mut group_mask_array = unsafe { &details.Anonymous.GroupMasks }.as_ptr();

        for _ in 0..details.GroupCount {
            // SAFETY: The OS promises us that this array contains `GroupCount` elements.
            let group_number = unsafe { *group_mask_array }.Group;

            // We do not care about the mask itself because all group members are implicitly
            // part of the same NUMA node - that's how groups are constructed by the OS.
            processor_group_to_numa_node.insert(group_number, numa_node_number);

            // SAFETY: The OS promises us that this array contains `GroupCount` elements.
            // It is fine to move past the end if we never access it (because the loop ends).
            group_mask_array = unsafe { group_mask_array.add(1) };
        }
    }

    let processor_group_max_sizes = get_processor_group_max_sizes();
    let processor_group_active_sizes = get_processor_group_active_sizes();
    let performance_processors = get_performance_processor_global_indexes();

    let mut processors =
        Vec::with_capacity(processor_group_max_sizes.iter().map(|&s| s as usize).sum());

    for group_index in 0..processor_group_max_sizes.len() {
        // The next global index is recalculated at the beginning of every processor group
        // because there may be a gap at the end of the previous processor group if it is
        // a dynamically sized group.
        let mut next_global_index = processor_group_max_sizes
            .iter()
            .take(group_index)
            .map(|&x| x as u32)
            .sum();

        for index_in_group in 0..processor_group_active_sizes[group_index] {
            let global_index = next_global_index;
            next_global_index += 1;

            let memory_region_index = *processor_group_to_numa_node
                .get(&(group_index as u16))
                .expect(
                "a processor group exists that the OS did not provide a memory region mapping for",
            );

            let efficiency_class = if performance_processors.contains(&global_index) {
                EfficiencyClass::Performance
            } else {
                EfficiencyClass::Efficiency
            };

            let processor = Processor::new(
                group_index as ProcessorGroupIndex,
                index_in_group,
                global_index,
                memory_region_index,
                efficiency_class,
            );

            processors.push(processor);
        }
    }

    NonEmpty::from_vec(processors).expect(
        "we are returning all processors on the system - obviously there must be at least one",
    )
}

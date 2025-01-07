use std::{collections::HashMap, mem::offset_of};

use bytes::{Buf, BufMut, Bytes, BytesMut};
use itertools::Itertools;
use windows::{
    core::HRESULT,
    Win32::{
        Foundation::ERROR_INSUFFICIENT_BUFFER,
        System::{
            SystemInformation::{
                GetLogicalProcessorInformationEx, RelationNumaNodeEx, GROUP_AFFINITY,
                LOGICAL_PROCESSOR_RELATIONSHIP, NUMA_NODE_RELATIONSHIP,
                SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
            },
            Threading::GetActiveProcessorCount,
        },
    },
};

use crate::{MemoryRegionId, ProcessorId};

fn get_logical_processor_information_numa_ex_raw() -> Bytes {
    loop {
        let mut required_length: u32 = 0;

        // SAFETY: Straightforward FFI call, no concerns beyond passing correct parameters.
        let probe_result = unsafe {
            GetLogicalProcessorInformationEx(RelationNumaNodeEx, None, &raw mut required_length)
        };
        let e = probe_result.expect_err("GetLogicalProcessorInformationEx with null buffer must always fail and return required buffer size");
        assert_eq!(
            e.code(),
            HRESULT::from_win32(ERROR_INSUFFICIENT_BUFFER.0),
            "GetLogicalProcessorInformationEx failed with unexpected error code {e}",
        );

        let mut buffer = BytesMut::with_capacity(required_length as usize);
        let mut final_length = required_length;

        // SAFETY: Straightforward FFI call, no concerns beyond passing correct parameters.
        let real_result = unsafe {
            GetLogicalProcessorInformationEx(
                RelationNumaNodeEx,
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
        // SAFETY: We must be sure that the specified number of bytes has actually been written.
        // We are, obviously, because the operating system just told us it did that.
        unsafe {
            buffer.advance_mut(final_length as usize);
        }

        return buffer.freeze();
    }
}

pub(crate) fn get_active_processors_by_memory_region() -> Box<[MemoryRegionProcessors]> {
    // TODO: We can probably do this much easier with pointer funny business and unsafe code.

    // This data format is a colossal pain to use because the structures are dynamically sized.
    // We need to parse the bytes in a streaming manner until we run out of bytes to parse.
    let mut processor_infos_buffer = get_logical_processor_information_numa_ex_raw();

    // We will do this manually for now, to avoid taking a dependency on some parser lib.
    // If we only need to do it here, it is not as horrible as it could potentially be.

    // This is the data we want to extract - a mapping of which processor group belongs to
    // which NUMA node. We assume that each group can only belong to one NUMA node.
    let mut numa_node_and_group_pairs = Vec::new();

    const OFFSET_TO_SIZE: usize = offset_of!(SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX, Size);
    const OFFSET_TO_UNION_0: usize = offset_of!(SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX, Anonymous);
    const OFFSET_TO_NUMA_NODE_NUMBER: usize = offset_of!(NUMA_NODE_RELATIONSHIP, NodeNumber);
    const OFFSET_TO_NUMA_GROUP_COUNT: usize = offset_of!(NUMA_NODE_RELATIONSHIP, GroupCount);
    const OFFSET_TO_NUMA_GROUP_ARRAY: usize =
        offset_of!(NUMA_NODE_RELATIONSHIP, Anonymous.GroupMasks);
    const OFFSET_TO_NUMA_GROUP_NUMBER: usize = offset_of!(GROUP_AFFINITY, Group);
    const GROUP_AFFINITY_SIZE: usize = size_of::<GROUP_AFFINITY>();

    while !processor_infos_buffer.is_empty() {
        // We are now at the start of a SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX structure.

        processor_infos_buffer.advance(OFFSET_TO_SIZE);
        let processor_info_size = processor_infos_buffer.get_u32_le();

        // Seek over any padding to the union.
        processor_infos_buffer.advance(OFFSET_TO_UNION_0 - OFFSET_TO_SIZE - size_of::<u32>());

        let remaining_processor_info_size = processor_info_size as usize - OFFSET_TO_UNION_0;
        // For later sanity check, to verify we consumed entire current entry.
        let expected_remaining_after_current = processor_infos_buffer
            .remaining()
            .checked_sub(remaining_processor_info_size)
            .expect("calculated that we need to read more data than exists in GetLogicalProcessorInformationEx buffer");

        // We are now at the start of a NUMA_NODE_RELATIONSHIP structure.

        processor_infos_buffer.advance(OFFSET_TO_NUMA_NODE_NUMBER);
        let numa_node_number = processor_infos_buffer.get_u32_le();

        processor_infos_buffer
            .advance(OFFSET_TO_NUMA_GROUP_COUNT - OFFSET_TO_NUMA_NODE_NUMBER - size_of::<u32>());
        let group_count = processor_infos_buffer.get_u16_le();

        processor_infos_buffer
            .advance(OFFSET_TO_NUMA_GROUP_ARRAY - OFFSET_TO_NUMA_GROUP_COUNT - size_of::<u16>());

        // We are now at the start of an array [GROUP_AFFINITY; group_count].

        // We are going to assume that all processors in a group belong to the same NUMA node
        // and will skip parsing the affinity mask itself. This seems to be an implicit
        // element of the processor group design, so should be relatively safe to assume.
        for _ in 0..group_count {
            processor_infos_buffer.advance(OFFSET_TO_NUMA_GROUP_NUMBER);
            let group_number = processor_infos_buffer.get_u16_le();

            numa_node_and_group_pairs.push((numa_node_number, group_number));

            // Seek over any remainder to the next GROUP_AFFINITY structure.
            processor_infos_buffer
                .advance(GROUP_AFFINITY_SIZE - OFFSET_TO_NUMA_GROUP_NUMBER - size_of::<u16>());
        }

        // There might still be some padding remaining in the current entry. Skip over it if so.
        processor_infos_buffer.advance(
            expected_remaining_after_current
                .checked_sub(processor_infos_buffer.remaining())
                .expect("calculated that we need to advance past end of GetLogicalProcessorInformationEx buffer"),
        );
    }

    let numa_nodes_to_processor_groups = numa_node_and_group_pairs.into_iter().into_group_map();

    numa_nodes_to_processor_groups
        .into_iter()
        .filter_map(|(numa_node, processor_groups)| {
            let processor_ids = processor_groups
                .into_iter()
                .flat_map(|group_number| {
                    // TODO: What happens if a processor group has 0 active processors?
                    // 0 is defined as an error result, so it cannot just be a valid value here.

                    // SAFETY: Trivial FFI call, nothing to worry about.
                    let active_processors_in_group =
                        unsafe { GetActiveProcessorCount(group_number) };

                    // The OS says there are up to 64, so this is guaranteed but let's be explicit.
                    assert!(active_processors_in_group <= u8::MAX as u32);
                    let active_processors_in_group = active_processors_in_group as u8;

                    (0..active_processors_in_group).map(move |processor_number| {
                        ProcessorId::new(group_number, processor_number)
                    })
                })
                .collect_vec()
                .into_boxed_slice();

            if processor_ids.is_empty() {
                None
            } else {
                Some(MemoryRegionProcessors {
                    id: MemoryRegionId::new(numa_node),
                    processors: processor_ids,
                })
            }
        })
        .collect_vec()
        .into_boxed_slice()
}

#[derive(Debug)]
pub(crate) struct MemoryRegionProcessors {
    id: MemoryRegionId,
    processors: Box<[ProcessorId]>,
}

impl MemoryRegionProcessors {
    pub(crate) fn id(&self) -> &MemoryRegionId {
        &self.id
    }

    pub(crate) fn processors(&self) -> &[ProcessorId] {
        &self.processors
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_test() {
        let xyz = get_active_processors_by_memory_region();
        println!("{xyz:?}");
    }
}

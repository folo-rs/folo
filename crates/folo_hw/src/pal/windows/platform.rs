use std::{collections::HashMap, mem::offset_of, num::NonZeroUsize};

use itertools::Itertools;
use nonempty::NonEmpty;
use windows::{
    core::HRESULT,
    Win32::{
        Foundation::ERROR_INSUFFICIENT_BUFFER,
        System::SystemInformation::{
            RelationNumaNode, RelationNumaNodeEx, RelationProcessorCore, GROUP_AFFINITY,
            LOGICAL_PROCESSOR_RELATIONSHIP, SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
        },
    },
};

use crate::{
    pal::{
        windows::{Bindings, BuildTargetBindings, NativeBuffer, ProcessorGroupIndex},
        Platform, ProcessorImpl,
    },
    EfficiencyClass, ProcessorId,
};

/// Singleton instance of `BuildTargetPlatform`, used by public API types
/// to hook up to the correct PAL implementation.
pub(crate) static BUILD_TARGET_PLATFORM: BuildTargetPlatform =
    BuildTargetPlatform::new(&BuildTargetBindings);

/// The platform that matches the crate's build target.
///
/// You would only use a different platform in unit tests that need to mock the platform.
/// Even then, whenever possible, unit tests should use the real platform for maximum realism.
#[derive(Debug)]
pub(crate) struct BuildTargetPlatform<B: Bindings = BuildTargetBindings> {
    bindings: &'static B,
}

impl<B: Bindings> Platform for BuildTargetPlatform<B> {
    type Processor = ProcessorImpl;

    fn get_all_processors(&self) -> NonEmpty<Self::Processor> {
        self.get_all()
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
        // seems to, in practice, simply allow it to be executed on all of them (or at least on
        // some of them). Therefore we can implement this as a single "clear all + set desired"
        // pass without worrying about any potential intermediate state where everything is cleared.

        // This is a pseudo handle and does not need to be closed.
        let current_thread = self.bindings.get_current_thread();

        let processor_group_sizes = self.get_processor_group_max_sizes();

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
            //
            // SAFETY: We need to ensure the affinity mask pointer remains valid for the duration
            // of the call. We do - it is backed by a local variable that lives to end of scope.
            unsafe {
                self.bindings
                    .set_thread_group_affinity(current_thread, &affinity, None)
            }
            .expect("SetThreadGroupAffinity failed unexpectedly");
        }
    }
}

impl<B: Bindings> BuildTargetPlatform<B> {
    pub(crate) const fn new(bindings: &'static B) -> Self {
        Self { bindings }
    }

    /// Returns the max number of processors in each processor group.
    /// This is used to calculate the global index of a processor.
    fn get_processor_group_max_sizes(&self) -> Box<[u8]> {
        let group_count = self.bindings.get_maximum_processor_group_count();

        let mut group_sizes = Vec::with_capacity(group_count as usize);

        for group_index in 0..group_count {
            let processor_count = self.bindings.get_maximum_processor_count(group_index);

            // The OS says there are up to 64, so this is guaranteed but let's be explicit.
            assert!(processor_count <= u8::MAX as u32);
            let processor_count = processor_count as u8;

            group_sizes.push(processor_count);
        }

        group_sizes.into_boxed_slice()
    }

    /// Returns the active number of processors in each processor group.
    /// This is used to identify which processors actually exist.
    fn get_processor_group_active_sizes(&self) -> Box<[u8]> {
        // We always consider all groups, even if they have 0 processors.
        let group_count = self.bindings.get_maximum_processor_group_count();

        let mut group_sizes = Vec::with_capacity(group_count as usize);

        for group_index in 0..group_count {
            let processor_count = self.bindings.get_active_processor_count(group_index);

            // The OS says there are up to 64, so this is guaranteed but let's be explicit.
            assert!(processor_count <= u8::MAX as u32);
            let processor_count = processor_count as u8;

            group_sizes.push(processor_count);
        }

        group_sizes.into_boxed_slice()
    }

    fn get_logical_processor_information_raw(
        &self,
        relationship: LOGICAL_PROCESSOR_RELATIONSHIP,
    ) -> NativeBuffer<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX> {
        loop {
            let mut required_length: u32 = 0;

            // SAFETY: Pointers must outlive the call (true - local variable lives beyond call).
            let probe_result = unsafe {
                self.bindings.get_logical_processor_information_ex(
                    relationship,
                    None,
                    &raw mut required_length,
                )
            };
            let e = probe_result.expect_err("GetLogicalProcessorInformationEx with null buffer must always fail and return required buffer size");
            assert_eq!(
                e.code(),
                HRESULT::from_win32(ERROR_INSUFFICIENT_BUFFER.0),
                "GetLogicalProcessorInformationEx size probe failed with unexpected error code {e}",
            );

            let required_length_usize = NonZeroUsize::new(required_length as usize).expect(
                "GetLogicalProcessorInformationEx size probe said 0 bytes are needed - impossible",
            );
            let mut buffer =
                NativeBuffer::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>::new(required_length_usize);
            let mut final_length = required_length;

            // SAFETY: Pointers must outlive the call (true - local variables live beyond call).
            let real_result = unsafe {
                self.bindings.get_logical_processor_information_ex(
                    relationship,
                    Some(buffer.as_ptr().as_ptr()),
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

            return buffer;
        }
    }

    // Gets the global index of every performance processor.
    // Implicitly, anything not on this list is an efficiency processor.
    fn get_performance_processor_global_indexes(
        &self,
        group_max_sizes: &[u8],
    ) -> Box<[ProcessorId]> {
        let core_relationships_raw =
            self.get_logical_processor_information_raw(RelationProcessorCore);

        // We create a map of processor index to efficiency class. Then we simply take all
        // processors with the max efficiency class (whatever the numeric value) - those are the
        // performance processors.

        let mut processor_to_efficiency_class = HashMap::new();

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

            let processors_in_group = group_max_sizes[group_index as usize] as u32;

            // Minimum effort approach for WOW64 support - we only see the first 32 in a group.
            let processors_in_group = processors_in_group.min(usize::BITS);

            let mask = details.GroupMask[0].Mask;

            for index_in_group in 0..processors_in_group {
                if mask & (1 << index_in_group) == 0 {
                    continue;
                }

                let group_start_offset: ProcessorId = group_max_sizes
                    .iter()
                    .take(group_index as usize)
                    .map(|x| *x as ProcessorId)
                    .sum();

                let global_index: ProcessorId = group_start_offset + index_in_group;
                processor_to_efficiency_class.insert(global_index, details.EfficiencyClass);
            }
        }

        let max_efficiency_class = processor_to_efficiency_class
            .values()
            .max()
            .copied()
            .expect(
                "there must be at least one processor - this code is running on one, after all",
            );

        processor_to_efficiency_class
            .iter()
            .filter_map(|(&global_index, &efficiency_class)| {
                if efficiency_class == max_efficiency_class {
                    Some(global_index)
                } else {
                    None
                }
            })
            .collect_vec()
            .into_boxed_slice()
    }

    fn get_all(&self) -> NonEmpty<ProcessorImpl> {
        let memory_region_relationships_raw =
            self.get_logical_processor_information_raw(RelationNumaNodeEx);

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
            let current = next;

            // SAFETY: We just process the data in the form the OS promises to give it to us.
            let info = unsafe { &*current };

            // SAFETY: We just process the data in the form the OS promises to give it to us.
            next = unsafe { next.byte_add(info.Size as usize) };

            // Even though we request NumaNodeEx, it returns entries with NumaNode. It just does.
            // It always does, asking for Ex simply allows it to return multiple processor groups
            // for the same NUMA node (instead of just the primary group as with plain NumaNode).
            assert_eq!(info.Relationship, RelationNumaNode);

            let details = unsafe { &info.Anonymous.NumaNode };

            let numa_node_number = details.NodeNumber;

            // In the struct definition, this is a 1-element array because Rust has no notion
            // of dynamic-size arrays. We use pointer arithmetic to access the real array elements.
            //
            // NOTE: that we need to start from scratch with the original pointer here!
            // Pointer -> shared ref -> pointer conversions are not guaranteed to return the
            // original pointer, so if we get rid of a pointer once, we cannot get it back!
            //
            // SAFETY: RelationNumaNodeEx guarantees that this union member is present.
            let mut group_mask_array = unsafe {
                current
                    .byte_add(offset_of!(
                        SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
                        Anonymous.NumaNode.Anonymous.GroupMask
                    ))
                    .cast::<GROUP_AFFINITY>()
            };

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

        let processor_group_max_sizes = self.get_processor_group_max_sizes();
        let processor_group_active_sizes = self.get_processor_group_active_sizes();
        let performance_processors =
            self.get_performance_processor_global_indexes(&processor_group_max_sizes);

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

                let processor = ProcessorImpl::new(
                    group_index as ProcessorGroupIndex,
                    index_in_group,
                    global_index,
                    memory_region_index,
                    efficiency_class,
                );

                processors.push(processor);
            }
        }

        // We must return the processors sorted by global index. While the above logic may
        // already ensure this as a side-effect, we will sort here explicitly to be sure.
        processors.sort();

        NonEmpty::from_vec(processors).expect(
            "we are returning all processors on the system - obviously there must be at least one",
        )
    }
}

#[cfg(test)]
mod tests {
    use std::{
        mem::{self, offset_of},
        sync::{Arc, LazyLock},
    };

    use itertools::Itertools;
    use mockall::Sequence;
    use windows::Win32::Foundation::{BOOL, HANDLE};

    use crate::{
        pal::windows::{MockBindings, NativeBuffer, ProcessorIndexInGroup},
        MemoryRegionId,
    };

    use super::*;

    #[test]
    fn get_all_processors_smoke_test() {
        // We imagine a simple system with 2 physical cores, 4 logical processors, all in a
        // single processor group and a single memory region. Welcome to 2010!
        static BINDINGS: LazyLock<MockBindings> = LazyLock::new(|| {
            let mut mock = MockBindings::new();

            simulate_processor_layout(&mut mock, [4], [4], [0], [vec![0, 0, 0, 0]]);

            mock
        });

        let platform = BuildTargetPlatform::new(&*BINDINGS);

        let processors = platform.get_all_processors();

        // We expect to see 4 logical processors. This API does not care about the physical cores.
        assert_eq!(processors.len(), 4);

        // All processors must be in the same group and memory region.
        assert_eq!(1, processors.iter().map(|p| p.group_index).dedup().count());
        assert_eq!(
            1,
            processors
                .iter()
                .map(|p| p.memory_region_id)
                .dedup()
                .count()
        );

        let p0 = &processors[0];
        assert_eq!(p0.group_index, 0);
        assert_eq!(p0.index_in_group, 0);
        assert_eq!(p0.memory_region_id, 0);

        let p1 = &processors[1];
        assert_eq!(p1.group_index, 0);
        assert_eq!(p1.index_in_group, 1);
        assert_eq!(p1.memory_region_id, 0);

        let p2 = &processors[2];
        assert_eq!(p2.group_index, 0);
        assert_eq!(p2.index_in_group, 2);
        assert_eq!(p2.memory_region_id, 0);

        let p3 = &processors[3];
        assert_eq!(p3.group_index, 0);
        assert_eq!(p3.index_in_group, 3);
        assert_eq!(p3.memory_region_id, 0);
    }

    #[test]
    fn test_two_numa_nodes_efficiency_performance() {
        static BINDINGS: LazyLock<MockBindings> = LazyLock::new(|| {
            let mut mock = MockBindings::new();
            // Two groups, each with 2 active processors:
            // Group 0 -> [Performance, Efficiency], Group 1 -> [Efficiency, Performance].
            simulate_processor_layout(&mut mock, [2, 2], [2, 2], [0, 1], [vec![1, 0], vec![0, 1]]);
            mock
        });
        let platform = BuildTargetPlatform::new(&*BINDINGS);
        let processors = platform.get_all_processors();
        assert_eq!(processors.len(), 4);

        // Group 0
        let p0 = &processors[0];
        assert_eq!(p0.group_index, 0);
        assert_eq!(p0.index_in_group, 0);
        assert_eq!(p0.memory_region_id, 0);
        assert_eq!(p0.efficiency_class, EfficiencyClass::Performance);

        let p1 = &processors[1];
        assert_eq!(p1.group_index, 0);
        assert_eq!(p1.index_in_group, 1);
        assert_eq!(p1.memory_region_id, 0);
        assert_eq!(p1.efficiency_class, EfficiencyClass::Efficiency);

        // Group 1
        let p2 = &processors[2];
        assert_eq!(p2.group_index, 1);
        assert_eq!(p2.index_in_group, 0);
        assert_eq!(p2.memory_region_id, 1);
        assert_eq!(p2.efficiency_class, EfficiencyClass::Efficiency);

        let p3 = &processors[3];
        assert_eq!(p3.group_index, 1);
        assert_eq!(p3.index_in_group, 1);
        assert_eq!(p3.memory_region_id, 1);
        assert_eq!(p3.efficiency_class, EfficiencyClass::Performance);
    }

    #[test]
    fn test_one_big_numa_two_small_nodes() {
        static BINDINGS: LazyLock<MockBindings> = LazyLock::new(|| {
            let mut mock = MockBindings::new();
            // Three groups: group 0 -> 4 Performance, group 1 -> 2 Efficiency, group 2 -> 2 Efficiency
            simulate_processor_layout(
                &mut mock,
                [4, 2, 2],
                [4, 2, 2],
                [0, 1, 2],
                [vec![1, 1, 1, 1], vec![0, 0], vec![0, 0]],
            );
            mock
        });
        let platform = BuildTargetPlatform::new(&*BINDINGS);
        let processors = platform.get_all_processors();
        assert_eq!(processors.len(), 8);

        // First 4 in group 0 => Performance
        for i in 0..4 {
            let p = &processors[i];
            assert_eq!(p.group_index, 0);
            assert_eq!(p.index_in_group, i as ProcessorIndexInGroup);
            assert_eq!(p.memory_region_id, 0);
            assert_eq!(p.efficiency_class, EfficiencyClass::Performance);
        }
        // Next 2 in group 1 => Efficiency
        for i in 4..6 {
            let p = &processors[i];
            assert_eq!(p.group_index, 1);
            assert_eq!(p.index_in_group, (i - 4) as ProcessorIndexInGroup);
            assert_eq!(p.memory_region_id, 1);
            assert_eq!(p.efficiency_class, EfficiencyClass::Efficiency);
        }
        // Last 2 in group 2 => Efficiency
        for i in 6..8 {
            let p = &processors[i];
            assert_eq!(p.group_index, 2);
            assert_eq!(p.index_in_group, (i - 6) as ProcessorIndexInGroup);
            assert_eq!(p.memory_region_id, 2);
            assert_eq!(p.efficiency_class, EfficiencyClass::Efficiency);
        }
    }

    #[test]
    fn test_one_active_one_inactive_numa_node() {
        static BINDINGS: LazyLock<MockBindings> = LazyLock::new(|| {
            let mut mock = MockBindings::new();
            // Group 0 -> inactive, Group 1 -> [Performance, Efficiency, Performance]
            simulate_processor_layout(&mut mock, [0, 3], [2, 3], [0, 1], [vec![], vec![1, 0, 1]]);
            mock
        });
        let platform = BuildTargetPlatform::new(&*BINDINGS);
        let processors = platform.get_all_processors();
        assert_eq!(processors.len(), 3);

        // Group 1 => [Perf, Eff, Perf]
        let p0 = &processors[0];
        assert_eq!(p0.group_index, 1);
        assert_eq!(p0.index_in_group, 0);
        assert_eq!(p0.memory_region_id, 1);
        assert_eq!(p0.efficiency_class, EfficiencyClass::Performance);

        let p1 = &processors[1];
        assert_eq!(p1.group_index, 1);
        assert_eq!(p1.index_in_group, 1);
        assert_eq!(p1.memory_region_id, 1);
        assert_eq!(p1.efficiency_class, EfficiencyClass::Efficiency);

        let p2 = &processors[2];
        assert_eq!(p2.group_index, 1);
        assert_eq!(p2.index_in_group, 2);
        assert_eq!(p2.memory_region_id, 1);
        assert_eq!(p2.efficiency_class, EfficiencyClass::Performance);
    }

    #[test]
    fn test_two_numa_nodes_some_inactive_processors() {
        static BINDINGS: LazyLock<MockBindings> = LazyLock::new(|| {
            let mut mock = MockBindings::new();
            // Group 0 -> Efficiency, Group 1 -> Performance
            simulate_processor_layout(&mut mock, [2, 2], [4, 4], [0, 1], [vec![0, 0], vec![1, 1]]);
            mock
        });
        let platform = BuildTargetPlatform::new(&*BINDINGS);
        let processors = platform.get_all_processors();
        assert_eq!(processors.len(), 4);

        // Group 0 => [Eff, Eff]
        let p0 = &processors[0];
        assert_eq!(p0.group_index, 0);
        assert_eq!(p0.index_in_group, 0);
        assert_eq!(p0.memory_region_id, 0);
        assert_eq!(p0.efficiency_class, EfficiencyClass::Efficiency);

        let p1 = &processors[1];
        assert_eq!(p1.group_index, 0);
        assert_eq!(p1.index_in_group, 1);
        assert_eq!(p1.memory_region_id, 0);
        assert_eq!(p1.efficiency_class, EfficiencyClass::Efficiency);

        // Group 1 => [Perf, Perf]
        let p2 = &processors[2];
        assert_eq!(p2.group_index, 1);
        assert_eq!(p2.index_in_group, 0);
        assert_eq!(p2.memory_region_id, 1);
        assert_eq!(p2.efficiency_class, EfficiencyClass::Performance);

        let p3 = &processors[3];
        assert_eq!(p3.group_index, 1);
        assert_eq!(p3.index_in_group, 1);
        assert_eq!(p3.memory_region_id, 1);
        assert_eq!(p3.efficiency_class, EfficiencyClass::Performance);
    }

    #[test]
    fn test_one_multi_group_numa_node_one_small() {
        static BINDINGS: LazyLock<MockBindings> = LazyLock::new(|| {
            let mut mock = MockBindings::new();
            // Group 0 -> [Perf, Perf], Group 1 -> [Eff, Eff], Group 2 -> [Perf]
            simulate_processor_layout(
                &mut mock,
                [2, 2, 1],
                [2, 2, 1],
                [0, 0, 1],
                [vec![1, 1], vec![0, 0], vec![1]],
            );
            mock
        });
        let platform = BuildTargetPlatform::new(&*BINDINGS);
        let processors = platform.get_all_processors();
        assert_eq!(processors.len(), 5);

        // Group 0 => [Performance, Performance]
        for (i, p) in processors.iter().enumerate() {
            if i < 2 {
                assert_eq!(p.group_index, 0);
                assert_eq!(p.memory_region_id, 0);
            } else if i < 4 {
                assert_eq!(p.group_index, 1);
                assert_eq!(p.memory_region_id, 0);
            } else {
                assert_eq!(p.group_index, 2);
                assert_eq!(p.memory_region_id, 1);
            }
            assert_eq!(p.index_in_group, i as ProcessorIndexInGroup % 2); // Groups of max 2.

            if i < 2 {
                assert_eq!(p.efficiency_class, EfficiencyClass::Performance);
            } else if i < 4 {
                assert_eq!(p.efficiency_class, EfficiencyClass::Efficiency);
            } else {
                assert_eq!(p.efficiency_class, EfficiencyClass::Performance);
            }
        }
    }

    #[test]
    fn test_insufficient_buffer_retry() {
        use mockall::Sequence;

        static BINDINGS: LazyLock<MockBindings> = LazyLock::new(|| {
            let mut mock = MockBindings::new();
            let mut seq = Sequence::new();

            // First iteration, probe (None) => size=64 => insufficient buffer
            mock.expect_get_logical_processor_information_ex()
                .times(1)
                .in_sequence(&mut seq)
                .withf(|rel, buf, _| rel == &RelationProcessorCore && buf.is_none())
                .returning(|_, _, returned_length| {
                    unsafe { *returned_length = 64 };
                    Err(windows::core::Error::from_hresult(
                        ERROR_INSUFFICIENT_BUFFER.to_hresult(),
                    ))
                });

            // First iteration, real call (Some(64)) => still insufficient => size=96
            mock.expect_get_logical_processor_information_ex()
                .times(1)
                .in_sequence(&mut seq)
                .withf(|rel, buf, _| rel == &RelationProcessorCore && buf.is_some())
                .returning(|_, _, returned_length| {
                    unsafe { *returned_length = 96 };
                    Err(windows::core::Error::from_hresult(
                        ERROR_INSUFFICIENT_BUFFER.to_hresult(),
                    ))
                });

            // Second iteration, probe (None) => size=128 => insufficient
            mock.expect_get_logical_processor_information_ex()
                .times(1)
                .in_sequence(&mut seq)
                .withf(|rel, buf, _| rel == &RelationProcessorCore && buf.is_none())
                .returning(|_, _, returned_length| {
                    unsafe { *returned_length = 128 };
                    Err(windows::core::Error::from_hresult(
                        ERROR_INSUFFICIENT_BUFFER.to_hresult(),
                    ))
                });

            // Second iteration, real call (Some(128)) => success
            mock.expect_get_logical_processor_information_ex()
                .times(1)
                .in_sequence(&mut seq)
                .withf(|rel, buf, _| rel == &RelationProcessorCore && buf.is_some())
                .returning(|_, _, returned_length| {
                    unsafe { *returned_length = 128 };
                    Ok(())
                });

            mock
        });

        let platform = BuildTargetPlatform::new(&*BINDINGS);
        let buffer = platform.get_logical_processor_information_raw(RelationProcessorCore);
        assert!(!buffer.is_empty(), "Expected final buffer to be filled");
    }

    /// Configures mock bindings to simulate a particular type of processor layout.
    ///
    /// The simulation is valid for one call to get_all_processors() only.
    fn simulate_processor_layout<const GROUP_COUNT: usize>(
        mock: &mut MockBindings,
        // If entry is 0 the group is not considered active. Such groups have to be at the end.
        // Presumably. Lack of machine to actually test this on limits our ability to be sure.
        group_active_counts: [ProcessorIndexInGroup; GROUP_COUNT],
        group_max_counts: [ProcessorIndexInGroup; GROUP_COUNT],
        group_memory_regions: [MemoryRegionId; GROUP_COUNT],
        // Only for the active processors in each group.
        efficiency_ratings_per_group: [Vec<u8>; GROUP_COUNT],
    ) {
        assert_eq!(group_active_counts.len(), group_max_counts.len());
        assert_eq!(group_active_counts.len(), group_memory_regions.len());
        assert_eq!(
            group_active_counts.len(),
            efficiency_ratings_per_group.len()
        );

        for (group_index, group_active_processors) in group_active_counts.iter().enumerate() {
            assert!(*group_active_processors <= group_max_counts[group_index]);

            assert_eq!(
                efficiency_ratings_per_group[group_index].len(),
                *group_active_processors as usize
            );
        }

        // Copy the data because we need to keep referencing it in the mock.
        let group_active_counts = Arc::new(group_active_counts.to_vec());
        let group_max_counts = Arc::new(group_max_counts.to_vec());
        let group_memory_regions = Arc::new(group_memory_regions.to_vec());
        let efficiency_ratings_per_group = Arc::new(
            efficiency_ratings_per_group
                .iter()
                .map(|x| x.to_vec())
                .collect_vec(),
        );

        simulate_get_counts(mock, &group_active_counts, &group_max_counts);
        simulate_get_numa_relations(mock, &group_memory_regions, &group_active_counts);
        simulate_get_core_info(mock, &group_active_counts, &efficiency_ratings_per_group);
    }

    fn simulate_get_counts(
        mock: &mut MockBindings,
        group_active_counts: &Arc<Vec<ProcessorIndexInGroup>>,
        group_max_counts: &Arc<Vec<ProcessorIndexInGroup>>,
    ) {
        let max_group_count = group_active_counts.len() as u16;

        // The "get maximum" we expect the platform to always report the same numbers for, so we
        // do not care how many times they are called. The "get active" we only want to call once
        // per processor group to avoid multiple calls into the platform getting different results.

        // Note that "get active group count" is not actually used - the code probes all groups
        // and just checks number of processors in group to identify if the group is active.
        mock.expect_get_maximum_processor_group_count()
            .return_const(max_group_count);

        mock.expect_get_active_processor_count()
            .times(group_max_counts.len())
            .returning({
                let group_active_counts = Arc::clone(group_active_counts);

                move |group_number| group_active_counts[group_number as usize] as u32
            });

        mock.expect_get_maximum_processor_count().returning({
            let group_max_counts = Arc::clone(group_max_counts);

            move |group_number| group_max_counts[group_number as usize] as u32
        });
    }

    fn simulate_get_numa_relations(
        mock: &mut MockBindings,
        group_memory_regions: &Arc<Vec<u32>>,
        group_active_counts: &Arc<Vec<ProcessorIndexInGroup>>,
    ) {
        // NB! WARNING! size_of<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>() is not correct! This is
        // because there is a dynamically-sized array in there that in the Rust bindings is just
        // hardcoded to size 1. The real size in memory may be greater than size_of()!
        //
        // We define here some additional buffer to add on top of the "known" memory.
        const MAX_ADDITIONAL_PROCESSOR_GROUPS: usize = 32;

        #[repr(C)]
        struct ExpandedLogicalProcessorInformation {
            root: SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
            // We do not necessarily use it in-place, this is a fake member
            // just to make room in the sizeof calculation.
            extra_buffer: [GROUP_AFFINITY; MAX_ADDITIONAL_PROCESSOR_GROUPS],
        }

        // There will be a call to get the groups that are members of each NUMA node.
        // The NUMA response is structured by memory region, not by processor group.
        // Therefore we need to also structure our data this way.
        let memory_regions_to_group_indexes = group_memory_regions
            .iter()
            .enumerate()
            .map(|(group_index, memory_region_index)| (memory_region_index, group_index))
            .into_group_map();

        let memory_region_count = memory_regions_to_group_indexes.len();

        let memory_region_responses = memory_regions_to_group_indexes
            .into_iter()
            .map(|(memory_region_index, group_indexes)| {
                // We made some additional space in the struct to allow for more groups
                // but there is still an upper limit to what we can fit, so be careful.
                assert!(group_indexes.len() <= MAX_ADDITIONAL_PROCESSOR_GROUPS);

                let mut response = ExpandedLogicalProcessorInformation {
                    root: SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX {
                        // This is always RelationNumaNode, even if RelationNumaNodeEx is requested.
                        // RelationNumaNodeEx simply allows the group count to be more than 1.
                        Relationship: RelationNumaNode,
                        Size: mem::size_of::<ExpandedLogicalProcessorInformation>() as u32,
                        ..Default::default()
                    },
                    extra_buffer: [Default::default(); MAX_ADDITIONAL_PROCESSOR_GROUPS],
                };

                // SAFETY: We define RelationNumaNode above so this dereference is valid.
                let response_numa = unsafe { &mut response.root.Anonymous.NumaNode };

                response_numa.NodeNumber = *memory_region_index;
                response_numa.GroupCount = group_indexes.len() as u16;

                // The type definition just has an array of 1 here (not correct), so we write
                // through pointers to fill each array element. We have to do this dance of starting
                // out borrow from the root of the document we are returning to prove to the
                // compiler and Miri that we have the rights to write beyond the 1-element array.
                let document = &raw mut response;

                // SAFETY: We define GroupCount above so this dereference is valid.
                let mut group_mask = unsafe {
                    document
                        .byte_add(offset_of!(
                            ExpandedLogicalProcessorInformation,
                            root.Anonymous.NumaNode.Anonymous.GroupMask
                        ))
                        .cast::<GROUP_AFFINITY>()
                };

                for group_index in group_indexes {
                    // Make a usize here with the number of bits set to the number of active processors.
                    let mask = (1 << group_active_counts[group_index]) - 1;

                    let group_data = GROUP_AFFINITY {
                        Group: group_index as u16,
                        // The mask has a bit set for each processor that exists in the group.
                        // We only set the bits for the processors that are active.
                        Mask: mask,
                        ..Default::default()
                    };

                    // SAFETY: We define GroupCount above and ensured that we have enough space
                    // via MAX_ADDITIONAL_PROCESSOR_GROUPS so this is valid.
                    unsafe {
                        group_mask.write(group_data);
                    }

                    // SAFETY: We define GroupCount above and ensured that we have enough space
                    // via MAX_ADDITIONAL_PROCESSOR_GROUPS so this is valid.
                    group_mask = unsafe { group_mask.add(1) };
                }

                response
            })
            .collect_vec();

        let memory_region_response_buffer = NativeBuffer::from_vec(memory_region_responses);
        let memory_region_response_len = memory_region_response_buffer.len();

        let mut seq = Sequence::new();

        // This will actually be two calls, one for the size probe and one for the real data.
        mock.expect_get_logical_processor_information_ex()
            .times(1)
            .in_sequence(&mut seq)
            .withf(|relationship_type, buffer, _| {
                *relationship_type == RelationNumaNodeEx && buffer.is_none()
            })
            .returning(move |_, _, returned_length| {
                // SAFETY: Caller must guarantee that the pointer is valid for use.
                unsafe {
                    *returned_length = memory_region_response_len as u32;
                }

                Err(windows::core::Error::from_hresult(HRESULT::from_win32(
                    ERROR_INSUFFICIENT_BUFFER.0,
                )))
            });

        mock.expect_get_logical_processor_information_ex()
            .times(1)
            .in_sequence(&mut seq)
            .withf(|relationship_type, buffer, _| {
                *relationship_type == RelationNumaNodeEx && buffer.is_some()
            })
            .returning_st(move |_, buffer, returned_length| {
                // SAFETY: Caller must guarantee that the pointer is valid for use.
                unsafe {
                    *returned_length = memory_region_response_len as u32;
                }

                // SAFETY: Caller must guarantee that the pointer is valid for use.
                unsafe {
                    buffer
                        .unwrap()
                        .cast::<ExpandedLogicalProcessorInformation>()
                        .copy_from_nonoverlapping(
                            memory_region_response_buffer.as_ptr().as_ptr(),
                            memory_region_count,
                        );
                }

                Ok(())
            });
    }

    fn simulate_get_core_info(
        mock: &mut MockBindings,
        group_active_counts: &Arc<Vec<ProcessorIndexInGroup>>,
        efficiency_ratings_per_group: &Arc<Vec<Vec<u8>>>,
    ) {
        // Next we define the "get core info" simulation, used to access metadata of each processor.
        // Specifically, this provides the efficiency class. The other info is currently unused.

        // In case of RelationProcessorCore, we do not use any dynamically sized arrays, which
        // keeps the size logic a bit simpler than with the NUMA nodes response. We have one entry
        // in the response for each processor. For the sake of simplicity (the code does not care)
        // we pretend that each core is 1 logical processor (no SMT).
        let response_entries = group_active_counts
            .iter()
            .enumerate()
            .flat_map(|(group_index, group_active_count)| {
                (0..*group_active_count).map({
                    let efficiency_ratings_per_group = Arc::clone(efficiency_ratings_per_group);

                    move |index_in_group| {
                        let mut entry = SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX {
                            Relationship: RelationProcessorCore,
                            Size: mem::size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>() as u32,
                            ..Default::default()
                        };

                        // SAFETY: We define RelationProcessorCore above so this dereference is valid.
                        let entry_processor = unsafe { &mut entry.Anonymous.Processor };

                        entry_processor.EfficiencyClass =
                            efficiency_ratings_per_group[group_index][index_in_group as usize];

                        entry_processor.GroupCount = 1;
                        entry_processor.GroupMask[0].Group = group_index as u16;
                        entry_processor.GroupMask[0].Mask = 1 << index_in_group;

                        entry
                    }
                })
            })
            .collect_vec();

        let core_info_response_entry_count = response_entries.len();
        let core_info_response_buffer = NativeBuffer::from_vec(response_entries);
        let core_info_response_len = core_info_response_buffer.len();

        let mut seq = Sequence::new();

        // This will actually be two calls, one for the size probe and one for the real data.
        mock.expect_get_logical_processor_information_ex()
            .times(1)
            .in_sequence(&mut seq)
            .withf(|relationship_type, buffer, _| {
                *relationship_type == RelationProcessorCore && buffer.is_none()
            })
            .returning(move |_, _, returned_length| {
                // SAFETY: Caller must guarantee that the pointer is valid for use.
                unsafe {
                    *returned_length = core_info_response_len as u32;
                }

                Err(windows::core::Error::from_hresult(HRESULT::from_win32(
                    ERROR_INSUFFICIENT_BUFFER.0,
                )))
            });

        mock.expect_get_logical_processor_information_ex()
            .times(1)
            .in_sequence(&mut seq)
            .withf(|relationship_type, buffer, _| {
                *relationship_type == RelationProcessorCore && buffer.is_some()
            })
            .returning_st(move |_, buffer, returned_length| {
                // SAFETY: Caller must guarantee that the pointer is valid for use.
                unsafe {
                    *returned_length = core_info_response_len as u32;
                }

                // SAFETY: Caller must guarantee that the pointer is valid for use.
                unsafe {
                    buffer.unwrap().copy_from_nonoverlapping(
                        core_info_response_buffer.as_ptr().as_ptr(),
                        core_info_response_entry_count,
                    );
                }

                Ok(())
            });
    }

    #[test]
    fn test_pin_current_thread_to_single_processor() {
        static BINDINGS: LazyLock<MockBindings> = LazyLock::new(|| {
            let mut mock = MockBindings::new();
            simulate_processor_layout(&mut mock, [1], [1], [0], [vec![0]]);
            mock.expect_get_current_thread()
                .return_const_st(HANDLE::default());
            mock.expect_set_thread_group_affinity()
                .withf(|thread, affinity, _| {
                    *thread == HANDLE::default()
                        && unsafe { (**affinity).Group == 0 && (**affinity).Mask == 1 }
                })
                .return_const(BOOL::from(true));
            mock
        });

        let platform = BuildTargetPlatform::new(&*BINDINGS);
        let processors = platform.get_all_processors();
        platform.pin_current_thread_to(&processors);
    }

    #[test]
    fn test_pin_current_thread_to_multiple_processors() {
        static BINDINGS: LazyLock<MockBindings> = LazyLock::new(|| {
            let mut mock = MockBindings::new();
            simulate_processor_layout(&mut mock, [2], [2], [0], [vec![0, 0]]);
            mock.expect_get_current_thread()
                .return_const_st(HANDLE::default());
            mock.expect_set_thread_group_affinity()
                .withf(|thread, affinity, _| {
                    *thread == HANDLE::default()
                        && unsafe { (**affinity).Group == 0 && (**affinity).Mask == 3 }
                })
                .return_const(BOOL::from(true));
            mock
        });

        let platform = BuildTargetPlatform::new(&*BINDINGS);
        let processors = platform.get_all_processors();
        platform.pin_current_thread_to(&processors);
    }

    #[test]
    fn test_pin_current_thread_to_multiple_groups() {
        static BINDINGS: LazyLock<MockBindings> = LazyLock::new(|| {
            let mut mock = MockBindings::new();
            simulate_processor_layout(&mut mock, [1, 1], [1, 1], [0, 1], [vec![0], vec![0]]);
            mock.expect_get_current_thread()
                .return_const_st(HANDLE::default());
            mock.expect_set_thread_group_affinity()
                .withf(|thread, affinity, _| {
                    *thread == HANDLE::default()
                        && unsafe { (**affinity).Group == 0 && (**affinity).Mask == 1 }
                })
                .return_const(BOOL::from(true));
            mock.expect_set_thread_group_affinity()
                .withf(|thread, affinity, _| {
                    *thread == HANDLE::default()
                        && unsafe { (**affinity).Group == 1 && (**affinity).Mask == 1 }
                })
                .return_const(BOOL::from(true));
            mock
        });

        let platform = BuildTargetPlatform::new(&*BINDINGS);
        let processors = platform.get_all_processors();
        platform.pin_current_thread_to(&processors);
    }

    #[test]
    fn test_pin_current_thread_to_efficiency_processors() {
        static BINDINGS: LazyLock<MockBindings> = LazyLock::new(|| {
            let mut mock = MockBindings::new();
            // Group 0 -> [Performance, Efficiency], Group 1 -> [Efficiency, Performance]
            simulate_processor_layout(&mut mock, [2, 2], [2, 2], [0, 1], [vec![1, 0], vec![0, 1]]);
            mock.expect_get_current_thread()
                .return_const_st(HANDLE::default());
            mock.expect_set_thread_group_affinity()
                .withf(|thread, affinity, _| {
                    *thread == HANDLE::default()
                        && unsafe { (**affinity).Group == 0 && (**affinity).Mask == 2 }
                })
                .return_const(BOOL::from(true));
            mock.expect_set_thread_group_affinity()
                .withf(|thread, affinity, _| {
                    *thread == HANDLE::default()
                        && unsafe { (**affinity).Group == 1 && (**affinity).Mask == 1 }
                })
                .return_const(BOOL::from(true));
            mock
        });

        let platform = BuildTargetPlatform::new(&*BINDINGS);
        let processors = platform.get_all_processors();
        let efficiency_processors = NonEmpty::from_vec(
            processors
                .iter()
                .filter(|p| p.efficiency_class == EfficiencyClass::Efficiency)
                .collect_vec(),
        )
        .unwrap();
        platform.pin_current_thread_to(&efficiency_processors);
    }
}

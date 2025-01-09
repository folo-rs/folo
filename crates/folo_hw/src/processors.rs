use std::{
    collections::{HashMap, HashSet},
    fmt::{Debug, Display},
    num::NonZeroUsize,
    sync::LazyLock,
    thread,
};

use itertools::Itertools;
use nonempty::{nonempty, NonEmpty};
use rand::{
    seq::{IteratorRandom, SliceRandom},
    thread_rng, Rng, RngCore,
};
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

// https://github.com/cloudhead/nonempty/issues/68
extern crate alloc;

/// Differentiates processors by their efficiency class, allowing work requiring high
/// performance to be placed on the most performant processors at the expense of energy usage.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum EfficiencyClass {
    /// A processor that is optimized for energy efficiency at the expense of performance.
    Efficiency,

    /// A processor that is optimized for performance at the expense of energy efficiency.
    Performance,
}

type ProcessorGroupIndex = u16;
type ProcessorIndexInGroup = u8;
type ProcessorGlobalIndex = u32;
type MemoryRegionIndex = u32;

/// A processor present on the system and available to the current process.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Processor {
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

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
struct MemoryRegion {
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
/// fine-grained selection criteria via `ProcessorSet::builder()`. You can use
/// `ProcessorSet::to_builder()` to narrow down an existing set further.
///
/// One you have a `ProcessorSet`, you can iterate over `ProcessorSet::processors()`
/// to inspect the individual processors.
///
/// # Changes at runtime
///
/// It is possible that a system will have processors added or removed at runtime. This is not
/// supported - any hardware changes made at runtime will not be visible to the `ProcessorSet`
/// instances. Operations attempted on removed processors may fail with an error or panic. Added
/// processors will not be considered a member of any set.
#[derive(Clone, Debug)]
pub struct ProcessorSet {
    processors: Box<[Processor]>,
}

impl ProcessorSet {
    /// Gets a `ProcessorSet` referencing all processors on the system.
    pub fn all() -> &'static Self {
        &ALL_PROCESSORS
    }

    pub fn builder() -> ProcessorSetBuilder {
        ProcessorSetBuilder::default()
    }

    /// Returns a `ProcessorSetBuilder` that considers all processors in the current set as
    /// candidates, to be used to further narrow down the set to a specific subset.
    pub fn to_builder(&self) -> ProcessorSetBuilder {
        ProcessorSetBuilder::default().filter(|p| self.processors.contains(p))
    }

    fn new(processors: NonEmpty<Processor>) -> Self {
        Self {
            processors: Into::<Vec<_>>::into(processors).into_boxed_slice(),
        }
    }

    #[expect(clippy::len_without_is_empty)] // Never empty by definition.
    pub fn len(&self) -> usize {
        self.processors.len()
    }

    pub fn processors(&self) -> impl Iterator<Item = &Processor> + '_ {
        self.processors.iter()
    }

    /// Modifies the affinity of the current thread to execute
    /// only on the processors in this processor set.
    ///
    /// # Behavior with multiple processors
    ///
    /// If multiple processors are present in the processor set, they might not be evenly used.
    /// An arbitrary processor may be preferentially used, with others used only when the preferred
    /// processor is otherwise busy. This behavior is not configurable.
    pub fn pin_current_thread_to(&self) {
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
            for processor in self
                .processors
                .iter()
                .filter(|p| p.group_index == group_index as ProcessorGroupIndex)
            {
                affinity.Mask |= 1 << processor.index_in_group;
            }

            // We do not expect this to ever fail - these flags should be settable
            // for all processor groups at all times for the current thread, even if
            // the hardware dynamically changes (the affinity masks are still valid, after all,
            // even if they point to non-existing processors).
            unsafe { SetThreadGroupAffinity(current_thread, &affinity, None) }
                .expect("SetThreadGroupAffinity failed unexpectedly");
        }
    }

    /// Spawns one thread for each processor in the set, pinned to that processor,
    /// providing the target processor information to the thread entry point.
    pub fn spawn_threads<E, R>(&self, entrypoint: E) -> Box<[thread::JoinHandle<R>]>
    where
        E: Fn(Processor) -> R + Send + Clone + 'static,
        R: Send + 'static,
    {
        self.processors()
            .map(|p| {
                let processor = *p;
                let entrypoint = entrypoint.clone();

                thread::spawn(move || {
                    let set = ProcessorSet::from(processor);
                    set.pin_current_thread_to();
                    entrypoint(processor)
                })
            })
            .collect::<Vec<_>>()
            .into_boxed_slice()
    }

    /// Spawns a thread pinned to all processors in the set.
    ///
    /// # Behavior with multiple processors
    ///
    /// If multiple processors are present in the processor set, they might not be evenly used.
    /// An arbitrary processor may be preferentially used, with others used only when the preferred
    /// processor is otherwise busy. This behavior is not configurable.
    pub fn spawn_thread<E, R>(&self, entrypoint: E) -> thread::JoinHandle<R>
    where
        E: Fn(ProcessorSet) -> R + Send + 'static,
        R: Send + 'static,
    {
        let set = self.clone();
        thread::spawn(move || {
            set.pin_current_thread_to();
            entrypoint(set)
        })
    }
}

impl From<Processor> for ProcessorSet {
    fn from(value: Processor) -> Self {
        ProcessorSet::new(nonempty![value])
    }
}

#[derive(Clone, Debug, Default)]
pub struct ProcessorSetBuilder {
    processor_type_selector: ProcessorTypeSelector,
    memory_region_selector: MemoryRegionSelector,

    except_global_indexes: HashSet<ProcessorGlobalIndex>,
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

    /// Uses a predicate to identify processors that are valid candidates for the set.
    pub fn filter(mut self, predicate: impl Fn(&Processor) -> bool) -> Self {
        for processor in ALL_PROCESSORS.processors() {
            if !predicate(processor) {
                self.except_global_indexes.insert(processor.global_index);
            }
        }

        self
    }

    /// Removes processors from the set of candidates. Useful to help ensure that different
    /// workloads get placed on different processors.
    pub fn except<'a, I>(mut self, processors: I) -> Self
    where
        I: IntoIterator<Item = &'a Processor>,
    {
        for processor in processors {
            self.except_global_indexes.insert(processor.global_index);
        }

        self
    }

    /// Picks a specific number of processors from the set of candidates and returns a
    /// processor set with the requested number of processors that match specified criteria.
    ///
    /// Returns `None` if there were not enough matching processors to satisfy the request.
    pub fn take(self, count: NonZeroUsize) -> Option<ProcessorSet> {
        let candidates = self.candidates_by_memory_region();

        if candidates.is_empty() {
            // No candidates to choose from - everything was filtered out.
            return None;
        }

        let processors = match self.memory_region_selector {
            MemoryRegionSelector::PreferSame => {
                // We will start decrementing it to zero.
                let count = count.get();

                // We shuffle the memory regions and just take random processors from the first
                // one remaining until the count has been satisfied.
                let mut remaining_memory_regions = candidates.keys().copied().collect::<Vec<_>>();
                let mut processors: Vec<Processor> = Vec::with_capacity(count);

                while processors.len() < count {
                    if remaining_memory_regions.is_empty() {
                        // Not enough candidates remaining to satisfy request.
                        return None;
                    }

                    let (i, memory_region) = remaining_memory_regions
                        .iter()
                        .enumerate()
                        .choose(&mut rand::thread_rng())
                        .expect("we picked a random existing index - element must exist");

                    let processors_in_region = candidates.get(memory_region).expect(
                        "we picked an existing key for an existing HashSet - the values must exist",
                    );

                    remaining_memory_regions.remove(i);

                    // There might not be enough to fill the request, which is fine.
                    let choose_count = count.min(processors_in_region.len());

                    let region_processors = processors_in_region
                        .choose_multiple(&mut rand::thread_rng(), choose_count)
                        .copied();

                    processors.extend(region_processors);
                }

                processors
            }
            MemoryRegionSelector::RequireSame => {
                // We filter out memory regions that do not have enough candidates and pick a
                // random one from the remaining, then picking a random `count` processors.
                let remaining_memory_regions = candidates
                    .iter()
                    .filter_map(|(region, processors)| {
                        if processors.len() < count.get() {
                            return None;
                        }

                        Some(region)
                    })
                    .collect_vec();

                let memory_region = remaining_memory_regions.choose(&mut rand::thread_rng())?;

                let processors = candidates.get(memory_region).expect(
                    "we picked an existing key for an existing HashSet - the values must exist",
                );

                processors
                    .choose_multiple(&mut rand::thread_rng(), count.get())
                    .copied()
                    .copied()
                    .collect_vec()
            }
            MemoryRegionSelector::RequireDifferent => {
                // We pick random `count` memory regions and a random processor from each.

                if candidates.len() < count.get() {
                    // Not enough memory regions to satisfy request.
                    return None;
                }

                candidates
                    .iter()
                    .choose_multiple(&mut thread_rng(), count.get())
                    .into_iter()
                    .map(|(region, processors)| {
                        processors
                            .iter()
                            .choose(&mut thread_rng())
                            .copied()
                            .copied()
                            .expect(
                                "we are picking one item from a non-empty list - item must exist",
                            )
                    })
                    .collect_vec()
            }
        };

        Some(ProcessorSet::new(NonEmpty::from_vec(processors)?))
    }

    /// Returns a processor set with all processors that match the specified criteria.
    ///
    /// If multiple mutually exclusive sets are a match, returns an arbitrary one of them.
    /// For example, if specifying only a "same memory region" constraint, it will return all
    /// the processors in an arbitrary (potentially even random) memory region.
    ///
    /// Returns `None` if there were no matching processors to satisfy the request.
    pub fn take_all(self) -> Option<ProcessorSet> {
        let candidates = self.candidates_by_memory_region();

        if candidates.is_empty() {
            // No candidates to choose from - everything was filtered out.
            return None;
        }

        let processors = match self.memory_region_selector {
            MemoryRegionSelector::PreferSame => {
                // We return all processors in all memory regions.
                candidates
                    .values()
                    .flat_map(|x| x.iter().copied())
                    .copied()
                    .collect()
            }
            MemoryRegionSelector::RequireSame => {
                // We return all processors in a random memory region.
                // The candidate set only contains memory regions with at least 1 processor.
                let memory_region = candidates
                    .keys()
                    .choose(&mut rand::thread_rng())
                    .expect("we picked a random existing index - element must exist");

                let processors = candidates.get(memory_region).expect(
                    "we picked an existing key for an existing HashSet - the values must exist",
                );

                processors.iter().copied().copied().collect()
            }
            MemoryRegionSelector::RequireDifferent => {
                // We return a random processor from each memory region.
                // The candidate set only contains memory regions with at least 1 processor.
                let processors = candidates.values().map(|processors| {
                    processors
                        .choose(&mut rand::thread_rng())
                        .copied()
                        .expect("we picked a random item from a non-empty list - item must exist")
                });

                processors.copied().collect()
            }
        };

        Some(ProcessorSet::new(NonEmpty::from_vec(processors)?))
    }

    // Returns candidates grouped by memory region, with each returned memory region having at
    // least one candidate processor.
    fn candidates_by_memory_region(&self) -> HashMap<MemoryRegionIndex, Vec<&Processor>> {
        ALL_PROCESSORS
            .processors()
            .filter_map(move |p| {
                if self.except_global_indexes.contains(&p.global_index) {
                    return None;
                }

                let is_acceptable_type = match self.processor_type_selector {
                    ProcessorTypeSelector::Any => true,
                    ProcessorTypeSelector::Performance => {
                        p.efficiency_class == EfficiencyClass::Performance
                    }
                    ProcessorTypeSelector::Efficiency => {
                        p.efficiency_class == EfficiencyClass::Efficiency
                    }
                };

                if !is_acceptable_type {
                    return None;
                }

                Some((p.memory_region_index, p))
            })
            .into_group_map()
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
    ///
    /// Every system is guaranteed to have at least one performance processor.
    /// If all processors are of the same performance class,
    /// they are all considered to be performance processors.
    Performance,

    /// Only efficiency processors (slower, energy-efficient) are valid candidates.
    ///
    /// There is no guarantee that any efficiency processors are present on the system.
    Efficiency,
}

static ALL_PROCESSORS: LazyLock<ProcessorSet> = LazyLock::new(get_all);

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

fn get_all() -> ProcessorSet {
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
        assert_eq!(info.Relationship, RelationNumaNode);

        let details = unsafe { &info.Anonymous.NumaNode };

        let numa_node_number = details.NodeNumber;

        // In the struct definition, this is a 1-element array because Rust has no notion
        // of dynamic-size arrays. We use pointer arithmetic to access the real array elements.
        // SAFETY: RelationNumaNodeEx guarantees that this union member is present.
        let mut group_mask_array = unsafe { details.Anonymous.GroupMasks }.as_ptr();

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

    ProcessorSet::new(NonEmpty::from_vec(processors).expect(
        "we are returning all processors on the system - obviously there must be at least one",
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use super::*;

    // TODO: We need a "give up" mechanism so tests do not hang forever if they fail.

    const ONE: NonZeroUsize = NonZeroUsize::new(1).unwrap();

    #[test]
    fn spawn_on_any_processor() {
        let done = Arc::new(Barrier::new(2));

        let set = ProcessorSet::all();
        set.spawn_thread({
            let done = Arc::clone(&done);

            move |_| {
                done.wait();
            }
        });

        done.wait();
    }

    #[test]
    fn spawn_on_every_processor() {
        let processor_count: usize = get_processor_group_active_sizes()
            .iter()
            .map(|&x| x as usize)
            .sum();

        let done = Arc::new(Barrier::new(processor_count + 1));

        let set = ProcessorSet::all();
        set.spawn_threads({
            let done = Arc::clone(&done);

            move |_| {
                done.wait();
            }
        });

        done.wait();
    }

    #[test]
    fn filter_by_memory_region_real() {
        // We know there is at least one memory region, so these must succeed.
        ProcessorSet::builder()
            .same_memory_region()
            .take_all()
            .unwrap();
        ProcessorSet::builder()
            .same_memory_region()
            .take(ONE)
            .unwrap();
        ProcessorSet::builder()
            .different_memory_regions()
            .take_all()
            .unwrap();
        ProcessorSet::builder()
            .different_memory_regions()
            .take(ONE)
            .unwrap();
    }

    #[test]
    fn filter_by_efficiency_class_real() {
        // There must be at least one.
        ProcessorSet::builder()
            .performance_processors_only()
            .take_all()
            .unwrap();
        ProcessorSet::builder()
            .performance_processors_only()
            .take(ONE)
            .unwrap();

        // There might not be any. We just try resolving it and ignore the result.
        // As long as it does not panic, we are good.
        ProcessorSet::builder()
            .efficiency_processors_only()
            .take_all();
        ProcessorSet::builder()
            .efficiency_processors_only()
            .take(ONE);
    }

    #[test]
    fn filter_in_all() {
        // If we filter in all processors, we should get all of them.
        let processors = ProcessorSet::builder()
            .filter(|_| true)
            .take_all()
            .unwrap();

        assert_eq!(processors.len(), ALL_PROCESSORS.len());
    }

    #[test]
    fn filter_out_all() {
        // If we filter out all processors, there should be nothing left.
        assert!(ProcessorSet::builder()
            .filter(|_| false)
            .take_all()
            .is_none());
    }

    #[test]
    fn except_all() {
        // If we exclude all processors, there should be nothing left.
        assert!(ProcessorSet::builder()
            .except(ProcessorSet::all().processors())
            .take_all()
            .is_none());
    }
}

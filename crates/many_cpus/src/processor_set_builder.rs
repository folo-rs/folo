use std::{collections::VecDeque, fmt::Debug, num::NonZeroUsize};

use foldhash::HashMapExt;
use foldhash::{HashMap, HashSet, HashSetExt};
use itertools::Itertools;
use nonempty::NonEmpty;
use rand::prelude::*;
use rand::rng;

use crate::HardwareTrackerClientFacade;
use crate::{
    EfficiencyClass, MemoryRegionId, Processor, ProcessorId, ProcessorSet,
    pal::{Platform, PlatformFacade},
};

/// Builds a [`ProcessorSet`] based on specified criteria. The default criteria include all
/// available processors.
///
/// You can obtain a builder via [`ProcessorSet::builder()`] or from an existing processor set
/// via [`ProcessorSet::to_builder()`].
///
/// # Process-scoped processor affinity
///
/// Operating systems provide various ways to start processes with a specific processor affinity,
/// such as via the `start /affinity 0x1` command on Windows, the `taskset 0x1` command on Linux
/// or via the cgroups mechanism.
///
/// Some of these techniques only specify a default processor affinity, while others are hard
/// constraints.
///
/// Due to limitations in the design of the underlying platform APIs, how `ProcessorSetBuilder`
/// interacts with these constraints depends on the operating system and the specific way in which
/// the constraint was defined. We do not guarantee that any specific process-level affinity
/// configuration on any specific operating system is or is not considered a constraint.
///
/// Typical behavior resembles the following:
///
/// * On Linux, process-scoped affinity is treated as a hard constraint - only processors allowed by
///   the affinity configuration are considered available for use.
/// * On Windows, process-scoped affinity is ignored and all processors present on the system are
///   considered available for use.
///
/// For maximum compatibility, do not rely on external configuration to limit the processors in use
/// and assume that `ProcessorSetBuilder` may see all processors present on the system.
///
/// If your code is running in a thread where you can be certain that processor affinity has not
/// been customized, you can inherit the allowed processor set from the current thread to get a set
/// of processors that matches the defaults the platform would prefer the process to use.
///
/// # Inheriting processor affinity from current thread
///
/// By default, the processor affinity of the current thread is ignored when building a processor
/// set, as this type may be used from a thread with a different processor affinity than the threads
/// one wants to configure.
///
/// However, if you do wish to inherit the processor affinity from the current thread, you may do
/// so by calling [`.where_available_for_current_thread()`][1] on the builder. This filters out all
/// processors that the current thread is not configured to execute on.
///
/// [1]: ProcessorSetBuilder::where_available_for_current_thread
#[derive(Clone, Debug)]
pub struct ProcessorSetBuilder {
    processor_type_selector: ProcessorTypeSelector,
    memory_region_selector: MemoryRegionSelector,

    except_indexes: HashSet<ProcessorId>,

    // ProcessorSet needs this because it needs to inform the tracker
    // about any changes to the pinning status of the current thread.
    // We just carry it around and pass to any processor set we create.
    tracker_client: HardwareTrackerClientFacade,

    pal: PlatformFacade,
}

impl ProcessorSetBuilder {
    pub fn new() -> Self {
        Self::with_internals(HardwareTrackerClientFacade::real(), PlatformFacade::real())
    }

    pub(crate) fn with_internals(
        tracker_client: HardwareTrackerClientFacade,
        pal: PlatformFacade,
    ) -> Self {
        Self {
            processor_type_selector: ProcessorTypeSelector::Any,
            memory_region_selector: MemoryRegionSelector::Any,
            except_indexes: HashSet::new(),
            tracker_client,
            pal,
        }
    }

    /// Requires that all processors in the set be marked as [performance processors][1].
    ///
    /// [1]: EfficiencyClass::Performance
    pub fn performance_processors_only(mut self) -> Self {
        self.processor_type_selector = ProcessorTypeSelector::Performance;
        self
    }

    /// Requires that all processors in the set be marked as [efficiency processors][1].
    ///
    /// [1]: EfficiencyClass::Efficiency
    pub fn efficiency_processors_only(mut self) -> Self {
        self.processor_type_selector = ProcessorTypeSelector::Efficiency;
        self
    }

    /// Requires that all processors in the set be from different memory regions, selecting a
    /// maximum of 1 processor from each memory region.
    pub fn different_memory_regions(mut self) -> Self {
        self.memory_region_selector = MemoryRegionSelector::RequireDifferent;
        self
    }

    /// Requires that all processors in the set be from the same memory region.
    pub fn same_memory_region(mut self) -> Self {
        self.memory_region_selector = MemoryRegionSelector::RequireSame;
        self
    }

    /// Declares a preference that all processors in the set be from different memory regions,
    /// though will select multiple processors from the same memory region as needed to satisfy
    /// the requested processor count (while keeping the spread maximal).
    pub fn prefer_different_memory_regions(mut self) -> Self {
        self.memory_region_selector = MemoryRegionSelector::PreferDifferent;
        self
    }

    /// Declares a preference that all processors in the set be from the same memory region,
    /// though will select processors from different memory regions as needed to satisfy the
    /// requested processor count (while keeping the spread minimal).
    pub fn prefer_same_memory_region(mut self) -> Self {
        self.memory_region_selector = MemoryRegionSelector::PreferSame;
        self
    }

    /// Uses a predicate to identify processors that are valid candidates for building the
    /// processor set, with a return value of `bool` indicating that a processor is a valid
    /// candidate for selection into the set.
    ///
    /// The candidates are passed to this function without necessarily first considering all other
    /// conditions - even if this predicate returns `true`, the processor may end up being filtered
    /// out by other conditions. Conversely, some candidates may already be filtered out before
    /// being passed to this predicate.
    pub fn filter(mut self, predicate: impl Fn(&Processor) -> bool) -> Self {
        for processor in self.all_processors() {
            if !predicate(&processor) {
                self.except_indexes.insert(processor.id());
            }
        }

        self
    }

    /// Removes specific processors from the set of candidates.
    pub fn except<'a, I>(mut self, processors: I) -> Self
    where
        I: IntoIterator<Item = &'a Processor>,
    {
        for processor in processors {
            self.except_indexes.insert(processor.id());
        }

        self
    }

    /// Removes processors from the set of candidates if they are not available for use by the
    /// current thread.
    ///
    /// This is a convenient way to identify the set of processors the platform prefers a process
    /// to use when called from a non-customized thread such as the `main()` entrypoint.
    pub fn where_available_for_current_thread(mut self) -> Self {
        let current_thread_processors = self.pal.current_thread_processors();

        for processor in self.all_processors() {
            if !current_thread_processors.contains(&processor.id()) {
                self.except_indexes.insert(processor.id());
            }
        }

        self
    }

    /// Creates a processor set with a specific number of processors that match the
    /// configured criteria.
    ///
    /// If multiple candidate sets are a match, returns an arbitrary one of them. For example, if
    /// there are six valid candidate processors then `take(4)` may return any four of them.
    ///
    /// Returns `None` if there were not enough candidate processors to satisfy the request.
    pub fn take(self, count: NonZeroUsize) -> Option<ProcessorSet> {
        let candidates = self.candidates_by_memory_region();

        if candidates.is_empty() {
            // No candidates to choose from - everything was filtered out.
            return None;
        }

        let processors = match self.memory_region_selector {
            MemoryRegionSelector::Any => {
                // We do not care about memory regions, so merge into one big happy family and
                // pick a random `count` processors from it. As long as there is enough!
                let all_processors = candidates
                    .values()
                    .flat_map(|x| x.iter().cloned())
                    .collect::<Vec<_>>();

                if all_processors.len() < count.get() {
                    // Not enough processors to satisfy request.
                    return None;
                }

                all_processors
                    .choose_multiple(&mut rng(), count.get())
                    .cloned()
                    .collect_vec()
            }
            MemoryRegionSelector::PreferSame => {
                // We will start decrementing it to zero.
                let count = count.get();

                // We shuffle the memory regions and sort them by size, so if there are memory
                // regions with different numbers of candidates we will prefer the ones with more,
                // although we will consider all memory regions with at least 'count' candidates
                // as equal in sort order to avoid needlessly preferring giant memory regions.
                let mut remaining_memory_regions = candidates.keys().copied().collect_vec();
                remaining_memory_regions.shuffle(&mut rng());
                remaining_memory_regions.sort_unstable_by_key(|x| {
                    candidates
                        .get(x)
                        .expect("region must exist - we just got it from there")
                        .len()
                        // Clamp the length to `count` to treat all larger regions equally.
                        .min(count)
                });
                // We want to start with the largest memory regions first.
                remaining_memory_regions.reverse();

                let mut remaining_memory_regions = VecDeque::from(remaining_memory_regions);

                let mut processors: Vec<Processor> = Vec::with_capacity(count);

                while processors.len() < count {
                    let memory_region = remaining_memory_regions.pop_front()?;

                    let processors_in_region = candidates.get(&memory_region).expect(
                        "we picked an existing key from an existing HashSet - the values must exist",
                    );

                    // There might not be enough to fill the request, which is fine.
                    let choose_count = count.min(processors_in_region.len());

                    let region_processors = processors_in_region
                        .choose_multiple(&mut rand::rng(), choose_count)
                        .cloned();

                    processors.extend(region_processors);
                }

                processors
            }
            MemoryRegionSelector::RequireSame => {
                // We filter out memory regions that do not have enough candidates and pick a
                // random one from the remaining, then picking a random `count` processors.
                let qualifying_memory_regions = candidates
                    .iter()
                    .filter_map(|(region, processors)| {
                        if processors.len() < count.get() {
                            return None;
                        }

                        Some(region)
                    })
                    .collect_vec();

                let memory_region = qualifying_memory_regions.choose(&mut rand::rng())?;

                let processors = candidates.get(memory_region).expect(
                    "we picked an existing key for an existing HashSet - the values must exist",
                );

                processors
                    .choose_multiple(&mut rand::rng(), count.get())
                    .cloned()
                    .collect_vec()
            }
            MemoryRegionSelector::PreferDifferent => {
                // We iterate through the memory regions and prefer one from each, looping through
                // memory regions that still have processors until we have as many as requested.

                // We will start removing processors are memory regions that are used up.
                let mut candidates = candidates;

                let mut processors = Vec::with_capacity(count.get());

                while processors.len() < count.get() {
                    if candidates.is_empty() {
                        // Not enough candidates remaining to satisfy request.
                        return None;
                    }

                    for remaining_processors in candidates.values_mut() {
                        let (index, processor) =
                            remaining_processors.iter().enumerate().choose(&mut rng())?;

                        let processor = processor.clone();

                        remaining_processors.remove(index);

                        processors.push(processor);

                        if processors.len() == count.get() {
                            break;
                        }
                    }

                    // Remove any memory regions that have been depleted.
                    candidates.retain(|_, remaining_processors| !remaining_processors.is_empty());
                }

                processors
            }
            MemoryRegionSelector::RequireDifferent => {
                // We pick random `count` memory regions and a random processor from each.

                if candidates.len() < count.get() {
                    // Not enough memory regions to satisfy request.
                    return None;
                }

                candidates
                    .iter()
                    .choose_multiple(&mut rng(), count.get())
                    .into_iter()
                    .map(|(_, processors)| {
                        processors.iter().choose(&mut rng()).cloned().expect(
                            "we are picking one item from a non-empty list - item must exist",
                        )
                    })
                    .collect_vec()
            }
        };

        Some(ProcessorSet::new(
            NonEmpty::from_vec(processors)?,
            self.tracker_client,
            self.pal,
        ))
    }

    /// Returns a processor set with all processors that match the configured criteria.
    ///
    /// If multiple alternative non-empty sets are a match, returns an arbitrary one of them.
    /// For example, if specifying only a "same memory region" constraint, it will return all
    /// the processors in an arbitrary memory region with at least one qualifying processor.
    ///
    /// Returns `None` if there were no matching processors to satisfy the request.
    pub fn take_all(self) -> Option<ProcessorSet> {
        let candidates = self.candidates_by_memory_region();

        if candidates.is_empty() {
            // No candidates to choose from - everything was filtered out.
            return None;
        }

        let processors = match self.memory_region_selector {
            MemoryRegionSelector::Any
            | MemoryRegionSelector::PreferSame
            | MemoryRegionSelector::PreferDifferent => {
                // We return all processors in all memory regions because we have no strong
                // filtering criterium we must follow - all are fine, so we return all.
                candidates
                    .values()
                    .flat_map(|x| x.iter().cloned())
                    .collect()
            }
            MemoryRegionSelector::RequireSame => {
                // We return all processors in a random memory region.
                // The candidate set only contains memory regions with at least 1 processor, so
                // we know that all candidate memory regions are valid and we were not given a
                // count, so even 1 processor is enough to satisfy the "all" criterion.
                let memory_region = candidates
                    .keys()
                    .choose(&mut rand::rng())
                    .expect("we picked a random existing index - element must exist");

                let processors = candidates.get(memory_region).expect(
                    "we picked an existing key for an existing HashSet - the values must exist",
                );

                processors.to_vec()
            }
            MemoryRegionSelector::RequireDifferent => {
                // We return a random processor from each memory region.
                // The candidate set only contains memory regions with at least 1 processor, so
                // we know that all candidate memory regions have enough to satisfy our needs.
                let processors = candidates.values().map(|processors| {
                    processors
                        .choose(&mut rand::rng())
                        .cloned()
                        .expect("we picked a random item from a non-empty list - item must exist")
                });

                processors.collect()
            }
        };

        Some(ProcessorSet::new(
            NonEmpty::from_vec(processors)?,
            self.tracker_client,
            self.pal,
        ))
    }

    /// Executes the first stage filters to kick out processors purely based on their individual
    /// characteristics. Whatever pass this filter are valid candidates for selection as long
    /// as the next stage of filtering (the memory region logic) permits it.
    ///
    /// Returns candidates grouped by memory region, with each returned memory region having at
    /// least one candidate processor.
    fn candidates_by_memory_region(&self) -> HashMap<MemoryRegionId, Vec<Processor>> {
        let candidates_iter = self.all_processors().into_iter().filter_map(move |p| {
            if self.except_indexes.contains(&p.id()) {
                return None;
            }

            let is_acceptable_type = match self.processor_type_selector {
                ProcessorTypeSelector::Any => true,
                ProcessorTypeSelector::Performance => {
                    p.efficiency_class() == EfficiencyClass::Performance
                }
                ProcessorTypeSelector::Efficiency => {
                    p.efficiency_class() == EfficiencyClass::Efficiency
                }
            };

            if !is_acceptable_type {
                return None;
            }

            Some((p.memory_region_id(), p))
        });

        let mut candidates = HashMap::new();
        for (region, processor) in candidates_iter {
            candidates
                .entry(region)
                .or_insert_with(Vec::new)
                .push(processor);
        }

        candidates
    }

    fn all_processors(&self) -> NonEmpty<Processor> {
        // Cheap conversion, reasonable to do it inline since we do not expect
        // processor set logic to be on the hot path anyway.
        self.pal
            .get_all_processors()
            .map(|p| Processor::new(p, self.pal.clone()))
    }
}

impl Default for ProcessorSetBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
enum MemoryRegionSelector {
    /// The default - memory regions are not considered in processor selection.
    #[default]
    Any,

    /// Processors are all from the same memory region. If there are not enough processors in any
    /// memory region, the request will fail.
    RequireSame,

    /// Processors are all from the different memory regions. If there are not enough memory
    /// regions, the request will fail.
    RequireDifferent,

    /// Processors are ideally all from the same memory region. If there are not enough processors
    /// in a single memory region, more memory regions will be added to the candidate set as needed,
    /// but still keeping it to as few as possible.
    PreferSame,

    /// Processors are ideally all from the different memory regions. If there are not enough memory
    /// regions, multiple processors from the same memory region will be returned, but still keeping
    /// to as many different memory regions as possible to spread the processors out.
    PreferDifferent,
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

#[cfg(not(miri))] // Talking to the operating system is not possible under Miri.
#[cfg(test)]
mod tests_real {
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
        let processor_count = ProcessorSet::all().len();

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
        let processors = ProcessorSet::builder().filter(|_| true).take_all().unwrap();

        let processor_count = ProcessorSet::all().len();

        assert_eq!(processors.len(), processor_count);
    }

    #[test]
    fn filter_out_all() {
        // If we filter out all processors, there should be nothing left.
        assert!(
            ProcessorSet::builder()
                .filter(|_| false)
                .take_all()
                .is_none()
        );
    }

    #[test]
    fn except_all() {
        // If we exclude all processors, there should be nothing left.
        assert!(
            ProcessorSet::builder()
                .except(ProcessorSet::all().processors().iter())
                .take_all()
                .is_none()
        );
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use crate::pal::{FakeProcessor, MockPlatform, ProcessorFacade};
    use nonempty::nonempty;

    use super::*;

    // https://github.com/cloudhead/nonempty/issues/68
    extern crate alloc;

    const TWO_USIZE: NonZeroUsize = NonZeroUsize::new(2).unwrap();
    const THREE_USIZE: NonZeroUsize = NonZeroUsize::new(3).unwrap();
    const FOUR_USIZE: NonZeroUsize = NonZeroUsize::new(4).unwrap();

    #[test]
    fn smoke_test() {
        let mut platform = MockPlatform::new();

        let pal_processors = nonempty![
            FakeProcessor {
                index: 0,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 1,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Performance,
            }
        ];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );

        // Simplest possible test, verify that we see all the processors.
        let set = builder.take_all().unwrap();
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn efficiency_class_filter_take() {
        let mut platform = MockPlatform::new();

        let pal_processors = nonempty![
            FakeProcessor {
                index: 0,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 1,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Performance,
            }
        ];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.efficiency_processors_only().take_all().unwrap();
        assert_eq!(set.len(), 1);
        assert_eq!(set.processors().first().id(), 0);
    }

    #[test]
    fn efficiency_class_filter_take_all() {
        let mut platform = MockPlatform::new();

        let pal_processors = nonempty![
            FakeProcessor {
                index: 0,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 1,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Performance,
            }
        ];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.efficiency_processors_only().take_all().unwrap();
        assert_eq!(set.len(), 1);
        assert_eq!(set.processors().first().id(), 0);
    }

    #[test]
    fn take_n_processors() {
        let mut platform = MockPlatform::new();

        let pal_processors = nonempty![
            FakeProcessor {
                index: 0,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 1,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Performance,
            },
            FakeProcessor {
                index: 2,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Efficiency,
            }
        ];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.take(TWO_USIZE).unwrap();
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn take_n_not_enough_processors() {
        let mut platform = MockPlatform::new();

        let pal_processors = nonempty![
            FakeProcessor {
                index: 0,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 1,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Performance,
            }
        ];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.take(THREE_USIZE);
        assert!(set.is_none());
    }

    #[test]
    fn take_all_not_enough_processors() {
        let mut platform = MockPlatform::new();

        let pal_processors = nonempty![FakeProcessor {
            index: 0,
            memory_region: 0,
            efficiency_class: EfficiencyClass::Efficiency,
        }];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.performance_processors_only().take_all();
        assert!(set.is_none());
    }

    #[test]
    fn except_filter_take() {
        let mut platform = MockPlatform::new();

        let pal_processors = nonempty![
            FakeProcessor {
                index: 0,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 1,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Performance,
            }
        ];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );

        let except_set = builder.clone().filter(|p| p.id() == 0).take_all().unwrap();
        assert_eq!(except_set.len(), 1);

        let set = builder.except(except_set.processors()).take_all().unwrap();
        assert_eq!(set.len(), 1);
        assert_eq!(set.processors().first().id(), 1);
    }

    #[test]
    fn except_filter_take_all() {
        let mut platform = MockPlatform::new();

        let pal_processors = nonempty![
            FakeProcessor {
                index: 0,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 1,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Performance,
            }
        ];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let except_set = builder.clone().filter(|p| p.id() == 0).take_all().unwrap();
        assert_eq!(except_set.len(), 1);

        let set = builder.except(except_set.processors()).take_all().unwrap();
        assert_eq!(set.len(), 1);
        assert_eq!(set.processors().first().id(), 1);
    }

    #[test]
    fn custom_filter_take() {
        let mut platform = MockPlatform::new();

        let pal_processors = nonempty![
            FakeProcessor {
                index: 0,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 1,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Performance,
            }
        ];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.filter(|p| p.id() == 1).take_all().unwrap();
        assert_eq!(set.len(), 1);
        assert_eq!(set.processors().first().id(), 1);
    }

    #[test]
    fn custom_filter_take_all() {
        let mut platform = MockPlatform::new();

        let pal_processors = nonempty![
            FakeProcessor {
                index: 0,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 1,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Performance,
            }
        ];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.filter(|p| p.id() == 1).take_all().unwrap();
        assert_eq!(set.len(), 1);
        assert_eq!(set.processors().first().id(), 1);
    }

    #[test]
    fn same_memory_region_filter_take() {
        let mut platform = MockPlatform::new();

        let pal_processors = nonempty![
            FakeProcessor {
                index: 0,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 1,
                memory_region: 1,
                efficiency_class: EfficiencyClass::Performance,
            }
        ];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.same_memory_region().take_all().unwrap();
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn same_memory_region_filter_take_all() {
        let mut platform = MockPlatform::new();

        let pal_processors = nonempty![
            FakeProcessor {
                index: 0,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 1,
                memory_region: 1,
                efficiency_class: EfficiencyClass::Performance,
            }
        ];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.same_memory_region().take_all().unwrap();
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn different_memory_region_filter_take() {
        let mut platform = MockPlatform::new();

        let pal_processors = nonempty![
            FakeProcessor {
                index: 0,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 1,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 2,
                memory_region: 1,
                efficiency_class: EfficiencyClass::Performance,
            },
            FakeProcessor {
                index: 3,
                memory_region: 1,
                efficiency_class: EfficiencyClass::Performance,
            }
        ];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.different_memory_regions().take_all().unwrap();
        assert_eq!(set.len(), 2);

        assert_ne!(
            set.processors().first().memory_region_id(),
            set.processors().last().memory_region_id()
        );
    }

    #[test]
    fn different_memory_region_filter_take_all() {
        let mut platform = MockPlatform::new();

        let pal_processors = nonempty![
            FakeProcessor {
                index: 0,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 1,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 2,
                memory_region: 1,
                efficiency_class: EfficiencyClass::Performance,
            },
            FakeProcessor {
                index: 3,
                memory_region: 1,
                efficiency_class: EfficiencyClass::Performance,
            }
        ];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.different_memory_regions().take_all().unwrap();
        assert_eq!(set.len(), 2);

        assert_ne!(
            set.processors().first().memory_region_id(),
            set.processors().last().memory_region_id()
        );
    }

    #[test]
    fn filter_combinations() {
        let mut platform = MockPlatform::new();

        let pal_processors = nonempty![
            FakeProcessor {
                index: 0,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 1,
                memory_region: 1,
                efficiency_class: EfficiencyClass::Performance,
            },
            FakeProcessor {
                index: 2,
                memory_region: 1,
                efficiency_class: EfficiencyClass::Efficiency,
            }
        ];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let except_set = builder.clone().filter(|p| p.id() == 0).take_all().unwrap();
        let set = builder
            .efficiency_processors_only()
            .except(except_set.processors())
            .different_memory_regions()
            .take_all()
            .unwrap();
        assert_eq!(set.len(), 1);
        assert_eq!(set.processors().first().id(), 2);
    }

    #[test]
    fn same_memory_region_take_two_processors() {
        let mut platform = MockPlatform::new();

        let pal_processors = nonempty![
            FakeProcessor {
                index: 0,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 1,
                memory_region: 1,
                efficiency_class: EfficiencyClass::Performance,
            },
            FakeProcessor {
                index: 2,
                memory_region: 1,
                efficiency_class: EfficiencyClass::Efficiency,
            }
        ];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.same_memory_region().take(TWO_USIZE).unwrap();
        assert_eq!(set.len(), 2);
        assert!(set.processors().iter().any(|p| p.id() == 1));
        assert!(set.processors().iter().any(|p| p.id() == 2));
    }

    #[test]
    fn different_memory_region_and_efficiency_class_filters() {
        let mut platform = MockPlatform::new();

        let pal_processors = nonempty![
            FakeProcessor {
                index: 0,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 1,
                memory_region: 1,
                efficiency_class: EfficiencyClass::Performance,
            },
            FakeProcessor {
                index: 2,
                memory_region: 2,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 3,
                memory_region: 3,
                efficiency_class: EfficiencyClass::Performance,
            }
        ];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder
            .different_memory_regions()
            .efficiency_processors_only()
            .take_all()
            .unwrap();
        assert_eq!(set.len(), 2);
        assert!(set.processors().iter().any(|p| p.id() == 0));
        assert!(set.processors().iter().any(|p| p.id() == 2));
    }

    #[test]
    fn performance_processors_but_all_efficiency() {
        let mut platform = MockPlatform::new();
        let pal_processors = nonempty![FakeProcessor {
            index: 0,
            memory_region: 0,
            efficiency_class: EfficiencyClass::Efficiency,
        }];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.performance_processors_only().take_all();
        assert!(set.is_none(), "No performance processors should be found.");
    }

    #[test]
    fn require_different_single_region() {
        let mut platform = MockPlatform::new();
        let pal_processors = nonempty![FakeProcessor {
            index: 0,
            memory_region: 0,
            efficiency_class: EfficiencyClass::Efficiency,
        }];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.different_memory_regions().take(TWO_USIZE);
        assert!(
            set.is_none(),
            "Should fail because there's not enough distinct memory regions."
        );
    }

    #[test]
    fn prefer_different_memory_regions_take_all() {
        let mut platform = MockPlatform::new();

        let pal_processors = nonempty![
            FakeProcessor {
                index: 0,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 1,
                memory_region: 1,
                efficiency_class: EfficiencyClass::Performance,
            },
            FakeProcessor {
                index: 2,
                memory_region: 1,
                efficiency_class: EfficiencyClass::Efficiency,
            }
        ];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder
            .prefer_different_memory_regions()
            .take_all()
            .unwrap();
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn prefer_different_memory_regions_take_n() {
        let mut platform = MockPlatform::new();

        let pal_processors = nonempty![
            FakeProcessor {
                index: 0,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 1,
                memory_region: 1,
                efficiency_class: EfficiencyClass::Performance,
            },
            FakeProcessor {
                index: 2,
                memory_region: 1,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 3,
                memory_region: 2,
                efficiency_class: EfficiencyClass::Performance,
            }
        ];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder
            .prefer_different_memory_regions()
            .take(TWO_USIZE)
            .unwrap();
        assert_eq!(set.len(), 2);
        let regions: HashSet<_> = set
            .processors()
            .iter()
            .map(|p| p.memory_region_id())
            .collect();
        assert_eq!(regions.len(), 2);
    }

    #[test]
    fn prefer_same_memory_regions_take_n() {
        let mut platform = MockPlatform::new();

        let pal_processors = nonempty![
            FakeProcessor {
                index: 0,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 1,
                memory_region: 1,
                efficiency_class: EfficiencyClass::Performance,
            },
            FakeProcessor {
                index: 2,
                memory_region: 1,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 3,
                memory_region: 2,
                efficiency_class: EfficiencyClass::Performance,
            }
        ];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.prefer_same_memory_region().take(TWO_USIZE).unwrap();
        assert_eq!(set.len(), 2);
        let regions: HashSet<_> = set
            .processors()
            .iter()
            .map(|p| p.memory_region_id())
            .collect();
        assert_eq!(regions.len(), 1);
    }

    #[test]
    fn prefer_different_memory_regions_take_n_not_enough() {
        let mut platform = MockPlatform::new();

        let pal_processors = nonempty![
            FakeProcessor {
                index: 0,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 1,
                memory_region: 1,
                efficiency_class: EfficiencyClass::Performance,
            },
            FakeProcessor {
                index: 2,
                memory_region: 1,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 3,
                memory_region: 2,
                efficiency_class: EfficiencyClass::Performance,
            }
        ];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder
            .prefer_different_memory_regions()
            .take(FOUR_USIZE)
            .unwrap();
        assert_eq!(set.len(), 4);
    }

    #[test]
    fn prefer_same_memory_regions_take_n_not_enough() {
        let mut platform = MockPlatform::new();

        let pal_processors = nonempty![
            FakeProcessor {
                index: 0,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 1,
                memory_region: 1,
                efficiency_class: EfficiencyClass::Performance,
            },
            FakeProcessor {
                index: 2,
                memory_region: 1,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 3,
                memory_region: 2,
                efficiency_class: EfficiencyClass::Performance,
            }
        ];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder
            .prefer_same_memory_region()
            .take(THREE_USIZE)
            .unwrap();
        assert_eq!(set.len(), 3);
        let regions: HashSet<_> = set
            .processors()
            .iter()
            .map(|p| p.memory_region_id())
            .collect();
        assert_eq!(
            2,
            regions.len(),
            "should have picked to minimize memory regions (biggest first)"
        );
    }

    #[test]
    fn prefer_same_memory_regions_take_n_picks_best_fit() {
        let mut platform = MockPlatform::new();

        let pal_processors = nonempty![
            FakeProcessor {
                index: 0,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 1,
                memory_region: 1,
                efficiency_class: EfficiencyClass::Performance,
            },
            FakeProcessor {
                index: 2,
                memory_region: 1,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 3,
                memory_region: 2,
                efficiency_class: EfficiencyClass::Performance,
            }
        ];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.prefer_same_memory_region().take(TWO_USIZE).unwrap();
        assert_eq!(set.len(), 2);
        let regions: HashSet<_> = set
            .processors()
            .iter()
            .map(|p| p.memory_region_id())
            .collect();
        assert_eq!(
            1,
            regions.len(),
            "should have picked from memory region 1 which  can accommodate the preference"
        );
    }
}

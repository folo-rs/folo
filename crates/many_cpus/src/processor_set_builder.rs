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
/// available processors, with the maximum count determined by the process resource quota.
///
/// You can obtain a builder via [`ProcessorSet::builder()`] or from an existing processor set
/// via [`ProcessorSet::to_builder()`].
///
#[doc = include_str!("../docs/snippets/external_constraints.md")]
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
/// [2]: ProcessorSetBuilder::ignoring_resource_quota
#[derive(Clone, Debug)]
pub struct ProcessorSetBuilder {
    processor_type_selector: ProcessorTypeSelector,
    memory_region_selector: MemoryRegionSelector,

    except_indexes: HashSet<ProcessorId>,

    obey_resource_quota: bool,

    // ProcessorSet needs this because it needs to inform the tracker
    // about any changes to the pinning status of the current thread.
    // We just carry it around and pass to any processor set we create.
    tracker_client: HardwareTrackerClientFacade,

    pal: PlatformFacade,
}

impl ProcessorSetBuilder {
    /// Creates a new processor set builder with the default configuration.
    ///
    /// The default configuration considers every processor on the system as a valid candidate,
    /// except those for which the operating system has defined hard limits. See type-level
    /// documentation for more details on how operating system limits are handled.
    #[must_use]
    pub fn new() -> Self {
        Self::with_internals(HardwareTrackerClientFacade::real(), PlatformFacade::real())
    }

    #[must_use]
    pub(crate) fn with_internals(
        tracker_client: HardwareTrackerClientFacade,
        pal: PlatformFacade,
    ) -> Self {
        Self {
            processor_type_selector: ProcessorTypeSelector::Any,
            memory_region_selector: MemoryRegionSelector::Any,
            except_indexes: HashSet::new(),
            obey_resource_quota: true,
            tracker_client,
            pal,
        }
    }

    /// Requires that all processors in the set be marked as [performance processors][1].
    ///
    /// [1]: EfficiencyClass::Performance
    #[must_use]
    pub fn performance_processors_only(mut self) -> Self {
        self.processor_type_selector = ProcessorTypeSelector::Performance;
        self
    }

    /// Requires that all processors in the set be marked as [efficiency processors][1].
    ///
    /// [1]: EfficiencyClass::Efficiency
    #[must_use]
    pub fn efficiency_processors_only(mut self) -> Self {
        self.processor_type_selector = ProcessorTypeSelector::Efficiency;
        self
    }

    /// Requires that all processors in the set be from different memory regions, selecting a
    /// maximum of 1 processor from each memory region.
    #[must_use]
    pub fn different_memory_regions(mut self) -> Self {
        self.memory_region_selector = MemoryRegionSelector::RequireDifferent;
        self
    }

    /// Requires that all processors in the set be from the same memory region.
    #[must_use]
    pub fn same_memory_region(mut self) -> Self {
        self.memory_region_selector = MemoryRegionSelector::RequireSame;
        self
    }

    /// Declares a preference that all processors in the set be from different memory regions,
    /// though will select multiple processors from the same memory region as needed to satisfy
    /// the requested processor count (while keeping the spread maximal).
    #[must_use]
    pub fn prefer_different_memory_regions(mut self) -> Self {
        self.memory_region_selector = MemoryRegionSelector::PreferDifferent;
        self
    }

    /// Declares a preference that all processors in the set be from the same memory region,
    /// though will select processors from different memory regions as needed to satisfy the
    /// requested processor count (while keeping the spread minimal).
    #[must_use]
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
    #[must_use]
    pub fn filter(mut self, predicate: impl Fn(&Processor) -> bool) -> Self {
        // We invoke the filters immediately because the API gets really annoying if the
        // predicate has to borrow some stuff because it would need to be 'static and that
        // is cumbersome (since we do not return a generic-lifetimed thing back to the caller).
        for processor in self.all_processors() {
            if !predicate(&processor) {
                self.except_indexes.insert(processor.id());
            }
        }

        self
    }

    /// Removes specific processors from the set of candidates.
    #[must_use]
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
    #[must_use]
    pub fn where_available_for_current_thread(mut self) -> Self {
        let current_thread_processors = self.pal.current_thread_processors();

        for processor in self.all_processors() {
            if !current_thread_processors.contains(&processor.id()) {
                self.except_indexes.insert(processor.id());
            }
        }

        self
    }

    /// Ignores the process resource quota when determining the maximum number of processors
    /// that can be included in the created processor set.
    ///
    /// This can be valuable to identify the total set of available processors, though is typically
    /// not a good idea when scheduling work on the processors. See the type-level documentation
    /// for more details on resource quota handling best practices.
    #[must_use]
    pub fn ignoring_resource_quota(mut self) -> Self {
        self.obey_resource_quota = false;
        self
    }

    /// Creates a processor set with a specific number of processors that match the
    /// configured criteria.
    ///
    /// If multiple candidate sets are a match, returns an arbitrary one of them. For example, if
    /// there are six valid candidate processors then `take(4)` may return any four of them.
    ///
    /// Returns `None` if there were not enough candidate processors to satisfy the request.
    ///
    /// # Resource quota
    ///
    /// Unless overridden by [`ignoring_resource_quota()`][1], the call will fail if the number of
    /// requested processors is above the process resource quota. See the type-level
    /// documentation for more details on resource quota handling best practices.
    ///
    /// [1]: ProcessorSetBuilder::ignoring_resource_quota
    #[must_use]
    pub fn take(self, count: NonZeroUsize) -> Option<ProcessorSet> {
        if let Some(max_count) = self.resource_quota_processor_count_limit() {
            if count.get() > max_count {
                // We cannot satisfy the request.
                return None;
            }
        }

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
                        .choose_multiple(&mut rng(), choose_count)
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

                let memory_region = qualifying_memory_regions.choose(&mut rng())?;

                let processors = candidates.get(memory_region).expect(
                    "we picked an existing key for an existing HashSet - the values must exist",
                );

                processors
                    .choose_multiple(&mut rng(), count.get())
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
    ///
    /// # Resource quota
    ///
    /// Unless overridden by [`ignoring_resource_quota()`][1], the maximum number of processors
    /// returned is limited by the process resource quota. See the type-level documentation
    /// for more details on resource quota handling best practices.
    ///
    /// [1]: ProcessorSetBuilder::ignoring_resource_quota
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Hangs due to recursive access of OnceLock.
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
                // filtering criterion we must follow - all are fine, so we return all.
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
                    .choose(&mut rng())
                    .expect("we picked a random existing index - element must exist");

                let processors = candidates.get(memory_region).expect(
                    "we picked an existing key for an existing HashSet - the values must exist",
                );

                processors.clone()
            }
            MemoryRegionSelector::RequireDifferent => {
                // We return a random processor from each memory region.
                // The candidate set only contains memory regions with at least 1 processor, so
                // we know that all candidate memory regions have enough to satisfy our needs.
                let processors = candidates.values().map(|processors| {
                    processors
                        .choose(&mut rng())
                        .cloned()
                        .expect("we picked a random item from a non-empty list - item must exist")
                });

                processors.collect()
            }
        };

        let processors = self.reduce_processors_until_under_quota(processors);

        Some(ProcessorSet::new(
            NonEmpty::from_vec(processors)?,
            self.tracker_client,
            self.pal,
        ))
    }

    fn reduce_processors_until_under_quota(&self, processors: Vec<Processor>) -> Vec<Processor> {
        let Some(max_count) = self.resource_quota_processor_count_limit() else {
            return processors;
        };

        let mut processors = processors;

        // If we picked too many, reduce until we are under quota.
        while processors.len() > max_count {
            processors
                .pop()
                .expect("guarded by len-check in loop condition");
        }

        processors
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
        self.pal.get_all_processors().map(Processor::new)
    }

    fn resource_quota_processor_count_limit(&self) -> Option<usize> {
        if self.obey_resource_quota {
            let max_processor_time = self.pal.max_processor_time();

            // We round down the quota to get a whole number of processors.
            // We specifically round down because our goal with the resource quota is to never
            // exceed it, even by a fraction, as that would cause quality of service degradation.
            #[expect(clippy::cast_sign_loss, reason = "quota cannot be negative")]
            #[expect(
                clippy::cast_possible_truncation,
                reason = "we are correctly rounding to avoid the problem"
            )]
            let max_processor_count = max_processor_time.floor() as usize;

            // We can never restrict to less than 1 processor by quota because that would be
            // nonsense - there is always some available processor time, so at least one
            // processor must be usable. Therefore, we round below 1, and round down above 1.
            Some(max_processor_count.max(1))
        } else {
            None
        }
    }
}

impl Default for ProcessorSetBuilder {
    #[inline]
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
    use folo_utils::nz;

    use super::*;

    #[test]
    fn spawn_on_any_processor() {
        let set = ProcessorSet::builder().take_all().unwrap();
        let result = set.spawn_thread(move |_| 1234).join().unwrap();

        assert_eq!(result, 1234);
    }

    #[test]
    fn spawn_on_every_processor() {
        let set = ProcessorSet::builder().take_all().unwrap();
        let processor_count = set.len();

        let join_handles = set.spawn_threads(move |_| 4567);

        assert_eq!(join_handles.len(), processor_count);

        for handle in join_handles {
            let result = handle.join().unwrap();
            assert_eq!(result, 4567);
        }
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
            .take(nz!(1))
            .unwrap();
        ProcessorSet::builder()
            .different_memory_regions()
            .take_all()
            .unwrap();
        ProcessorSet::builder()
            .different_memory_regions()
            .take(nz!(1))
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
            .take(nz!(1))
            .unwrap();

        // There might not be any. We just try resolving it and ignore the result.
        // As long as it does not panic, we are good.
        drop(
            ProcessorSet::builder()
                .efficiency_processors_only()
                .take_all(),
        );
        drop(
            ProcessorSet::builder()
                .efficiency_processors_only()
                .take(nz!(1)),
        );
    }

    #[test]
    fn filter_in_all() {
        // Ensure we use a constant starting set, in case we are running tests under constraints.
        let starting_set = ProcessorSet::builder().take_all().unwrap();

        // If we filter in all processors, we should get all of them.
        let processors = starting_set
            .to_builder()
            .filter(|_| true)
            .take_all()
            .unwrap();
        let processor_count = starting_set.len();

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
        // Ensure we use a constant starting set, in case we are running tests under constraints.
        let starting_set = ProcessorSet::builder().take_all().unwrap();

        // If we exclude all processors, there should be nothing left.
        assert!(
            starting_set
                .to_builder()
                .except(starting_set.processors().iter())
                .take_all()
                .is_none()
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::pal::{FakeProcessor, MockPlatform, ProcessorFacade};
    use folo_utils::nz;
    use nonempty::nonempty;

    use super::*;

    // https://github.com/cloudhead/nonempty/issues/68
    extern crate alloc;

    #[test]
    fn smoke_test() {
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

        let platform = new_mock_platform(pal_processors);

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

        let platform = new_mock_platform(pal_processors);

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

        let platform = new_mock_platform(pal_processors);

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

        let platform = new_mock_platform(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.take(nz!(2)).unwrap();
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn take_n_not_enough_processors() {
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

        // The "max processor time" call will already abort the build early
        // because it shows that not enough processor time is available.
        let platform = new_mock_platform_with_get_count(pal_processors, 0, 1);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.take(nz!(3));
        assert!(set.is_none());
    }

    #[test]
    fn take_n_not_enough_processor_time_quota() {
        let mut platform = MockPlatform::new();

        // There is only 1.0 processors worth of quota available.
        platform
            .expect_max_processor_time()
            .times(1)
            .return_const(1.0);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.take(nz!(2));
        assert!(set.is_none());
    }

    #[test]
    fn take_n_not_enough_processor_time_quota_but_ignoring_quota() {
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

        let mut platform = MockPlatform::new();
        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        // There is only 1.0 processors worth of quota available. This limitation should be ignored.
        // In fact, this call should not even be made - we have it here just to fail the test with
        // 1.0 (which is too low) if one day the call starts being made.
        platform.expect_max_processor_time().return_const(1.0);

        platform
            .expect_get_all_processors_core()
            .times(1)
            .return_const(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.ignoring_resource_quota().take(nz!(2));

        assert_eq!(set.unwrap().len(), 2);
    }

    #[test]
    fn take_n_quota_limit_min_1() {
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

        let mut platform = MockPlatform::new();
        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        // There is 0.001 processors worth of quota available, which gets rounded UP to 1.0.
        // While we normally round down due to quality of service considerations. However, we still
        // need to return at least 1 processor because there is at least some some processor time
        // available that we must allow to be spent - rounding to zero would be nonsense.
        platform.expect_max_processor_time().return_const(0.001);

        platform
            .expect_get_all_processors_core()
            .times(1)
            .return_const(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.take(nz!(1));

        assert_eq!(set.unwrap().len(), 1);
    }

    #[test]
    fn take_all_rounds_down_quota() {
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

        let mut platform = MockPlatform::new();
        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        // There is 1.999 processors worth of quota available, which gets rounded to 1.0.
        platform
            .expect_max_processor_time()
            .times(1)
            .return_const(1.999);

        platform
            .expect_get_all_processors_core()
            .times(1)
            .return_const(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.take_all().unwrap();
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn take_all_min_1_despite_quota() {
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

        let mut platform = MockPlatform::new();
        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        // There is 0.001 processors worth of quota available, which gets rounded UP to 1.0.
        // While we normally round down due to quality of service considerations. However, we still
        // need to return at least 1 processor because there is at least some some processor time
        // available that we must allow to be spent - rounding to zero would be nonsense.
        platform
            .expect_max_processor_time()
            .times(1)
            .return_const(0.001);

        platform
            .expect_get_all_processors_core()
            .times(1)
            .return_const(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.take_all().unwrap();
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn take_all_not_enough_processors() {
        let pal_processors = nonempty![FakeProcessor {
            index: 0,
            memory_region: 0,
            efficiency_class: EfficiencyClass::Efficiency,
        }];

        // We expect 0 calls to max_processor_time() because we failed early in the build.
        let platform = new_mock_platform_with_get_count(pal_processors, 1, 0);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.performance_processors_only().take_all();
        assert!(set.is_none());
    }

    #[test]
    fn except_filter_take() {
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

        // .filter() eagerly evaluates processors, so we need to allow +1 call.
        // We call .take() twice, so need to allow +1 of each.
        let platform = new_mock_platform_with_get_count(pal_processors, 3, 2);

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

        // .filter() eagerly evaluates processors, so we need to allow +1 call.
        // We call .take() twice, so need to allow +1 of each.
        let platform = new_mock_platform_with_get_count(pal_processors, 3, 2);

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

        // .filter() eagerly evaluates processors, so we need to allow 2 calls.
        let platform = new_mock_platform_with_get_count(pal_processors, 2, 1);

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

        // .filter() eagerly evaluates processors, so we need to allow 2 calls.
        let platform = new_mock_platform_with_get_count(pal_processors, 2, 1);

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

        let platform = new_mock_platform(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.same_memory_region().take_all().unwrap();
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn same_memory_region_filter_take_all() {
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

        let platform = new_mock_platform(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.same_memory_region().take_all().unwrap();
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn different_memory_region_filter_take() {
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

        let platform = new_mock_platform(pal_processors);

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

        let platform = new_mock_platform(pal_processors);

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

        // .filter() eagerly evaluates processors, so we need to allow +1 call.
        // We call .take() twice, so need to allow +1 of each.
        let platform = new_mock_platform_with_get_count(pal_processors, 3, 2);

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

        let platform = new_mock_platform(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.same_memory_region().take(nz!(2)).unwrap();
        assert_eq!(set.len(), 2);
        assert!(set.processors().iter().any(|p| p.id() == 1));
        assert!(set.processors().iter().any(|p| p.id() == 2));
    }

    #[test]
    fn different_memory_region_and_efficiency_class_filters() {
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

        let platform = new_mock_platform(pal_processors);

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
        let pal_processors = nonempty![FakeProcessor {
            index: 0,
            memory_region: 0,
            efficiency_class: EfficiencyClass::Efficiency,
        }];

        // We expect 0 calls to max_processor_time() because we failed early in the build.
        let platform = new_mock_platform_with_get_count(pal_processors, 1, 0);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );

        let set = builder.performance_processors_only().take_all();
        assert!(set.is_none(), "No performance processors should be found.");
    }

    #[test]
    fn require_different_single_region() {
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
            }
        ];

        let platform = new_mock_platform(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.different_memory_regions().take(nz!(2));
        assert!(
            set.is_none(),
            "Should fail because there's not enough distinct memory regions."
        );
    }

    #[test]
    fn prefer_different_memory_regions_take_all() {
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

        let platform = new_mock_platform(pal_processors);

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

        let platform = new_mock_platform(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder
            .prefer_different_memory_regions()
            .take(nz!(2))
            .unwrap();
        assert_eq!(set.len(), 2);
        let regions: HashSet<_> = set
            .processors()
            .iter()
            .map(Processor::memory_region_id)
            .collect();
        assert_eq!(regions.len(), 2);
    }

    #[test]
    fn prefer_same_memory_regions_take_n() {
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

        let platform = new_mock_platform(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.prefer_same_memory_region().take(nz!(2)).unwrap();
        assert_eq!(set.len(), 2);
        let regions: HashSet<_> = set
            .processors()
            .iter()
            .map(Processor::memory_region_id)
            .collect();
        assert_eq!(regions.len(), 1);
    }

    #[test]
    fn prefer_different_memory_regions_take_n_not_enough() {
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

        let platform = new_mock_platform(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder
            .prefer_different_memory_regions()
            .take(nz!(4))
            .unwrap();
        assert_eq!(set.len(), 4);
    }

    #[test]
    fn prefer_same_memory_regions_take_n_not_enough() {
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

        let platform = new_mock_platform(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.prefer_same_memory_region().take(nz!(3)).unwrap();
        assert_eq!(set.len(), 3);
        let regions: HashSet<_> = set
            .processors()
            .iter()
            .map(Processor::memory_region_id)
            .collect();
        assert_eq!(
            2,
            regions.len(),
            "should have picked to minimize memory regions (biggest first)"
        );
    }

    #[test]
    fn prefer_same_memory_regions_take_n_picks_best_fit() {
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

        let platform = new_mock_platform(pal_processors);

        let builder = ProcessorSetBuilder::with_internals(
            HardwareTrackerClientFacade::default_mock(),
            platform.into(),
        );
        let set = builder.prefer_same_memory_region().take(nz!(2)).unwrap();
        assert_eq!(set.len(), 2);
        let regions: HashSet<_> = set
            .processors()
            .iter()
            .map(Processor::memory_region_id)
            .collect();
        assert_eq!(
            1,
            regions.len(),
            "should have picked from memory region 1 which  can accommodate the preference"
        );
    }

    fn new_mock_platform(processors: NonEmpty<FakeProcessor>) -> MockPlatform {
        new_mock_platform_with_get_count(processors, 1, 1)
    }

    #[allow(
        clippy::cast_precision_loss,
        reason = "unavoidable f64 conversion but all realistic values likely in safe bounds"
    )]
    fn new_mock_platform_with_get_count(
        processors: NonEmpty<FakeProcessor>,
        get_all_processors_count: usize,
        get_max_processor_time_count: usize,
    ) -> MockPlatform {
        let mut platform = MockPlatform::new();
        let pal_processors = processors.map(ProcessorFacade::Fake);

        platform
            .expect_max_processor_time()
            .times(get_max_processor_time_count)
            .return_const(pal_processors.len() as f64);

        platform
            .expect_get_all_processors_core()
            .times(get_all_processors_count)
            .return_const(pal_processors);

        platform
    }
}

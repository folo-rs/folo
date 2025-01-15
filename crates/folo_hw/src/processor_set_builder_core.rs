use std::{
    collections::{HashMap, HashSet},
    fmt::Debug,
    num::NonZeroUsize,
};

use itertools::Itertools;
use nonempty::NonEmpty;
use rand::{
    seq::{IteratorRandom, SliceRandom},
    thread_rng,
};

use crate::{
    pal::{
        EfficiencyClass, MemoryRegionIndex, PlatformCommon, ProcessorCommon, ProcessorGlobalIndex,
    },
    ProcessorCore, ProcessorSetCore,
};

#[derive(Clone, Debug)]
pub(crate) struct ProcessorSetBuilderCore<PAL>
where
    PAL: PlatformCommon,
{
    processor_type_selector: ProcessorTypeSelector,
    memory_region_selector: MemoryRegionSelector,

    except_indexes: HashSet<ProcessorGlobalIndex>,

    pal: &'static PAL,
}

impl<PAL> ProcessorSetBuilderCore<PAL>
where
    PAL: PlatformCommon,
{
    pub(crate) fn new(pal: &'static PAL) -> Self {
        Self {
            processor_type_selector: ProcessorTypeSelector::Any,
            memory_region_selector: MemoryRegionSelector::PreferSame,
            except_indexes: HashSet::new(),
            pal,
        }
    }

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

    pub fn filter(mut self, predicate: impl Fn(&ProcessorCore<PAL>) -> bool) -> Self {
        for processor in self.all_processors() {
            if !predicate(&processor) {
                self.except_indexes.insert(processor.index());
            }
        }

        self
    }

    pub fn except<'a, I>(mut self, processors: I) -> Self
    where
        I: IntoIterator<Item = &'a ProcessorCore<PAL>>,
        <PAL as PlatformCommon>::Processor: 'a,
    {
        for processor in processors {
            self.except_indexes.insert(processor.index());
        }

        self
    }

    pub fn take(self, count: NonZeroUsize) -> Option<ProcessorSetCore<PAL>> {
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
                let mut processors: Vec<ProcessorCore<PAL>> = Vec::with_capacity(count);

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
                    .map(|(_, processors)| {
                        processors.iter().choose(&mut thread_rng()).copied().expect(
                            "we are picking one item from a non-empty list - item must exist",
                        )
                    })
                    .collect_vec()
            }
        };

        Some(ProcessorSetCore::new(
            NonEmpty::from_vec(processors)?,
            self.pal,
        ))
    }

    pub fn take_all(self) -> Option<ProcessorSetCore<PAL>> {
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

                processors.to_vec()
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

                processors.collect()
            }
        };

        Some(ProcessorSetCore::new(
            NonEmpty::from_vec(processors)?,
            self.pal,
        ))
    }

    // Returns candidates grouped by memory region, with each returned memory region having at
    // least one candidate processor.
    fn candidates_by_memory_region(&self) -> HashMap<MemoryRegionIndex, Vec<ProcessorCore<PAL>>> {
        self.all_processors()
            .into_iter()
            .filter_map(move |p| {
                if self.except_indexes.contains(&p.index()) {
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

                Some((p.memory_region(), p))
            })
            .into_group_map()
    }

    fn all_processors(&self) -> NonEmpty<ProcessorCore<PAL>> {
        // Cheap conversion, reasonable to do it inline since we do not expect
        // processor set logic to be on the hot path anyway.
        self.pal
            .get_all_processors()
            .map(|p| ProcessorCore::new(p, self.pal))
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

// #[cfg(test)]
// mod tests {
//     use std::sync::{Arc, Barrier};

//     use super::*;

//     // TODO: We need a "give up" mechanism so tests do not hang forever if they fail.

//     const ONE: NonZeroUsize = NonZeroUsize::new(1).unwrap();

//     #[test]
//     fn spawn_on_any_processor() {
//         let done = Arc::new(Barrier::new(2));

//         let set = ProcessorSet::all();
//         set.spawn_thread({
//             let done = Arc::clone(&done);

//             move |_| {
//                 done.wait();
//             }
//         });

//         done.wait();
//     }

//     #[test]
//     fn spawn_on_every_processor() {
//         let processor_count: usize = ALL_PROCESSORS.len();

//         let done = Arc::new(Barrier::new(processor_count + 1));

//         let set = ProcessorSet::all();
//         set.spawn_threads({
//             let done = Arc::clone(&done);

//             move |_| {
//                 done.wait();
//             }
//         });

//         done.wait();
//     }

//     #[test]
//     fn filter_by_memory_region_real() {
//         // We know there is at least one memory region, so these must succeed.
//         ProcessorSet::builder()
//             .same_memory_region()
//             .take_all()
//             .unwrap();
//         ProcessorSet::builder()
//             .same_memory_region()
//             .take(ONE)
//             .unwrap();
//         ProcessorSet::builder()
//             .different_memory_regions()
//             .take_all()
//             .unwrap();
//         ProcessorSet::builder()
//             .different_memory_regions()
//             .take(ONE)
//             .unwrap();
//     }

//     #[test]
//     fn filter_by_efficiency_class_real() {
//         // There must be at least one.
//         ProcessorSet::builder()
//             .performance_processors_only()
//             .take_all()
//             .unwrap();
//         ProcessorSet::builder()
//             .performance_processors_only()
//             .take(ONE)
//             .unwrap();

//         // There might not be any. We just try resolving it and ignore the result.
//         // As long as it does not panic, we are good.
//         ProcessorSet::builder()
//             .efficiency_processors_only()
//             .take_all();
//         ProcessorSet::builder()
//             .efficiency_processors_only()
//             .take(ONE);
//     }

//     #[test]
//     fn filter_in_all() {
//         // If we filter in all processors, we should get all of them.
//         let processors = ProcessorSet::builder().filter(|_| true).take_all().unwrap();

//         assert_eq!(processors.len(), ALL_PROCESSORS.len());
//     }

//     #[test]
//     fn filter_out_all() {
//         // If we filter out all processors, there should be nothing left.
//         assert!(ProcessorSet::builder()
//             .filter(|_| false)
//             .take_all()
//             .is_none());
//     }

//     #[test]
//     fn except_all() {
//         // If we exclude all processors, there should be nothing left.
//         assert!(ProcessorSet::builder()
//             .except(ProcessorSet::all().processors())
//             .take_all()
//             .is_none());
//     }
// }

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
    pal::Platform, EfficiencyClass, MemoryRegionId, ProcessorCore, ProcessorId, ProcessorSetCore,
};

#[derive(Debug)]
pub(crate) struct ProcessorSetBuilderCore<PAL: Platform> {
    processor_type_selector: ProcessorTypeSelector,
    memory_region_selector: MemoryRegionSelector,

    except_indexes: HashSet<ProcessorId>,

    pal: &'static PAL,
}

impl<PAL: Platform> ProcessorSetBuilderCore<PAL> {
    pub(crate) fn new(pal: &'static PAL) -> Self {
        Self {
            processor_type_selector: ProcessorTypeSelector::Any,
            memory_region_selector: MemoryRegionSelector::PreferSame,
            except_indexes: HashSet::new(),
            pal,
        }
    }

    pub(crate) fn performance_processors_only(mut self) -> Self {
        self.processor_type_selector = ProcessorTypeSelector::Performance;
        self
    }

    pub(crate) fn efficiency_processors_only(mut self) -> Self {
        self.processor_type_selector = ProcessorTypeSelector::Efficiency;
        self
    }

    pub(crate) fn different_memory_regions(mut self) -> Self {
        self.memory_region_selector = MemoryRegionSelector::RequireDifferent;
        self
    }

    pub(crate) fn same_memory_region(mut self) -> Self {
        self.memory_region_selector = MemoryRegionSelector::RequireSame;
        self
    }

    pub(crate) fn filter(mut self, predicate: impl Fn(&ProcessorCore<PAL>) -> bool) -> Self {
        for processor in self.all_processors() {
            if !predicate(&processor) {
                self.except_indexes.insert(processor.id());
            }
        }

        self
    }

    pub(crate) fn except<'a, I>(mut self, processors: I) -> Self
    where
        I: IntoIterator<Item = &'a ProcessorCore<PAL>>,
        <PAL as Platform>::Processor: 'a,
    {
        for processor in processors {
            self.except_indexes.insert(processor.id());
        }

        self
    }

    pub(crate) fn take(self, count: NonZeroUsize) -> Option<ProcessorSetCore<PAL>> {
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

    pub(crate) fn take_all(self) -> Option<ProcessorSetCore<PAL>> {
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
    fn candidates_by_memory_region(&self) -> HashMap<MemoryRegionId, Vec<ProcessorCore<PAL>>> {
        self.all_processors()
            .into_iter()
            .filter_map(move |p| {
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

impl<PAL: Platform> Clone for ProcessorSetBuilderCore<PAL> {
    fn clone(&self) -> Self {
        Self {
            processor_type_selector: self.processor_type_selector,
            memory_region_selector: self.memory_region_selector,
            except_indexes: self.except_indexes.clone(),
            pal: self.pal,
        }
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

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;
    use std::sync::LazyLock;

    use crate::pal::{FakeProcessor, MockPlatform};
    use nonempty::nonempty;

    use super::*;

    // https://github.com/cloudhead/nonempty/issues/68
    extern crate alloc;

    const TWO_USIZE: NonZeroUsize = unsafe { NonZeroUsize::new_unchecked(2) };
    const THREE_USIZE: NonZeroUsize = unsafe { NonZeroUsize::new_unchecked(3) };

    #[test]
    fn smoke_test() {
        static PAL: LazyLock<MockPlatform> = LazyLock::new(|| {
            let mut mock = MockPlatform::new();

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

            mock.expect_get_all_processors_core()
                .return_const(pal_processors);

            mock
        });

        let builder = ProcessorSetBuilderCore::new(&*PAL);

        // Simplest possible test, verify that we see all the processors.
        let set = builder.take_all().unwrap();
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_efficiency_class_filter_take() {
        static PAL: LazyLock<MockPlatform> = LazyLock::new(|| {
            let mut mock = MockPlatform::new();

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

            mock.expect_get_all_processors_core()
                .return_const(pal_processors);

            mock
        });

        let builder = ProcessorSetBuilderCore::new(&*PAL);
        let set = builder.efficiency_processors_only().take_all().unwrap();
        assert_eq!(set.len(), 1);
        assert_eq!(set.processors().next().unwrap().id(), 0);
    }

    #[test]
    fn test_efficiency_class_filter_take_all() {
        static PAL: LazyLock<MockPlatform> = LazyLock::new(|| {
            let mut mock = MockPlatform::new();

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

            mock.expect_get_all_processors_core()
                .return_const(pal_processors);

            mock
        });

        let builder = ProcessorSetBuilderCore::new(&*PAL);
        let set = builder.efficiency_processors_only().take_all().unwrap();
        assert_eq!(set.len(), 1);
        assert_eq!(set.processors().next().unwrap().id(), 0);
    }

    #[test]
    fn test_take_n_processors() {
        static PAL: LazyLock<MockPlatform> = LazyLock::new(|| {
            let mut mock = MockPlatform::new();

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

            mock.expect_get_all_processors_core()
                .return_const(pal_processors);

            mock
        });

        let builder = ProcessorSetBuilderCore::new(&*PAL);
        let set = builder.take(TWO_USIZE).unwrap();
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_take_n_not_enough_processors() {
        static PAL: LazyLock<MockPlatform> = LazyLock::new(|| {
            let mut mock = MockPlatform::new();

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

            mock.expect_get_all_processors_core()
                .return_const(pal_processors);

            mock
        });

        let builder = ProcessorSetBuilderCore::new(&*PAL);
        let set = builder.take(THREE_USIZE);
        assert!(set.is_none());
    }

    #[test]
    fn test_take_all_not_enough_processors() {
        static PAL: LazyLock<MockPlatform> = LazyLock::new(|| {
            let mut mock = MockPlatform::new();

            let pal_processors = nonempty![FakeProcessor {
                index: 0,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Efficiency,
            }];

            mock.expect_get_all_processors_core()
                .return_const(pal_processors);

            mock
        });

        let builder = ProcessorSetBuilderCore::new(&*PAL);
        let set = builder.performance_processors_only().take_all();
        assert!(set.is_none());
    }

    #[test]
    fn test_except_filter_take() {
        static PAL: LazyLock<MockPlatform> = LazyLock::new(|| {
            let mut mock = MockPlatform::new();

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

            mock.expect_get_all_processors_core()
                .return_const(pal_processors);

            mock
        });

        let builder = ProcessorSetBuilderCore::new(&*PAL);

        let except_set = builder.clone().filter(|p| p.id() == 0).take_all().unwrap();
        assert_eq!(except_set.len(), 1);

        let set = builder.except(except_set.processors()).take_all().unwrap();
        assert_eq!(set.len(), 1);
        assert_eq!(set.processors().next().unwrap().id(), 1);
    }

    #[test]
    fn test_except_filter_take_all() {
        static PAL: LazyLock<MockPlatform> = LazyLock::new(|| {
            let mut mock = MockPlatform::new();

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

            mock.expect_get_all_processors_core()
                .return_const(pal_processors);

            mock
        });

        let builder = ProcessorSetBuilderCore::new(&*PAL);
        let except_set = builder.clone().filter(|p| p.id() == 0).take_all().unwrap();
        assert_eq!(except_set.len(), 1);

        let set = builder.except(except_set.processors()).take_all().unwrap();
        assert_eq!(set.len(), 1);
        assert_eq!(set.processors().next().unwrap().id(), 1);
    }

    #[test]
    fn test_custom_filter_take() {
        static PAL: LazyLock<MockPlatform> = LazyLock::new(|| {
            let mut mock = MockPlatform::new();

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

            mock.expect_get_all_processors_core()
                .return_const(pal_processors);

            mock
        });

        let builder = ProcessorSetBuilderCore::new(&*PAL);
        let set = builder.filter(|p| p.id() == 1).take_all().unwrap();
        assert_eq!(set.len(), 1);
        assert_eq!(set.processors().next().unwrap().id(), 1);
    }

    #[test]
    fn test_custom_filter_take_all() {
        static PAL: LazyLock<MockPlatform> = LazyLock::new(|| {
            let mut mock = MockPlatform::new();

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

            mock.expect_get_all_processors_core()
                .return_const(pal_processors);

            mock
        });

        let builder = ProcessorSetBuilderCore::new(&*PAL);
        let set = builder.filter(|p| p.id() == 1).take_all().unwrap();
        assert_eq!(set.len(), 1);
        assert_eq!(set.processors().next().unwrap().id(), 1);
    }

    #[test]
    fn test_same_memory_region_filter_take() {
        static PAL: LazyLock<MockPlatform> = LazyLock::new(|| {
            let mut mock = MockPlatform::new();

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

            mock.expect_get_all_processors_core()
                .return_const(pal_processors);

            mock
        });

        let builder = ProcessorSetBuilderCore::new(&*PAL);
        let set = builder.same_memory_region().take_all().unwrap();
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn test_same_memory_region_filter_take_all() {
        static PAL: LazyLock<MockPlatform> = LazyLock::new(|| {
            let mut mock = MockPlatform::new();

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

            mock.expect_get_all_processors_core()
                .return_const(pal_processors);

            mock
        });

        let builder = ProcessorSetBuilderCore::new(&*PAL);
        let set = builder.same_memory_region().take_all().unwrap();
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn test_different_memory_region_filter_take() {
        static PAL: LazyLock<MockPlatform> = LazyLock::new(|| {
            let mut mock = MockPlatform::new();

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

            mock.expect_get_all_processors_core()
                .return_const(pal_processors);

            mock
        });

        let builder = ProcessorSetBuilderCore::new(&*PAL);
        let set = builder.different_memory_regions().take_all().unwrap();
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_different_memory_region_filter_take_all() {
        static PAL: LazyLock<MockPlatform> = LazyLock::new(|| {
            let mut mock = MockPlatform::new();

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

            mock.expect_get_all_processors_core()
                .return_const(pal_processors);

            mock
        });

        let builder = ProcessorSetBuilderCore::new(&*PAL);
        let set = builder.different_memory_regions().take_all().unwrap();
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_filter_combinations() {
        static PAL: LazyLock<MockPlatform> = LazyLock::new(|| {
            let mut mock = MockPlatform::new();

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

            mock.expect_get_all_processors_core()
                .return_const(pal_processors);

            mock
        });

        let builder = ProcessorSetBuilderCore::new(&*PAL);
        let except_set = builder.clone().filter(|p| p.id() == 0).take_all().unwrap();
        let set = builder
            .efficiency_processors_only()
            .except(except_set.processors())
            .different_memory_regions()
            .take_all()
            .unwrap();
        assert_eq!(set.len(), 1);
        assert_eq!(set.processors().next().unwrap().id(), 2);
    }

    #[test]
    fn test_same_memory_region_take_two_processors() {
        static PAL: LazyLock<MockPlatform> = LazyLock::new(|| {
            let mut mock = MockPlatform::new();

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

            mock.expect_get_all_processors_core()
                .return_const(pal_processors);

            mock
        });

        let builder = ProcessorSetBuilderCore::new(&*PAL);
        let set = builder.same_memory_region().take(TWO_USIZE).unwrap();
        assert_eq!(set.len(), 2);
        assert!(set.processors().any(|p| p.id() == 1));
        assert!(set.processors().any(|p| p.id() == 2));
    }

    #[test]
    fn test_different_memory_region_and_efficiency_class_filters() {
        static PAL: LazyLock<MockPlatform> = LazyLock::new(|| {
            let mut mock = MockPlatform::new();

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

            mock.expect_get_all_processors_core()
                .return_const(pal_processors);

            mock
        });

        let builder = ProcessorSetBuilderCore::new(&*PAL);
        let set = builder
            .different_memory_regions()
            .efficiency_processors_only()
            .take_all()
            .unwrap();
        assert_eq!(set.len(), 2);
        assert!(set.processors().any(|p| p.id() == 0));
        assert!(set.processors().any(|p| p.id() == 2));
    }

    #[test]
    fn test_performance_processors_but_all_efficiency() {
        static PAL: LazyLock<MockPlatform> = LazyLock::new(|| {
            let mut mock = MockPlatform::new();
            let pal_processors = nonempty![FakeProcessor {
                index: 0,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Efficiency,
            }];
            mock.expect_get_all_processors_core()
                .return_const(pal_processors);
            mock
        });

        let builder = ProcessorSetBuilderCore::new(&*PAL);
        let set = builder.performance_processors_only().take_all();
        assert!(set.is_none(), "No performance processors should be found.");
    }

    #[test]
    fn test_require_different_single_region() {
        static PAL: LazyLock<MockPlatform> = LazyLock::new(|| {
            let mut mock = MockPlatform::new();
            let pal_processors = nonempty![FakeProcessor {
                index: 0,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Efficiency,
            }];
            mock.expect_get_all_processors_core()
                .return_const(pal_processors);
            mock
        });

        let builder = ProcessorSetBuilderCore::new(&*PAL);
        let set = builder.different_memory_regions().take(TWO_USIZE);
        assert!(
            set.is_none(),
            "Should fail because there's not enough distinct memory regions."
        );
    }
}

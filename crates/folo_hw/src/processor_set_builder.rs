use std::{fmt::Debug, num::NonZeroUsize};

use crate::{pal, Processor, ProcessorSet, ProcessorSetBuilderCore};

// This is a specialization of the *Core type for the build target platform. It is the only
// specialization available via the crate's public API surface - other specializations
// exist only for unit testing purposes where the platform is mocked, in which case the
// *Core type is used directly instead of using a newtype wrapper like we have here.

/// Builds a [`ProcessorSet`] based on specified criteria. The default criteria include all
/// available processors.
///
/// You can obtain a builder via [`ProcessorSet::builder()`] or from an existing processor set
/// via [`ProcessorSet::to_builder()`].
#[derive(Clone, Debug)]
pub struct ProcessorSetBuilder {
    core: ProcessorSetBuilderCore<pal::BuildTargetPlatform>,
}

impl ProcessorSetBuilder {
    pub fn new() -> Self {
        Self {
            core: ProcessorSetBuilderCore::new(&pal::BUILD_TARGET_PLATFORM),
        }
    }

    /// Requires that all processors in the set be marked as [performance processors][1].
    ///
    /// [1]: EfficiencyClass::Performance
    pub fn performance_processors_only(self) -> Self {
        Self {
            core: self.core.performance_processors_only(),
        }
    }

    /// Requires that all processors in the set be marked as [efficiency processors][1].
    ///
    /// [1]: EfficiencyClass::Efficiency
    pub fn efficiency_processors_only(self) -> Self {
        Self {
            core: self.core.efficiency_processors_only(),
        }
    }

    /// Requires that all processors in the set be from different memory regions, selecting a
    /// maximum of 1 processor from each memory region.
    pub fn different_memory_regions(self) -> Self {
        Self {
            core: self.core.different_memory_regions(),
        }
    }

    /// Requires that all processors in the set be from the same memory region.
    pub fn same_memory_region(self) -> Self {
        Self {
            core: self.core.same_memory_region(),
        }
    }

    /// Declares a preference that all processors in the set be from different memory regions,
    /// though will select multiple processors from the same memory region as needed to satisfy
    /// the requested processor count (while keeping the spread maximal).
    pub fn prefer_different_memory_regions(self) -> Self {
        Self {
            core: self.core.prefer_different_memory_regions(),
        }
    }

    /// Declares a preference that all processors in the set be from the same memory region,
    /// though will select processors from different memory regions as needed to satisfy the
    /// requested processor count (while keeping the spread minimal).
    pub fn prefer_same_memory_region(self) -> Self {
        Self {
            core: self.core.prefer_same_memory_region(),
        }
    }

    /// Uses a predicate to identify processors that are valid candidates for building the
    /// processor set, with a return value of `bool` indicating that a processor is a valid
    /// candidate for selection into the set.
    ///
    /// The candidates are passed to this function without necessarily first considering all other
    /// conditions - even if this predicate returns `true`, the processor may end up being filtered
    /// out by other conditions. Conversely, some candidates may already be filtered out before
    /// being passed to this predicate.
    pub fn filter(self, predicate: impl Fn(&Processor) -> bool) -> Self {
        Self {
            core: self
                .core
                .filter(|p| predicate(&Into::<Processor>::into(*p))),
        }
    }

    /// Removes specific processors from the set of candidates.
    pub fn except<'a, I>(self, processors: I) -> Self
    where
        I: IntoIterator<Item = &'a Processor>,
    {
        Self {
            core: self.core.except(processors.into_iter().map(|p| p.as_ref())),
        }
    }

    /// Creates a processor set with a specific number of processors that match the
    /// configured criteria.
    ///
    /// If multiple candidate sets are a match, returns an arbitrary one of them. For example, if
    /// there are six valid candidate processors then `take(4)` may return any four of them.
    ///
    /// Returns `None` if there were not enough candidate processors to satisfy the request.
    pub fn take(self, count: NonZeroUsize) -> Option<ProcessorSet> {
        self.core.take(count).map(|p| p.into())
    }

    /// Returns a processor set with all processors that match the configured criteria.
    ///
    /// If multiple alternative non-empty sets are a match, returns an arbitrary one of them.
    /// For example, if specifying only a "same memory region" constraint, it will return all
    /// the processors in an arbitrary memory region with at least one qualifying processor.
    ///
    /// Returns `None` if there were no matching processors to satisfy the request.
    pub fn take_all(self) -> Option<ProcessorSet> {
        self.core.take_all().map(|p| p.into())
    }
}

impl Default for ProcessorSetBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl From<ProcessorSetBuilderCore<pal::BuildTargetPlatform>> for ProcessorSetBuilder {
    fn from(inner: ProcessorSetBuilderCore<pal::BuildTargetPlatform>) -> Self {
        Self { core: inner }
    }
}

#[cfg(not(miri))] // Talking to the operating system is not possible under Miri.
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
        assert!(ProcessorSet::builder()
            .filter(|_| false)
            .take_all()
            .is_none());
    }

    #[test]
    fn except_all() {
        // If we exclude all processors, there should be nothing left.
        assert!(ProcessorSet::builder()
            .except(&ProcessorSet::all().processors())
            .take_all()
            .is_none());
    }
}

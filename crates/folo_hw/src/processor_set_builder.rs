use std::{fmt::Debug, num::NonZeroUsize};

use crate::{pal, Processor, ProcessorSet, ProcessorSetBuilderCore};

// This is a specialization of *Core type for the build target platform. It is the only
// specialization available via the crate's public API surface - other specializations
// exist only for unit testing purposes where the platform is mocked, in which case the
// *Core type is used directly instead of using a newtype wrapper like we have here.

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

    pub fn performance_processors_only(self) -> Self {
        Self {
            core: self.core.performance_processors_only(),
        }
    }

    pub fn efficiency_processors_only(self) -> Self {
        Self {
            core: self.core.efficiency_processors_only(),
        }
    }

    pub fn different_memory_regions(self) -> Self {
        Self {
            core: self.core.different_memory_regions(),
        }
    }

    pub fn same_memory_region(self) -> Self {
        Self {
            core: self.core.same_memory_region(),
        }
    }

    /// Uses a predicate to identify processors that are valid candidates for the set.
    pub fn filter(self, predicate: impl Fn(&Processor) -> bool) -> Self {
        Self {
            core: self
                .core
                .filter(|p| predicate(&Into::<Processor>::into(*p))),
        }
    }

    /// Removes processors from the set of candidates. Useful to help ensure that different
    /// workloads get placed on different processors.
    pub fn except<'a, I>(self, processors: I) -> Self
    where
        I: IntoIterator<Item = &'a Processor>,
    {
        Self {
            core: self.core.except(processors.into_iter().map(|p| p.as_ref())),
        }
    }

    /// Picks a specific number of processors from the set of candidates and returns a
    /// processor set with the requested number of processors that match specified criteria.
    ///
    /// Returns `None` if there were not enough matching processors to satisfy the request.
    pub fn take(self, count: NonZeroUsize) -> Option<ProcessorSet> {
        self.core.take(count).map(|p| p.into())
    }

    /// Returns a processor set with all processors that match the specified criteria.
    ///
    /// If multiple mutually exclusive sets are a match, returns an arbitrary one of them.
    /// For example, if specifying only a "same memory region" constraint, it will return all
    /// the processors in an arbitrary (potentially even random) memory region.
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

use std::{sync::LazyLock, thread};

use itertools::Itertools;
use nonempty::NonEmpty;

use crate::{
    pal::{Platform, PlatformFacade},
    Processor, ProcessorSetBuilder, CURRENT_TRACKER,
};

// https://github.com/cloudhead/nonempty/issues/68
extern crate alloc;

static ALL_PROCESSORS: LazyLock<ProcessorSet> = LazyLock::new(|| {
    ProcessorSetBuilder::default()
        .take_all()
        .expect("there must be at least one processor - how could this code run if not")
});

/// One or more processors present on the system and available for use.
///
/// You can obtain the full set of available processors via [`ProcessorSet::all()`] or specify more
/// fine-grained selection criteria via [`ProcessorSet::builder()`]. You can use
/// [`ProcessorSet::to_builder()`] to further narrow down an existing set.
///
/// One you have a [`ProcessorSet`], you can iterate over [`ProcessorSet::processors()`]
/// to inspect the individual processors in the set or use [`ProcessorSet::spawn_threads()`] to
/// spawn a set of threads pinned to each of the processors in the set, one thread per processor.
/// You may also use [``ProcessorSet::spawn_thread()`] to spawn a single thread pinned to all
/// processors in the set, allowing the thread to move but only between the processors in the set.
///
/// # Changes at runtime
///
/// It is possible that a system will have processors added or removed at runtime. This is not
/// supported - any hardware changes made at runtime will not be visible to the `ProcessorSet`
/// instances. Operations attempted on removed processors may fail with an error or panic or
/// silently misbehave (e.g. threads never starting). Added processors will not be considered a
/// member of any set.
#[derive(Clone, Debug)]
pub struct ProcessorSet {
    processors: NonEmpty<Processor>,
    pal: PlatformFacade,
}

impl ProcessorSet {
    /// Gets a [`ProcessorSet`] referencing all present and available processors on the system.
    ///
    /// This set does not include processors that are not available to the current process or those
    /// that are purely theoretical (e.g. processors that may be added later to the system but are
    /// not yet present).
    pub fn all() -> &'static Self {
        &ALL_PROCESSORS
    }

    /// Creates a builder that can be used to construct a processor set with specific criteria.
    pub fn builder() -> ProcessorSetBuilder {
        ProcessorSetBuilder::default()
    }

    /// Returns a [`ProcessorSetBuilder`] that is narrowed down to all processors in the current
    /// set, to be used to further narrow down the set to a specific subset.
    pub fn to_builder(&self) -> ProcessorSetBuilder {
        ProcessorSetBuilder::new().filter(|p| self.processors.contains(p))
    }

    pub(crate) fn new(processors: NonEmpty<Processor>, pal: PlatformFacade) -> Self {
        Self { processors, pal }
    }

    /// Returns a [`ProcessorSet`] containing the provided processors.
    pub fn from_processors(processors: NonEmpty<Processor>) -> Self {
        let pal = processors.first().pal.clone();
        Self::new(processors, pal)
    }

    /// Returns a [`ProcessorSet`] containing a single processor.
    pub fn from_processor(processor: Processor) -> Self {
        let pal = processor.pal.clone();
        Self::new(NonEmpty::singleton(processor), pal)
    }

    /// Returns the number of processors in the set. A processor set is never empty.
    #[expect(clippy::len_without_is_empty)] // Never empty by definition.
    pub fn len(&self) -> usize {
        self.processors.len()
    }

    /// Returns a reference to a collection containing all the processors in the set.
    pub fn processors(&self) -> &NonEmpty<Processor> {
        &self.processors
    }

    /// Modifies the affinity of the current thread to execute
    /// only on the processors in this processor set.
    ///
    /// # Behavior with multiple processors
    ///
    /// If multiple processors are present in the processor set, they might not be evenly used.
    /// An arbitrary processor may be preferentially used, with others used only when the preferred
    /// processor is otherwise busy.
    pub fn pin_current_thread_to(&self) {
        self.pal.pin_current_thread_to(&self.processors);

        // TODO: Shift tracker to a dependency so we can properly test the set-tracker interactions.
        if self.processors.len() == 1 {
            // If there is only one processor, both the processor and memory region are known.
            let processor = self.processors.first();

            CURRENT_TRACKER.with_borrow_mut(|tracker| {
                tracker.update_pin_status(Some(processor.id()), Some(processor.memory_region_id()));
            })
        } else if self
            .processors
            .iter()
            .map(|p| p.memory_region_id())
            .unique()
            .count()
            == 1
        {
            // All processors are in the same memory region, so we can at least track that.
            let memory_region_id = self.processors.first().memory_region_id();

            CURRENT_TRACKER.with_borrow_mut(|tracker| {
                tracker.update_pin_status(None, Some(memory_region_id));
            })
        } else {
            // We got nothing, have to resolve from scratch every time the data is asked for.
            CURRENT_TRACKER.with_borrow_mut(|tracker| {
                tracker.update_pin_status(None, None);
            })
        }
    }

    /// Spawns one thread for each processor in the set, pinned to that processor,
    /// providing the target processor information to the thread entry point.
    ///
    /// Each spawned thread will only be scheduled on one of the processors in the set. When that
    /// processor is busy, the thread will simply wait for the processor to become available.
    pub fn spawn_threads<E, R>(&self, entrypoint: E) -> Box<[thread::JoinHandle<R>]>
    where
        E: Fn(Processor) -> R + Send + Clone + 'static,
        R: Send + 'static,
    {
        self.processors()
            .iter()
            .map(|p| {
                let p = p.clone();
                let entrypoint = entrypoint.clone();

                thread::spawn(move || {
                    let set = Self::from(p.clone());
                    set.pin_current_thread_to();
                    entrypoint(p)
                })
            })
            .collect::<Vec<_>>()
            .into_boxed_slice()
    }

    /// Spawns a single thread pinned to the set. The thread will only be scheduled to execute on
    /// the processors in the processor set and may freely move between the processors in the set.
    ///
    /// # Behavior with multiple processors
    ///
    /// If multiple processors are present in the processor set, they might not be evenly used.
    /// An arbitrary processor may be preferentially used, with others used only when the preferred
    /// processor is otherwise busy.
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
        Self::from_processor(value)
    }
}

impl From<NonEmpty<Processor>> for ProcessorSet {
    fn from(value: NonEmpty<Processor>) -> Self {
        Self::from_processors(value)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use nonempty::nonempty;

    use crate::{
        pal::{FakeProcessor, MockPlatform},
        EfficiencyClass,
    };

    use super::*;

    #[test]
    fn smoke_test() {
        let mut platform = MockPlatform::new();

        // Pin current thread to entire set.
        platform
            .expect_pin_current_thread_to_core()
            .withf(|p| p.len() == 2)
            .return_const(());

        // Pin spawned single thread to entire set.
        platform
            .expect_pin_current_thread_to_core()
            .withf(|p| p.len() == 2)
            .return_const(());

        // Pin spawned two threads, each to one processor.
        platform
            .expect_pin_current_thread_to_core()
            .withf(|p| p.len() == 1)
            .return_const(());

        platform
            .expect_pin_current_thread_to_core()
            .withf(|p| p.len() == 1)
            .return_const(());

        let platform = PlatformFacade::from_mock(platform);

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

        let processors = pal_processors.map({
            let platform = platform.clone();
            move |p| Processor::new(p.into(), platform.clone())
        });

        let processor_set = ProcessorSet::new(processors, platform);

        // Getters appear to get the expected values.
        assert_eq!(processor_set.len(), 2);

        // Iterator iterates through the expected stuff.
        let mut processor_iter = processor_set.processors().iter();

        let p1 = processor_iter.next().unwrap();
        assert_eq!(p1.id(), 0);
        assert_eq!(p1.memory_region_id(), 0);
        assert_eq!(p1.efficiency_class(), EfficiencyClass::Efficiency);

        let p2 = processor_iter.next().unwrap();
        assert_eq!(p2.id(), 1);
        assert_eq!(p2.memory_region_id(), 0);
        assert_eq!(p2.efficiency_class(), EfficiencyClass::Performance);

        assert!(processor_iter.next().is_none());

        // Pin calls into PAL to execute the pinning.
        processor_set.pin_current_thread_to();

        // spawn_thread() spawns and pins a single thread.
        let threads_spawned = Arc::new(AtomicUsize::new(0));

        processor_set
            .spawn_thread({
                let threads_spawned = Arc::clone(&threads_spawned);
                move |processor_set| {
                    // Verify that we appear to have been given the expected processor set.
                    assert_eq!(processor_set.len(), 2);

                    threads_spawned.fetch_add(1, Ordering::Relaxed);
                }
            })
            .join()
            .unwrap();

        assert_eq!(threads_spawned.load(Ordering::Relaxed), 1);

        // spawn_threads() spawns multiple threads and pins each.
        let threads_spawned = Arc::new(AtomicUsize::new(0));

        processor_set
            .spawn_threads({
                let threads_spawned = Arc::clone(&threads_spawned);
                move |_| {
                    threads_spawned.fetch_add(1, Ordering::Relaxed);
                }
            })
            .into_vec()
            .into_iter()
            .for_each(|h| h.join().unwrap());

        assert_eq!(threads_spawned.load(Ordering::Relaxed), 2);

        // A clone appears to contain the same stuff.
        let cloned_processor_set = processor_set.clone();

        assert_eq!(cloned_processor_set.len(), 2);
    }
}

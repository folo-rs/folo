use std::{fmt::Display, sync::LazyLock, thread};

use itertools::Itertools;
use nonempty::NonEmpty;

use crate::{
    HardwareTrackerClient, HardwareTrackerClientFacade, Processor, ProcessorSetBuilder,
    pal::{Platform, PlatformFacade},
};

// https://github.com/cloudhead/nonempty/issues/68
extern crate alloc;

/// Exposed via `ProcessorSet::all()`.
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
/// You may also use [`ProcessorSet::spawn_thread()`] to spawn a single thread pinned to all
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

    // We use this when we pin a thread, to update the tracker
    // about the current thread's pinning status.
    tracker_client: HardwareTrackerClientFacade,

    pal: PlatformFacade,
}

impl ProcessorSet {
    /// Gets a [`ProcessorSet`] referencing all present and available processors on the system,
    /// up to the resource quota assigned to the process.
    ///
    /// This set does not include processors that are not available to the current process or those
    /// that are purely theoretical (e.g. processors that may be added later to the system but are
    /// not yet present).
    ///
    /// This processor set is not updated if the operating system enforced constraints applied
    /// to the current process change at runtime.
    ///
    /// # Resource quota
    ///
    /// If the current process is assigned a resource quota via operating system constraints, this
    /// set may only include a subset of all processors, to ensure that the quota is not exceeded.
    /// You can use [`ProcessorSetBuilder::ignoring_resource_quota()`][1] to obtain a processor set
    /// that ignores the resource quota.
    ///
    /// [1]: ProcessorSetBuilder::ignoring_resource_quota
    #[must_use]
    pub fn all() -> &'static Self {
        &ALL_PROCESSORS
    }

    /// Creates a builder that can be used to construct a processor set with specific criteria.
    #[cfg_attr(test, mutants::skip)] // Mutates to itself via Default::default().
    #[must_use]
    pub fn builder() -> ProcessorSetBuilder {
        ProcessorSetBuilder::default()
    }

    /// Returns a [`ProcessorSetBuilder`] that is narrowed down to all processors in this
    /// processor set.
    ///
    /// This can be used to further narrow down the set to a specific subset.
    #[must_use]
    pub fn to_builder(&self) -> ProcessorSetBuilder {
        ProcessorSetBuilder::with_internals(self.tracker_client.clone(), self.pal.clone()).filter({
            let processors = self.processors.clone();
            move |p| processors.contains(p)
        })
    }

    #[must_use]
    pub(crate) fn new(
        processors: NonEmpty<Processor>,
        tracker_client: HardwareTrackerClientFacade,
        pal: PlatformFacade,
    ) -> Self {
        Self {
            processors,
            tracker_client,
            pal,
        }
    }

    /// Returns a [`ProcessorSet`] containing the provided processors.
    #[must_use]
    pub fn from_processors(processors: NonEmpty<Processor>) -> Self {
        // Note: this always uses the real platform, so do not use it in tests
        // that rely on ProcessorSets with a mock platform. Given that the only
        // way to create a ProcessorSet with a mock platform is anyway private,
        // this should be easy to control.
        Self::new(
            processors,
            HardwareTrackerClientFacade::real(),
            PlatformFacade::real(),
        )
    }

    /// Returns a [`ProcessorSet`] containing a single processor.
    #[must_use]
    pub fn from_processor(processor: Processor) -> Self {
        // Note: this always uses the real platform, so do not use it in tests
        // that rely on ProcessorSets with a mock platform. Given that the only
        // way to create a ProcessorSet with a mock platform is anyway private,
        // this should be easy to control.
        Self::new(
            NonEmpty::singleton(processor),
            HardwareTrackerClientFacade::real(),
            PlatformFacade::real(),
        )
    }

    /// Returns the number of processors in the set. A processor set is never empty.
    #[expect(clippy::len_without_is_empty, reason = "never empty by definition")]
    #[must_use]
    pub fn len(&self) -> usize {
        self.processors.len()
    }

    /// Returns a reference to a collection containing all the processors in the set.
    #[must_use]
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

        if self.processors.len() == 1 {
            // If there is only one processor, both the processor and memory region are known.
            let processor = self.processors.first();

            self.tracker_client
                .update_pin_status(Some(processor.id()), Some(processor.memory_region_id()));
        } else if self
            .processors
            .iter()
            .map(Processor::memory_region_id)
            .unique()
            .count()
            == 1
        {
            // All processors are in the same memory region, so we can at least track that.
            let memory_region_id = self.processors.first().memory_region_id();

            self.tracker_client
                .update_pin_status(None, Some(memory_region_id));
        } else {
            // We got nothing, have to resolve from scratch every time the data is asked for.
            self.tracker_client.update_pin_status(None, None);
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
            .map(|processor| {
                thread::spawn({
                    let processor = processor.clone();
                    let entrypoint = entrypoint.clone();
                    let tracker_client = self.tracker_client.clone();
                    let pal = self.pal.clone();

                    move || {
                        let set = Self::new(
                            NonEmpty::from_vec(vec![processor.clone()])
                                .expect("we provide 1-item vec as input, so it must be non-empty"),
                            tracker_client.clone(),
                            pal.clone(),
                        );
                        set.pin_current_thread_to();
                        entrypoint(processor)
                    }
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
        E: FnOnce(Self) -> R + Send + 'static,
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

impl Display for ProcessorSet {
    #[cfg_attr(test, mutants::skip)] // We have no API contract to test here.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let list = cpulist::emit(self.processors.iter().map(Processor::id));
        write!(f, " {list} ({} processors)", self.len())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use folo_utils::nz;
    use nonempty::nonempty;

    use crate::{
        EfficiencyClass, MockHardwareTrackerClient,
        pal::{FakeProcessor, MockPlatform},
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

        let processors = pal_processors.map(move |p| Processor::new(p.into()));

        let mut tracker_client = MockHardwareTrackerClient::new();

        tracker_client
            .expect_update_pin_status()
            // Once for entrypoint thread, once for spawn_thread().
            .times(2)
            .withf(|processor, memory_region| {
                processor.is_none() && matches!(memory_region, Some(0))
            })
            .return_const(());

        // Once for each of the threads in spawn_threads().
        tracker_client
            .expect_update_pin_status()
            .times(1)
            .withf(|processor, memory_region| {
                matches!(processor, Some(0)) && matches!(memory_region, Some(0))
            })
            .return_const(());

        tracker_client
            .expect_update_pin_status()
            .times(1)
            .withf(|processor, memory_region| {
                matches!(processor, Some(1)) && matches!(memory_region, Some(0))
            })
            .return_const(());

        let tracker_client = HardwareTrackerClientFacade::from_mock(tracker_client);

        let processor_set = ProcessorSet::new(processors, tracker_client, platform);

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

        // We create one clone for the worker thread to use.
        // We do not create any additional clones (spawn_thread() is guaranteed to only spawn 1).
        let threads_spawned_clone = Arc::clone(&threads_spawned);

        let non_copy_value = "foo".to_string();

        processor_set
            .spawn_thread({
                fn process_string(_s: String) {}

                move |processor_set| {
                    // Verify that we appear to have been given the expected processor set.
                    assert_eq!(processor_set.len(), 2);

                    // We prove that the callback can use !Copy values by calling this fn.
                    process_string(non_copy_value);

                    threads_spawned_clone.fetch_add(1, Ordering::Relaxed);
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

    #[cfg(not(miri))] // Miri does not support talking to the real platform.
    #[test]
    fn from_processor_preserves_processor() {
        let one = ProcessorSet::builder().take(nz!(1)).unwrap();
        let processor = one.processors().first().clone();
        let one_again = ProcessorSet::from_processor(processor);

        assert_eq!(one_again.len(), 1);
        assert_eq!(one_again.processors().first(), one.processors().first());
    }

    #[cfg(not(miri))] // Miri does not support talking to the real platform.
    #[test]
    fn from_processors_preserves_processors() {
        let all = ProcessorSet::builder().take_all().unwrap();
        let processors = NonEmpty::collect(all.processors().iter().cloned()).unwrap();
        let all_again = ProcessorSet::from_processors(processors);

        assert_eq!(all_again.len(), all.len());

        // The public API does not make guarantees about the order of processors, so we do this
        // clumsy contains() based check that does not make assumptions about the order.
        for processor in all.processors() {
            assert!(all_again.processors().contains(processor));
        }
    }

    #[cfg(not(miri))] // Miri does not support talking to the real platform.
    #[test]
    fn to_builder_preserves_processors() {
        let set = ProcessorSet::builder().take(nz!(1)).unwrap();

        let builder = set.to_builder();

        let set2 = builder.take_all().unwrap();
        assert_eq!(set2.len(), 1);

        let processor1 = set.processors().first();
        let processor2 = set2.processors().first();

        assert_eq!(processor1, processor2);
    }

    #[cfg(not(miri))] // Miri does not support talking to the real platform.
    #[test]
    fn inherit_on_pinned() {
        thread::spawn(|| {
            let one = ProcessorSet::builder().take(nz!(1)).unwrap();

            one.pin_current_thread_to();

            // Potential false negative here if the system only has one processor but that's fine.
            let current_thread_allowed = ProcessorSet::builder()
                .where_available_for_current_thread()
                .take_all()
                .unwrap();

            assert_eq!(current_thread_allowed.len(), 1);
            assert_eq!(
                current_thread_allowed.processors().first(),
                one.processors().first()
            );
        })
        .join()
        .unwrap();
    }
}

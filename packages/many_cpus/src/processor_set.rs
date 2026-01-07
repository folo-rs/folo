use std::fmt::Display;
use std::num::NonZero;
use std::thread;

use itertools::Itertools;
use nonempty::NonEmpty;

use crate::pal::{Platform, PlatformFacade};
use crate::{Processor, ProcessorSetBuilder, SystemHardware};

/// One or more processors present on the system and available for use.
///
/// The typical way to obtain a `ProcessorSet` is to call [`SystemHardware::processors()`].
/// You may reduce a `ProcessorSet` to a smaller subset by using [`ProcessorSet::to_builder()`].
///
/// Once you have a `ProcessorSet`, you can iterate over [`ProcessorSet::processors()`]
/// to inspect the individual processors in the set. There are several ways to apply the processor
/// set to threads:
///
/// 1. If you created a thread manually, you can call [`ProcessorSet::pin_current_thread_to()`],
///    which will configure the thread to only run on the processors in the set.
/// 2. You can use [`ProcessorSet::spawn_thread()`] to spawn a thread pinned to the set, which
///    will only be scheduled to run on the processors in the set.
/// 3. You can use [`ProcessorSet::spawn_threads()`] to spawn a set of threads, with one thread
///    for each of the processors in the set. Each thread will be pinned to its own processor.
#[doc = include_str!("../docs/snippets/changes_at_runtime.md")]
/// [`SystemHardware::processors()`]: crate::SystemHardware::processors
/// [`ProcessorSet::to_builder()`]: crate::ProcessorSet::to_builder
#[derive(Clone, Debug)]
pub struct ProcessorSet {
    processors: NonEmpty<Processor>,

    /// The hardware instance this processor set is associated with.
    /// Used to update pin status when a thread is pinned to this set.
    hardware: SystemHardware,

    pal: PlatformFacade,
}

impl ProcessorSet {
    #[must_use]
    pub(crate) fn new(
        processors: NonEmpty<Processor>,
        hardware: SystemHardware,
        pal: PlatformFacade,
    ) -> Self {
        Self {
            processors,
            hardware,
            pal,
        }
    }

    /// Returns a [`ProcessorSetBuilder`] that is narrowed down to all processors in this
    /// processor set.
    ///
    /// This can be used to further narrow down the set to a specific subset.
    ///
    /// # Example
    ///
    /// ```
    /// use std::num::NonZero;
    ///
    /// use many_cpus::SystemHardware;
    ///
    /// let hardware = SystemHardware::current();
    /// let all = hardware.processors();
    ///
    /// // Narrow down to just performance processors.
    /// let performance = all.to_builder().performance_processors_only().take_all();
    /// ```
    #[must_use]
    pub fn to_builder(&self) -> ProcessorSetBuilder {
        ProcessorSetBuilder::with_internals(self.hardware.clone(), self.pal.clone()).filter({
            let processors = self.processors.clone();
            move |p| processors.contains(p)
        })
    }

    /// Returns a subset of this processor set containing the specified number of processors.
    ///
    /// This is a convenience method equivalent to `.to_builder().take(count)`.
    ///
    /// If multiple subsets are possible, returns an arbitrary one of them. For example, if
    /// this set contains six processors then `take(4)` may return any four of them.
    ///
    /// Returns `None` if there are not enough processors in the set to satisfy the request.
    ///
    /// # Example
    ///
    /// ```
    /// use many_cpus::SystemHardware;
    /// use new_zealand::nz;
    ///
    /// let hardware = SystemHardware::current();
    /// let all = hardware.processors();
    ///
    /// // Take 2 processors from the set.
    /// if let Some(subset) = all.take(nz!(2)) {
    ///     println!("Selected {} processors", subset.len());
    /// }
    /// ```
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder to builder.
    pub fn take(&self, count: NonZero<usize>) -> Option<Self> {
        self.to_builder().take(count)
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
            SystemHardware::current().clone(),
            PlatformFacade::target(),
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
            SystemHardware::current().clone(),
            PlatformFacade::target(),
        )
    }

    /// Returns the number of processors in the set. A processor set is never empty.
    #[must_use]
    #[inline]
    #[expect(clippy::len_without_is_empty, reason = "never empty by definition")]
    pub fn len(&self) -> usize {
        self.processors.len()
    }

    /// Returns an iterator over references to the processors in the set.
    #[must_use]
    #[inline]
    pub fn iter(&self) -> nonempty::Iter<'_, Processor> {
        self.processors.iter()
    }

    /// Returns a reference to a collection containing all the processors in the set.
    #[must_use]
    #[inline]
    pub fn processors(&self) -> &NonEmpty<Processor> {
        &self.processors
    }

    /// Consumes the processor set and returns the collection of processors.
    #[must_use]
    #[inline]
    pub(crate) fn into_processors(self) -> NonEmpty<Processor> {
        self.processors
    }

    /// Modifies the affinity of the current thread to execute
    /// only on the processors in this processor set.
    ///
    /// # Example
    ///
    /// ```
    /// use std::num::NonZero;
    /// use std::thread;
    ///
    /// use many_cpus::SystemHardware;
    ///
    /// let hw = SystemHardware::current();
    ///
    /// // Create a processor set with specific processors
    /// let processors = hw
    ///     .processors()
    ///     .to_builder()
    ///     .take(NonZero::new(2).unwrap())
    ///     .unwrap_or_else(|| hw.processors());
    ///
    /// // Pin the current thread to those processors
    /// processors.pin_current_thread_to();
    ///
    /// println!("Thread pinned to {} processor(s)", processors.len());
    ///
    /// // For single-processor sets, we can verify processor-level pinning
    /// let single_processor = hw
    ///     .processors()
    ///     .to_builder()
    ///     .take(NonZero::new(1).unwrap())
    ///     .unwrap();
    ///
    /// thread::spawn(move || {
    ///     single_processor.pin_current_thread_to();
    ///
    ///     // This thread is now pinned to exactly one processor.
    ///     assert!(SystemHardware::current().is_thread_processor_pinned());
    ///     println!(
    ///         "Thread pinned to processor {}",
    ///         single_processor.processors().first().id()
    ///     );
    /// })
    /// .join()
    /// .unwrap();
    /// ```
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

            self.hardware
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

            self.hardware
                .update_pin_status(None, Some(memory_region_id));
        } else {
            // We got nothing, have to resolve from scratch every time the data is asked for.
            self.hardware.update_pin_status(None, None);
        }
    }

    /// Spawns one thread for each processor in the set, pinned to that processor,
    /// providing the target processor information to the thread entry point.
    ///
    /// Each spawned thread will only be scheduled on one of the processors in the set. When that
    /// processor is busy, the thread will simply wait for the processor to become available.
    #[cfg_attr(test, mutants::skip)] // Cargo-mutants does not understand this signature - every mutation is unviable waste of time.
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
                    let hardware = self.hardware.clone();
                    let pal = self.pal.clone();

                    move || {
                        let set = Self::new(
                            NonEmpty::from_vec(vec![processor.clone()])
                                .expect("we provide 1-item vec as input, so it must be non-empty"),
                            hardware.clone(),
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
    /// # Example
    ///
    /// ```
    /// use std::num::NonZero;
    ///
    /// use many_cpus::SystemHardware;
    ///
    /// let hw = SystemHardware::current();
    ///
    /// // Create a processor set with multiple processors
    /// let processors = hw
    ///     .processors()
    ///     .to_builder()
    ///     .take(NonZero::new(2).unwrap())
    ///     .unwrap_or_else(|| hw.processors());
    ///
    /// // Spawn a single thread that can use any processor in the set
    /// let handle = processors.spawn_thread(|processor_set| {
    ///     println!("Thread can execute on {} processors", processor_set.len());
    ///
    ///     // The thread can move between processors in the set
    ///     // but is restricted to only those processors.
    ///     42
    /// });
    ///
    /// let result = handle.join().unwrap();
    /// assert_eq!(result, 42);
    /// ```
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
    #[inline]
    fn from(value: Processor) -> Self {
        Self::from_processor(value)
    }
}

impl From<NonEmpty<Processor>> for ProcessorSet {
    #[inline]
    fn from(value: NonEmpty<Processor>) -> Self {
        Self::from_processors(value)
    }
}

impl From<ProcessorSet> for ProcessorSetBuilder {
    /// Converts a [`ProcessorSet`] into a [`ProcessorSetBuilder`] for further customization.
    ///
    /// This is equivalent to calling [`ProcessorSet::to_builder()`].
    #[inline]
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder to to_builder.
    fn from(value: ProcessorSet) -> Self {
        value.to_builder()
    }
}

impl From<&ProcessorSet> for ProcessorSetBuilder {
    /// Converts a [`ProcessorSet`] reference into a [`ProcessorSetBuilder`] for further
    /// customization.
    ///
    /// This is equivalent to calling [`ProcessorSet::to_builder()`].
    #[inline]
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder to to_builder.
    fn from(value: &ProcessorSet) -> Self {
        value.to_builder()
    }
}

impl AsRef<Self> for ProcessorSet {
    #[inline]
    #[cfg_attr(test, mutants::skip)] // Trivial.
    fn as_ref(&self) -> &Self {
        self
    }
}

impl Display for ProcessorSet {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let list = cpulist::emit(self.processors.iter().map(Processor::id));
        write!(f, " {list} ({} processors)", self.len())
    }
}

impl IntoIterator for ProcessorSet {
    type IntoIter = <NonEmpty<Processor> as IntoIterator>::IntoIter;
    type Item = Processor;

    /// Consumes the processor set and returns an iterator over the processors.
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.processors.into_iter()
    }
}

impl<'a> IntoIterator for &'a ProcessorSet {
    type IntoIter = nonempty::Iter<'a, Processor>;
    type Item = &'a Processor;

    /// Returns an iterator over references to the processors in the set.
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.processors.iter()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use new_zealand::nz;

    use super::*;
    use crate::EfficiencyClass;
    use crate::fake::{HardwareBuilder, ProcessorBuilder};

    #[test]
    fn smoke_test() {
        // Use fake hardware for pin status tracking and processor configuration.
        let hardware = SystemHardware::fake(
            HardwareBuilder::new()
                .processor(ProcessorBuilder::new().efficiency_class(EfficiencyClass::Efficiency))
                .processor(ProcessorBuilder::new().efficiency_class(EfficiencyClass::Performance)),
        );

        let processor_set = hardware.processors();

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

        // Since we pinned to 2 processors in same memory region,
        // processor is not pinned but memory region is.
        assert!(!hardware.is_thread_processor_pinned());
        assert!(hardware.is_thread_memory_region_pinned());

        // spawn_thread() spawns and pins a single thread.
        let threads_spawned = Arc::new(AtomicUsize::new(0));

        // We create one clone for the worker thread to use.
        // We do not create any additional clones (spawn_thread() is guaranteed to only spawn 1).
        let threads_spawned_clone = Arc::clone(&threads_spawned);

        let non_copy_value = "foo".to_string();

        processor_set
            .spawn_thread({
                fn process_string(_s: String) {}

                move |spawned_processor_set| {
                    // Verify that we appear to have been given the expected processor set.
                    assert_eq!(spawned_processor_set.len(), 2);

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

    #[test]
    fn display_shows_cpulist_and_count() {
        // Use fake hardware for the Display test - no actual pinning needed.
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(4), nz!(1)));

        let processor_set = hardware.processors();

        let display_output = processor_set.to_string();

        // The display format is " <cpulist> (<count> processors)".
        assert!(
            display_output.contains("0-3"),
            "display output should contain cpulist format: {display_output}"
        );
        assert!(
            display_output.contains("4 processors"),
            "display output should contain processor count: {display_output}"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
    fn from_processor_preserves_processor() {
        use crate::SystemHardware;

        let one = SystemHardware::current()
            .processors()
            .to_builder()
            .take(nz!(1))
            .unwrap();
        let processor = one.processors().first().clone();
        let one_again = ProcessorSet::from_processor(processor.clone());

        assert_eq!(one_again.len(), 1);
        assert_eq!(one_again.processors().first(), one.processors().first());

        let one_again = ProcessorSet::from(processor);

        assert_eq!(one_again.len(), 1);
        assert_eq!(one_again.processors().first(), one.processors().first());
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
    fn from_processors_preserves_processors() {
        use crate::SystemHardware;

        let hw = SystemHardware::current();

        if hw.processors().len() < 2 {
            eprintln!("Skipping test because there are not enough processors");
            return;
        }

        let two = hw.processors().take(nz!(2)).unwrap();
        let processors = NonEmpty::collect(two.processors().iter().cloned()).unwrap();
        let two_again = ProcessorSet::from_processors(processors.clone());

        assert_eq!(two_again.len(), two.len());

        // The public API does not make guarantees about the order of processors, so we do this
        // clumsy contains() based check that does not make assumptions about the order.
        for processor in two.processors() {
            assert!(two_again.processors().contains(processor));
        }

        let two_again = ProcessorSet::from(processors);

        assert_eq!(two_again.len(), two.len());

        // The public API does not make guarantees about the order of processors, so we do this
        // clumsy contains() based check that does not make assumptions about the order.
        for processor in two.processors() {
            assert!(two_again.processors().contains(processor));
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
    fn from_processors_with_one_preserves_processors() {
        use crate::SystemHardware;

        let one = SystemHardware::current()
            .processors()
            .to_builder()
            .take(nz!(1))
            .unwrap();
        let processors = one.processors().clone();
        let one_again = ProcessorSet::from_processors(processors);

        assert_eq!(one_again.len(), 1);
        assert_eq!(one_again.processors().first(), one.processors().first());
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
    fn to_builder_preserves_processors() {
        use crate::SystemHardware;

        let set = SystemHardware::current()
            .processors()
            .to_builder()
            .take(nz!(1))
            .unwrap();

        let builder = set.to_builder();

        let set2 = builder.take_all().unwrap();
        assert_eq!(set2.len(), 1);

        let processor1 = set.processors().first();
        let processor2 = set2.processors().first();

        assert_eq!(processor1, processor2);
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
    fn inherit_on_pinned() {
        use crate::SystemHardware;

        thread::spawn(|| {
            let hw = SystemHardware::current();
            let one = hw.processors().take(nz!(1)).unwrap();

            one.pin_current_thread_to();

            // Potential false negative here if the system only has one processor but that's fine.
            let current_thread_allowed = hw
                .processors()
                .to_builder()
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

/// Fallback PAL integration tests - these test the integration between `ProcessorSet`
/// and the fallback platform abstraction layer.
#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests_fallback {
    use std::thread;

    use new_zealand::nz;

    use crate::pal::fallback::{BUILD_TARGET_PLATFORM, ProcessorImpl as FallbackProcessor};
    use crate::pal::{PlatformFacade, ProcessorFacade};
    use crate::{Processor, ProcessorSet, SystemHardware};

    /// Creates a fallback PAL and a cloned hardware instance for testing.
    fn fallback_pal_and_hw() -> (PlatformFacade, SystemHardware) {
        let pal = PlatformFacade::Fallback(&BUILD_TARGET_PLATFORM);
        let hw = SystemHardware::current().clone();
        (pal, hw)
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
    fn smoke_test() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let (pal, hw) = fallback_pal_and_hw();

        let processors = nonempty::nonempty![
            Processor::new(ProcessorFacade::Fallback(FallbackProcessor::new(0))),
            Processor::new(ProcessorFacade::Fallback(FallbackProcessor::new(1)))
        ];

        let processor_set = ProcessorSet::new(processors, hw, pal);

        assert_eq!(processor_set.len(), 2);

        let mut processor_iter = processor_set.processors().iter();

        let p1 = processor_iter.next().unwrap();
        assert_eq!(p1.id(), 0);
        assert_eq!(p1.memory_region_id(), 0);

        let p2 = processor_iter.next().unwrap();
        assert_eq!(p2.id(), 1);
        assert_eq!(p2.memory_region_id(), 0);

        assert!(processor_iter.next().is_none());

        processor_set.pin_current_thread_to();

        let threads_spawned = Arc::new(AtomicUsize::new(0));
        let threads_spawned_clone = Arc::clone(&threads_spawned);

        processor_set
            .spawn_thread(move |_| {
                threads_spawned_clone.fetch_add(1, Ordering::Relaxed);
            })
            .join()
            .unwrap();

        assert_eq!(threads_spawned.load(Ordering::Relaxed), 1);
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
    fn pin_updates_tracker() {
        let (pal, hw) = fallback_pal_and_hw();

        let processors = nonempty::nonempty![Processor::new(ProcessorFacade::Fallback(
            FallbackProcessor::new(0)
        ))];

        let processor_set = ProcessorSet::new(processors, hw, pal);

        processor_set.pin_current_thread_to();

        // Verify that the tracker now reports pinning.
        let hw = SystemHardware::current();
        assert!(hw.is_thread_processor_pinned());
        assert!(hw.is_thread_memory_region_pinned());
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
    fn spawn_thread_pins_correctly() {
        use std::sync::mpsc;

        let (pal, hw) = fallback_pal_and_hw();

        let processors = nonempty::nonempty![Processor::new(ProcessorFacade::Fallback(
            FallbackProcessor::new(0)
        ))];

        let processor_set = ProcessorSet::new(processors, hw, pal);

        let (tx, rx) = mpsc::channel();

        processor_set
            .spawn_thread(move |_| {
                let is_pinned = SystemHardware::current().is_thread_processor_pinned();
                tx.send(is_pinned).unwrap();
            })
            .join()
            .unwrap();

        let is_pinned = rx.recv().unwrap();
        assert!(is_pinned);
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
    fn spawn_threads_pins_all_correctly() {
        let (pal, hw) = fallback_pal_and_hw();

        let processors = nonempty::nonempty![
            Processor::new(ProcessorFacade::Fallback(FallbackProcessor::new(0))),
            Processor::new(ProcessorFacade::Fallback(FallbackProcessor::new(1)))
        ];

        let processor_set = ProcessorSet::new(processors, hw, pal);

        let threads =
            processor_set.spawn_threads(|_| SystemHardware::current().is_thread_processor_pinned());

        for thread in threads {
            let is_pinned = thread.join().unwrap();
            assert!(is_pinned);
        }
    }
    #[test]
    #[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
    fn from_processor_preserves_data() {
        let processor = Processor::new(ProcessorFacade::Fallback(FallbackProcessor::new(0)));

        // We need to ensure the tracker and PAL singletons are using fallback.
        // For this test, we just verify basic construction works.
        let processor_id = processor.id();

        let processor_set = ProcessorSet::from_processor(processor);

        assert_eq!(processor_set.len(), 1);
        assert_eq!(processor_set.processors().first().id(), processor_id);
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
    fn inherit_on_pinned() {
        use std::thread;

        use crate::SystemHardware;

        thread::spawn(|| {
            let hw = SystemHardware::current();
            let one = hw.processors().take(nz!(1)).unwrap();

            one.pin_current_thread_to();

            let current_thread_allowed = hw
                .processors()
                .to_builder()
                .where_available_for_current_thread()
                .take_all()
                .unwrap();

            // After pinning to one processor, we should see only that processor
            // when we inherit the affinity.
            assert_eq!(current_thread_allowed.len(), 1);
        })
        .join()
        .unwrap();
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
    fn to_builder_preserves_processors() {
        use crate::SystemHardware;

        let set = SystemHardware::current()
            .processors()
            .to_builder()
            .take(nz!(1))
            .unwrap();

        let builder = set.to_builder();

        let set2 = builder.take_all().unwrap();
        assert_eq!(set2.len(), 1);

        let processor1 = set.processors().first();
        let processor2 = set2.processors().first();

        assert_eq!(processor1.id(), processor2.id());
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
    fn take_all_returns_all_processors() {
        use crate::SystemHardware;

        let all_set = SystemHardware::current().processors();

        let expected_count = thread::available_parallelism()
            .map(std::num::NonZero::get)
            .unwrap_or(1);

        assert_eq!(all_set.len(), expected_count);
    }
}

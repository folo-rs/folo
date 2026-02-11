use std::fmt::Display;
use std::num::NonZero;
use std::thread;

use itertools::Itertools;
use nonempty::NonEmpty;

use crate::pal::Platform;
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
///
/// [`SystemHardware::processors()`]: crate::SystemHardware::processors
/// [`ProcessorSet::to_builder()`]: crate::ProcessorSet::to_builder
#[doc = include_str!("../docs/snippets/changes_at_runtime.md")]
#[derive(Clone, Debug)]
pub struct ProcessorSet {
    processors: NonEmpty<Processor>,

    /// The hardware instance this processor set is associated with.
    /// Used to update pin status when a thread is pinned to this set and to access the platform.
    hardware: SystemHardware,
}

impl ProcessorSet {
    #[must_use]
    pub(crate) fn new(processors: NonEmpty<Processor>, hardware: SystemHardware) -> Self {
        Self {
            processors,
            hardware,
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
        ProcessorSetBuilder::with_internals(self.hardware.clone())
            .with_source_processors(&self.processors)
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

    /// Decomposes the processor set into individual single-processor sets.
    ///
    /// Each processor in the original set becomes its own `ProcessorSet` containing
    /// exactly one processor. The number of resulting sets equals [`ProcessorSet::len()`].
    /// Returns a subset of this processor set containing only processors that satisfy the given
    /// predicate.
    ///
    /// # Example
    ///
    /// ```
    /// use many_cpus::SystemHardware;
    ///
    /// let hardware = SystemHardware::current();
    /// let all = hardware.processors();
    /// let original_len = all.len();
    ///
    /// let individual = all.decompose();
    ///
    /// assert_eq!(individual.len(), original_len);
    ///
    /// for set in &individual {
    ///     assert_eq!(set.len(), 1);
    /// }
    /// ```
    #[must_use]
    pub fn decompose(&self) -> NonEmpty<Self> {
        self.processors
            .clone()
            .map(|processor| Self::new(NonEmpty::singleton(processor), self.hardware.clone()))
    }

    /// This is a convenience method equivalent to `.to_builder().filter(predicate).take_all()`.
    ///
    /// Returns `None` if no processors in the set satisfy the predicate.
    ///
    /// # Example
    ///
    /// ```
    /// use many_cpus::{EfficiencyClass, SystemHardware};
    /// use new_zealand::nz;
    ///
    /// let hardware = SystemHardware::current();
    /// let all = hardware.processors();
    ///
    /// // Filter to only performance processors.
    /// if let Some(performance) = all.filter(|p| p.efficiency_class() == EfficiencyClass::Performance)
    /// {
    ///     println!("Selected {} performance processors", performance.len());
    /// }
    ///
    /// // Filter to processors with even IDs.
    /// if let Some(even_ids) = all.filter(|p| p.id() % 2 == 0) {
    ///     println!("Selected {} processors with even IDs", even_ids.len());
    /// }
    /// ```
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder to builder.
    pub fn filter(&self, predicate: impl Fn(&Processor) -> bool) -> Option<Self> {
        self.to_builder().filter(predicate).take_all()
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
    /// use new_zealand::nz;
    ///
    /// let hw = SystemHardware::current();
    ///
    /// // Create a processor set with specific processors
    /// let processors = hw
    ///     .processors()
    ///     .take(nz!(2))
    ///     .unwrap_or_else(|| hw.processors());
    ///
    /// // Pin the current thread to those processors
    /// processors.pin_current_thread_to();
    ///
    /// println!("Thread pinned to {} processor(s)", processors.len());
    ///
    /// // For single-processor sets, we can verify processor-level pinning
    /// let single_processor = hw.processors().take(nz!(1)).unwrap();
    ///
    /// thread::spawn(move || {
    ///     single_processor.pin_current_thread_to();
    ///
    ///     // This thread is now pinned to exactly one processor.
    ///     assert!(hw.is_thread_processor_pinned());
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
        self.hardware
            .platform()
            .pin_current_thread_to(&self.processors);

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

                    move || {
                        let set =
                            Self::new(NonEmpty::singleton(processor.clone()), hardware.clone());
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
    /// use new_zealand::nz;
    ///
    /// let hw = SystemHardware::current();
    ///
    /// // Create a processor set with multiple processors
    /// let processors = hw
    ///     .processors()
    ///     .take(nz!(2))
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
    fn iter_returns_all_processors() {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(3), nz!(1)));
        let processor_set = hardware.processors();

        let ids_via_iter: foldhash::HashSet<_> = processor_set.iter().map(Processor::id).collect();

        assert_eq!(ids_via_iter.len(), 3);
        assert!(ids_via_iter.contains(&0));
        assert!(ids_via_iter.contains(&1));
        assert!(ids_via_iter.contains(&2));
    }

    #[test]
    fn into_iterator_for_owned_processor_set() {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(3), nz!(1)));
        let processor_set = hardware.processors();

        // Use IntoIterator for owned ProcessorSet.
        let ids: foldhash::HashSet<_> = processor_set.into_iter().map(|p| p.id()).collect();

        assert_eq!(ids.len(), 3);
        assert!(ids.contains(&0));
        assert!(ids.contains(&1));
        assert!(ids.contains(&2));
    }

    #[test]
    fn into_iterator_for_ref_processor_set() {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(3), nz!(1)));
        let processor_set = hardware.processors();

        // Use IntoIterator for &ProcessorSet (via for loop syntax).
        let mut ids = foldhash::HashSet::default();
        for processor in &processor_set {
            ids.insert(processor.id());
        }

        assert_eq!(ids.len(), 3);
        assert!(ids.contains(&0));
        assert!(ids.contains(&1));
        assert!(ids.contains(&2));

        // ProcessorSet should still be usable after iteration (not consumed).
        assert_eq!(processor_set.len(), 3);
    }

    #[test]
    fn from_owned_processor_set_creates_builder() {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(4), nz!(1)));
        let processor_set = hardware.processors();

        // Use From<ProcessorSet> for ProcessorSetBuilder.
        let builder: ProcessorSetBuilder = processor_set.into();

        // The builder should be able to produce a processor set with the same processors.
        let rebuilt = builder.take_all().unwrap();
        assert_eq!(rebuilt.len(), 4);
    }

    #[test]
    fn from_ref_processor_set_creates_builder() {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(4), nz!(1)));
        let processor_set = hardware.processors();

        // Use From<&ProcessorSet> for ProcessorSetBuilder.
        let builder: ProcessorSetBuilder = (&processor_set).into();

        // The builder should be able to produce a processor set with the same processors.
        let rebuilt = builder.take_all().unwrap();
        assert_eq!(rebuilt.len(), 4);

        // Original processor set should still be usable.
        assert_eq!(processor_set.len(), 4);
    }

    #[test]
    fn decompose_single_processor() {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
        let processor_set = hardware.processors();

        let decomposed = processor_set.decompose();

        assert_eq!(decomposed.len(), 1);
        assert_eq!(decomposed.first().len(), 1);
        assert_eq!(decomposed.first().processors().first().id(), 0);
    }

    #[test]
    fn decompose_multiple_processors() {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(3), nz!(1)));
        let processor_set = hardware.processors();

        let decomposed = processor_set.decompose();

        assert_eq!(decomposed.len(), 3);

        for set in &decomposed {
            assert_eq!(set.len(), 1);
        }

        let ids: foldhash::HashSet<_> = decomposed
            .iter()
            .map(|set| set.processors().first().id())
            .collect();

        assert_eq!(ids.len(), 3);
        assert!(ids.contains(&0));
        assert!(ids.contains(&1));
        assert!(ids.contains(&2));
    }

    #[test]
    fn decompose_preserves_memory_regions() {
        let hardware = SystemHardware::fake(
            HardwareBuilder::new()
                .processor(ProcessorBuilder::new().id(0).memory_region(0))
                .processor(ProcessorBuilder::new().id(1).memory_region(1)),
        );

        let decomposed = hardware.processors().decompose();

        assert_eq!(decomposed.len(), 2);

        // Collect (id, memory_region_id) pairs without assuming order.
        let pairs: foldhash::HashSet<_> = decomposed
            .iter()
            .map(|set| {
                assert_eq!(set.len(), 1);
                let p = set.processors().first();
                (p.id(), p.memory_region_id())
            })
            .collect();

        assert!(pairs.contains(&(0, 0)));
        assert!(pairs.contains(&(1, 1)));
    }

    #[test]
    fn decompose_sets_can_pin() {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(2), nz!(1)));
        let decomposed = hardware.processors().decompose();

        // Each decomposed set should be able to pin a thread.
        decomposed.first().pin_current_thread_to();

        assert!(hardware.is_thread_processor_pinned());
        assert!(hardware.is_thread_memory_region_pinned());
    }

    #[test]
    fn pin_to_multiple_memory_regions_clears_region_pin() {
        // Create processors across two memory regions.
        let hardware = SystemHardware::fake(
            HardwareBuilder::new()
                .processor(ProcessorBuilder::new().id(0).memory_region(0))
                .processor(ProcessorBuilder::new().id(1).memory_region(1)),
        );

        // Get a processor set containing processors from both memory regions.
        let processor_set = hardware.processors();
        assert_eq!(processor_set.len(), 2);

        // Pin to the set with multiple memory regions.
        processor_set.pin_current_thread_to();

        // Neither processor nor memory region should be considered pinned because
        // we are pinned to processors across multiple memory regions.
        assert!(!hardware.is_thread_processor_pinned());
        assert!(!hardware.is_thread_memory_region_pinned());
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
    fn to_builder_preserves_processors() {
        let set = SystemHardware::current().processors().take(nz!(1)).unwrap();

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

    #[test]
    fn filter_basic() {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(6), nz!(1)));

        let all = hardware.processors();
        assert_eq!(all.len(), 6);

        let even_ids = all.filter(|p| p.id() % 2 == 0).unwrap();
        assert_eq!(even_ids.len(), 3);

        let ids: foldhash::HashSet<_> = even_ids.iter().map(Processor::id).collect();
        assert!(ids.contains(&0));
        assert!(ids.contains(&2));
        assert!(ids.contains(&4));
    }

    #[test]
    fn filter_returns_none_when_no_matches() {
        let hardware = SystemHardware::fake(
            HardwareBuilder::new()
                .processor(
                    ProcessorBuilder::new()
                        .id(0)
                        .efficiency_class(EfficiencyClass::Efficiency),
                )
                .processor(
                    ProcessorBuilder::new()
                        .id(1)
                        .efficiency_class(EfficiencyClass::Efficiency),
                ),
        );

        let all = hardware.processors();

        let performance = all.filter(|p| p.efficiency_class() == EfficiencyClass::Performance);
        assert!(performance.is_none());
    }

    #[test]
    fn filter_on_subset() {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(8), nz!(1)));

        let all = hardware.processors();

        let first_half = all.take(nz!(4)).unwrap();
        assert_eq!(first_half.len(), 4);

        let even_from_first_half = first_half.filter(|p| p.id() % 2 == 0).unwrap();

        // The filter should only include even IDs that were in the first_half.
        let first_half_ids: foldhash::HashSet<_> = first_half.iter().map(Processor::id).collect();
        let even_ids: foldhash::HashSet<_> =
            even_from_first_half.iter().map(Processor::id).collect();

        // All IDs in the filtered set must be even.
        for id in &even_ids {
            assert_eq!(id % 2, 0);
        }

        // All IDs in the filtered set must have been in the original first_half.
        for id in &even_ids {
            assert!(first_half_ids.contains(id));
        }

        // The filtered set should contain at least one processor.
        assert!(!even_ids.is_empty());
    }
}

/// Fallback PAL integration tests - these test the integration between `ProcessorSet`
/// and the fallback platform abstraction layer.
#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests_fallback {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, mpsc};
    use std::thread;

    use new_zealand::nz;

    use crate::SystemHardware;

    #[test]
    #[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
    fn smoke_test() {
        let hw = SystemHardware::fallback();
        let processor_set = hw.processors();

        assert!(processor_set.len() >= 1);

        let mut processor_iter = processor_set.processors().iter();

        let p1 = processor_iter.next().unwrap();
        assert_eq!(p1.id(), 0);
        assert_eq!(p1.memory_region_id(), 0);

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
        let hw = SystemHardware::fallback();
        let processor_set = hw.processors().take(nz!(1)).unwrap();

        processor_set.pin_current_thread_to();

        // Verify that the tracker now reports pinning.
        assert!(hw.is_thread_processor_pinned());
        assert!(hw.is_thread_memory_region_pinned());
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
    fn spawn_thread_pins_correctly() {
        let hw = SystemHardware::fallback();
        let processor_set = hw.processors().take(nz!(1)).unwrap();

        let (tx, rx) = mpsc::channel();

        processor_set
            .spawn_thread(move |_| {
                let is_pinned = hw.is_thread_processor_pinned();
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
        let hw = SystemHardware::fallback();
        let processor_set = hw.processors().take(nz!(2));

        // Skip if we do not have at least 2 processors.
        let Some(processor_set) = processor_set else {
            return;
        };

        let threads = processor_set.spawn_threads(move |_| hw.is_thread_processor_pinned());

        for thread in threads {
            let is_pinned = thread.join().unwrap();
            assert!(is_pinned);
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
    fn inherit_on_pinned() {
        thread::spawn(|| {
            let hw = SystemHardware::fallback();
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
        let hw = SystemHardware::fallback();

        let set = hw.processors().to_builder().take(nz!(1)).unwrap();

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
        let hw = SystemHardware::fallback();
        let all_set = hw.processors();

        let expected_count = thread::available_parallelism()
            .map(std::num::NonZero::get)
            .unwrap_or(1);

        assert_eq!(all_set.len(), expected_count);
    }
}

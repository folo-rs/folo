use std::{cell::RefCell, marker::PhantomData};

use negative_impl::negative_impl;

use crate::{
    MemoryRegionId, Processor, ProcessorId,
    pal::{AbstractProcessor, Platform, PlatformFacade},
};

thread_local! {
    /// Thread-local default instance of the hardware tracker core.
    ///
    /// Each thread has its own hardware tracker, as some of the information will be thread-
    /// specific. This also helps avoid the need for synchronization when accessing the tracker.
    pub(crate) static CURRENT_TRACKER: RefCell<HardwareTrackerCore>
        = RefCell::new(HardwareTrackerCore::new(PlatformFacade::real()));
}

/// Tracks and provides access to changing hardware information over time.
///
/// # Example
///
/// ```
/// use many_cpus::HardwareTracker;
///
/// HardwareTracker::with_current_processor(|p| {
///     let efficiency_class = p.efficiency_class();
///     let id = p.id();
///
///     println!("Executing on processor {id}, which has the efficiency class {efficiency_class:?}");
/// });
/// ```
///
/// # Current processor
///
/// Many of the tracked parameters are related to the current processor. The meaning of "current"
/// is simple: the current processor is whatever processor is executing the `HardwareTracker` code
/// when it is called.
///
/// Unless otherwise configured, threads can move between processors, so the current processor can
/// change over time. This means that there is a certain "time of check to time of use" discrepancy
/// that can occur. This is unavoidable if your threads are not pinned to specific processors.
///
/// If you need certainty in the data, you must use pinned threads, with each thread assigned to
/// execute on either a specific processor or set of processors that have the desired quality you
/// care about (e.g. a number of processors in the same memory region). You can pin the current
/// thread to one or more processors via
/// [ProcessorSet::pin_current_thread_to][crate::ProcessorSet::pin_current_thread_to].
#[derive(Debug)]
pub struct HardwareTracker {
    _no_ctor: PhantomData<()>,
}

impl HardwareTracker {
    /// Obtains a reference to the current processor for the duration of a callback.
    ///
    /// If all you need is the processor ID or memory region ID, you may see better performance
    /// if you query [`current_processor_id()`][1] or [`current_memory_region_id()`][2] directly.
    ///
    /// [1]: HardwareTracker::current_processor_id
    /// [2]: HardwareTracker::current_memory_region_id
    ///
    /// # Example
    ///
    /// ```
    /// use many_cpus::HardwareTracker;
    ///
    /// HardwareTracker::with_current_processor(|p| {
    ///     let efficiency_class = p.efficiency_class();
    ///     let id = p.id();
    ///
    ///     println!("Executing on processor {id}, which has the efficiency class {efficiency_class:?}");
    /// });
    /// ```
    #[cfg_attr(test, mutants::skip)] // Trivial layer, only the core is tested.
    #[inline]
    pub fn with_current_processor<F, R>(f: F) -> R
    where
        F: FnOnce(&Processor) -> R,
    {
        CURRENT_TRACKER.with_borrow(|core| f(core.current_processor()))
    }

    /// The ID of the processor currently executing this thread.
    #[cfg_attr(test, mutants::skip)] // Trivial layer, only the core is tested.
    #[inline]
    pub fn current_processor_id() -> ProcessorId {
        CURRENT_TRACKER.with_borrow(|core| core.current_processor_id())
    }

    /// The memory region ID of the processor currently executing this thread.
    #[cfg_attr(test, mutants::skip)] // Trivial layer, only the core is tested.
    #[inline]
    pub fn current_memory_region_id() -> MemoryRegionId {
        CURRENT_TRACKER.with_borrow(|core| core.current_memory_region_id())
    }

    /// Whether the current thread is pinned to a single processor.
    ///
    /// Threads may be pinned to any number of processors, not just one - this only checks for
    /// the scenario where the thread is pinned to one specific processor.
    ///
    /// This function only recognizes pinning done via the `many_cpus` crate. If you pin a thread
    /// using other means (e.g. `libc`), this function will not be able to detect it.
    ///
    /// # Example (basic)
    ///
    /// ```
    /// use many_cpus::HardwareTracker;
    ///
    /// // Threads are typically not pinned unless you pin them yourself.
    /// assert!(!HardwareTracker::is_thread_processor_pinned());
    /// ```
    ///
    /// # Example (pinned to one processor)
    ///
    /// ```
    /// use many_cpus::{HardwareTracker, ProcessorSet};
    /// use std::num::NonZero;
    /// use std::thread;
    ///
    /// let one_processor = ProcessorSet::builder()
    ///     .take(NonZero::new(1).unwrap())
    ///     .unwrap();
    ///
    /// thread::spawn(move || {
    ///     one_processor.pin_current_thread_to();
    ///
    ///     assert!(HardwareTracker::is_thread_processor_pinned());
    /// }).join().unwrap();
    /// ```
    ///
    /// # Example (pinned to multiple processors)
    ///
    /// ```
    /// use many_cpus::{HardwareTracker, ProcessorSet};
    /// use std::num::NonZero;
    /// use std::thread;
    ///
    /// let two_processors = ProcessorSet::builder()
    ///     .take(NonZero::new(2).unwrap());
    ///
    /// let Some(two_processors) = two_processors else {
    ///     eprintln!("This example requires at least two processors");
    ///     return;
    /// };
    ///
    /// thread::spawn(move || {
    ///     two_processors.pin_current_thread_to();
    ///
    ///     assert!(!HardwareTracker::is_thread_processor_pinned());
    /// }).join().unwrap();
    /// ```
    #[cfg_attr(test, mutants::skip)] // Trivial layer, only the core is tested.
    #[inline]
    pub fn is_thread_processor_pinned() -> bool {
        CURRENT_TRACKER.with_borrow(|core| core.is_thread_processor_pinned())
    }

    /// Whether the current thread is pinned to one or more processors that are all
    /// in the same memory region.
    ///
    /// This function only recognizes pinning done via the `many_cpus` crate. If you pin a thread
    /// using other means (e.g. `libc`), this function will not be able to detect it.
    ///
    /// # Example (basic)
    ///
    /// ```
    /// use many_cpus::HardwareTracker;
    ///
    /// // Threads are typically not pinned unless you pin them yourself.
    /// assert!(!HardwareTracker::is_thread_memory_region_pinned());
    /// ```
    ///
    /// # Example (pinned to one processor)
    ///
    /// ```
    /// use many_cpus::{HardwareTracker, ProcessorSet};
    /// use std::num::NonZero;
    /// use std::thread;
    ///
    /// let one_processor = ProcessorSet::builder()
    ///     .take(NonZero::new(1).unwrap())
    ///     .unwrap();
    ///
    /// thread::spawn(move || {
    ///     one_processor.pin_current_thread_to();
    ///
    ///     // Each processor is in exactly one memory region, so as we are
    ///     // pinned to one processor, we are also pinned to one memory region.
    ///     assert!(HardwareTracker::is_thread_memory_region_pinned());
    /// }).join().unwrap();
    /// ```
    ///
    /// # Example (pinned to multiple processors)
    ///
    /// ```
    /// use many_cpus::{HardwareTracker, ProcessorSet};
    /// use std::num::NonZero;
    /// use std::thread;
    ///
    /// let two_processors = ProcessorSet::builder()
    ///     .same_memory_region()
    ///     .take(NonZero::new(2).unwrap());
    ///
    /// let Some(two_processors) = two_processors else {
    ///     eprintln!("This example requires at least two processors in the same memory region");
    ///     return;
    /// };
    ///
    /// thread::spawn(move || {
    ///     two_processors.pin_current_thread_to();
    ///
    ///     // While we are not pinned to a single processor, all processors we are pinned to
    ///     // are in the same memory region, so we are still pinned to one memory region.
    ///     assert!(HardwareTracker::is_thread_memory_region_pinned());
    /// }).join().unwrap();
    /// ```
    #[cfg_attr(test, mutants::skip)] // Trivial layer, only the core is tested.
    #[inline]
    pub fn is_thread_memory_region_pinned() -> bool {
        CURRENT_TRACKER.with_borrow(|core| core.is_thread_memory_region_pinned())
    }
}

/// The real implementation of HardwareTracker, accepting the PAL facade as a parameter
/// to enable mocking for testing purposes. Public API uses a singleton of this per thread.
#[derive(Debug)]
pub(crate) struct HardwareTrackerCore {
    pinned_processor_id: Option<ProcessorId>,
    pinned_memory_region_id: Option<MemoryRegionId>,

    // Processors indexed by the processor ID.
    // In theory, there can be gaps in the sequence of processors, which is why these are Option.
    // The PAL guarantees that no processors will appear after the end of this list, though.
    all_processors: Box<[Option<Processor>]>,

    pal: PlatformFacade,
}

impl HardwareTrackerCore {
    pub(crate) fn new(pal: PlatformFacade) -> Self {
        let all_pal_processors = pal.get_all_processors();
        let max_processor_id = pal.max_processor_id();

        let mut all_processors = vec![None; max_processor_id as usize + 1];

        for processor in all_pal_processors {
            *all_processors
                .get_mut(processor.id() as usize)
                .expect("encountered processor with ID above max_processor_id") =
                Some(Processor::new(processor, pal.clone()));
        }

        Self {
            pal,
            all_processors: all_processors.into_boxed_slice(),
            pinned_processor_id: None,
            pinned_memory_region_id: None,
        }
    }

    pub(crate) fn current_processor(&self) -> &Processor {
        let processor_id = self.current_processor_id();

        // This should never go OOB unless the PAL lied to us.
        // Still, we do the bounds check just to be good citizens.
        let processor = self.all_processors.get(processor_id as usize);

        if let Some(Some(processor)) = processor {
            processor
        } else {
            // The current processor is one we do not know! Aaaah, something must have changed
            // at runtime! We do not currently plan to support dynamic hardware changes, so we
            // do not strictly care - just return an arbitrary processor and live with the shame.
            self.all_processors
                .iter()
                .find_map(|p| p.as_ref())
                .expect("the system must have at least one processor for code to execute")
        }
    }

    pub(crate) fn current_processor_id(&self) -> ProcessorId {
        self.pinned_processor_id
            .unwrap_or_else(|| self.pal.current_processor_id())
    }

    pub(crate) fn current_memory_region_id(&self) -> MemoryRegionId {
        self.pinned_memory_region_id
            .unwrap_or_else(|| self.current_processor().memory_region_id())
    }

    pub(crate) fn is_thread_processor_pinned(&self) -> bool {
        self.pinned_processor_id.is_some()
    }

    pub(crate) fn is_thread_memory_region_pinned(&self) -> bool {
        self.pinned_memory_region_id.is_some()
    }

    pub(crate) fn update_pin_status(
        &mut self,
        processor_id: Option<ProcessorId>,
        memory_region_id: Option<MemoryRegionId>,
    ) {
        assert!(
            !(memory_region_id.is_none() && processor_id.is_some()),
            "if processor is pinned, memory region is obviously also pinned"
        );

        self.pinned_processor_id = processor_id;
        self.pinned_memory_region_id = memory_region_id;
    }
}

#[negative_impl]
impl !Send for HardwareTracker {}
#[negative_impl]
impl !Sync for HardwareTracker {}

#[cfg(test)]
mod tests {
    use itertools::Itertools;
    use mockall::Sequence;
    use nonempty::nonempty;
    use static_assertions::assert_not_impl_any;

    use crate::{
        EfficiencyClass, ProcessorSet,
        pal::{FakeProcessor, MockPlatform, ProcessorFacade},
    };

    use super::*;

    // https://github.com/cloudhead/nonempty/issues/68
    extern crate alloc;

    #[test]
    fn single_threaded_type() {
        assert_not_impl_any!(HardwareTracker: Send, Sync);
    }

    #[cfg(not(miri))] // Miri does not support talking to the real platform.
    #[test]
    fn real_does_not_panic() {
        let tracker = HardwareTrackerCore::new(PlatformFacade::real());

        // We do not care what it returns (because we are operating in arbitrary thread which can
        // float among all processors); all we care about is that it does not panic.
        _ = tracker.current_processor();
        _ = tracker.current_memory_region_id();
    }

    #[test]
    fn processor_change_is_represented() {
        let mut platform = MockPlatform::new();

        let pal_processors = nonempty![FakeProcessor::with_index(0), FakeProcessor::with_index(1),];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_max_processor_id()
            .times(1)
            .return_const(1_u32);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        let mut seq = Sequence::new();

        platform
            .expect_current_processor_id()
            .times(1)
            .in_sequence(&mut seq)
            .return_const(0_u32);

        // On the second call, we return a different processor - the thread has moved.
        platform
            .expect_current_processor_id()
            .times(1)
            .in_sequence(&mut seq)
            .return_const(1_u32);

        let tracker = HardwareTrackerCore::new(PlatformFacade::from_mock(platform));

        assert_eq!(0, tracker.current_processor_id());
        assert_eq!(1, tracker.current_processor_id());
    }

    #[test]
    fn platform_ignored_when_pinned() {
        let mut platform = MockPlatform::new();

        let pal_processors = nonempty![FakeProcessor::with_index(0), FakeProcessor::with_index(1),];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_max_processor_id()
            .times(1)
            .return_const(1_u32);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        // We do not expect the tracker to ever ask the platform for the current processor
        // because we directly tell it what processor the current thread has been pinned to.

        let mut tracker = HardwareTrackerCore::new(PlatformFacade::from_mock(platform));

        tracker.update_pin_status(Some(0), Some(0));

        assert_eq!(0, tracker.current_processor_id());
        assert_eq!(0, tracker.current_processor_id());
    }

    #[test]
    fn ghost_processor_does_not_panic() {
        let mut platform = MockPlatform::new();

        // We have processors 0 and 2, leaving a gap at index 1.
        // There is also a ghost processor at index 3.
        let pal_processors = nonempty![FakeProcessor::with_index(0), FakeProcessor::with_index(2),];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_max_processor_id()
            .times(1)
            .return_const(3_u32);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        let mut seq = Sequence::new();

        platform
            .expect_current_processor_id()
            .times(1)
            .in_sequence(&mut seq)
            .return_const(0_u32);

        // On the second call, we return processor 1 - this is a ghost
        // the platform gave us no information about.
        platform
            .expect_current_processor_id()
            .times(1)
            .in_sequence(&mut seq)
            .return_const(1_u32);

        platform
            .expect_current_processor_id()
            .times(1)
            .in_sequence(&mut seq)
            .return_const(2_u32);

        // On the final call, we return processor 3 - this is not only a ghost the platform gave
        // us no information about but also exceeds the known maximum processor ID range.
        platform
            .expect_current_processor_id()
            .times(1)
            .in_sequence(&mut seq)
            .return_const(3_u32);

        let tracker = HardwareTrackerCore::new(PlatformFacade::from_mock(platform));

        let processor = tracker.current_processor();
        assert_eq!(0, processor.id());

        // First ghost. We do not care what it returns, as long as it returns something.
        // Our API contract does not promise to work with dynamic hardware changes, we just
        // want to avoid panics when it does occur.
        _ = tracker.current_processor();

        let processor = tracker.current_processor();
        assert_eq!(2, processor.id());

        // Second ghost. Same treatment - it has to return something, no matter what.
        _ = tracker.current_processor();
    }

    #[test]
    fn pinning_updates_are_respected() {
        let mut platform = MockPlatform::new();

        let pal_processors = nonempty![
            FakeProcessor {
                index: 0,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Performance
            },
            FakeProcessor {
                index: 1,
                memory_region: 1,
                efficiency_class: EfficiencyClass::Performance
            } // Ghost at index 2.
        ];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_max_processor_id()
            .times(1)
            .return_const(2_u32);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        let mut seq = Sequence::new();

        platform
            .expect_current_processor_id()
            .times(2)
            .in_sequence(&mut seq)
            .return_const(0_u32);

        platform
            .expect_current_processor_id()
            .times(3)
            .in_sequence(&mut seq)
            .return_const(1_u32);

        let mut tracker = HardwareTrackerCore::new(PlatformFacade::from_mock(platform));

        assert!(!tracker.is_thread_processor_pinned());
        assert!(!tracker.is_thread_memory_region_pinned());

        // We start in a pinned mode and expect the pinned-to knowledge to be used.
        tracker.update_pin_status(Some(0), Some(0));

        assert!(tracker.is_thread_processor_pinned());
        assert!(tracker.is_thread_memory_region_pinned());

        assert_eq!(0, tracker.current_processor_id());
        assert_eq!(0, tracker.current_memory_region_id());

        // We stay pinned, now to a different processor and memory region.
        tracker.update_pin_status(Some(1), Some(1));

        assert!(tracker.is_thread_processor_pinned());
        assert!(tracker.is_thread_memory_region_pinned());

        assert_eq!(1, tracker.current_processor_id());
        assert_eq!(1, tracker.current_memory_region_id());

        // Even when pinned to a ghost processor, everything is fine.
        tracker.update_pin_status(Some(2), Some(2));

        assert!(tracker.is_thread_processor_pinned());
        assert!(tracker.is_thread_memory_region_pinned());

        assert_eq!(2, tracker.current_processor_id());
        assert_eq!(2, tracker.current_memory_region_id());
        // It must return something (anything), even for ghosts we are pinned to.
        _ = tracker.current_processor();

        // We are now unpinned from the processor but remain pinned to a memory region.
        // We expect requesting the processor to query the platform, but not if we just
        // request the memory region ID.
        tracker.update_pin_status(None, Some(2));

        assert!(!tracker.is_thread_processor_pinned());
        assert!(tracker.is_thread_memory_region_pinned());

        let processor = tracker.current_processor();
        assert_eq!(0, processor.id());
        assert_eq!(2, tracker.current_memory_region_id());

        // Note that this is a second query to the platform.
        assert_eq!(0, tracker.current_processor_id());

        // Now we are completely unpinned.
        tracker.update_pin_status(None, None);

        assert!(!tracker.is_thread_processor_pinned());
        assert!(!tracker.is_thread_memory_region_pinned());

        let processor = tracker.current_processor();
        assert_eq!(1, processor.id());

        // Note that this is a second query to the platform.
        assert_eq!(1, tracker.current_memory_region_id());

        // Note that this is a third query to the platform.
        assert_eq!(1, tracker.current_processor_id());
    }

    // Unpinning from memory region while pinning to a processor is nonsense.
    #[test]
    #[should_panic]
    fn panic_if_pinned_processor_with_unpinned_memory_region() {
        let mut platform = MockPlatform::new();

        let pal_processors = nonempty![FakeProcessor::with_index(0)];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_max_processor_id()
            .times(1)
            .return_const(0_u32);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        let mut tracker = HardwareTrackerCore::new(PlatformFacade::from_mock(platform));

        tracker.update_pin_status(Some(0), None);
    }

    #[test]
    #[cfg(not(miri))] // Miri does not support talking to the real platform.
    fn real_current_processor_id_is_unique_and_matches_expectation() {
        // We spawn a thread on every processor and check that the processor ID is unique.
        let mut processor_ids = ProcessorSet::all()
            .spawn_threads(|processor| {
                let processor_id = processor.id();

                let current_processor_id = HardwareTracker::current_processor_id();
                assert_eq!(processor_id, current_processor_id);

                current_processor_id
            })
            .into_iter()
            .map(|x| x.join().unwrap())
            .collect_vec();

        // We expect the processor IDs to be unique.
        processor_ids.sort();

        let unique_id_count = processor_ids.iter().dedup().count();

        assert_eq!(unique_id_count, processor_ids.len());
    }
}

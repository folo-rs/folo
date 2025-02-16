use std::cell::RefCell;

use crate::{
    pal::{AbstractProcessor, Platform, PlatformFacade},
    MemoryRegionId, Processor, ProcessorId,
};

thread_local! {
    /// Thread-local default instance of the current processor tracker.
    ///
    /// This is what gets updated with new information whenever the current thread is pinned.
    pub static CURRENT_TRACKER: RefCell<CurrentTracker> = RefCell::new(CurrentTracker::new(PlatformFacade::real()));
}

/// Tracks/identifies the current processor and related metadata.
///
/// The meaning of "current" is deceptively simple: the current processor is whatever processor
/// is executing the code at the moment the "get current" logic is called.
///
/// Now, most threads can move between processors, so the current processor can change over time.
/// This means that there is a certain "time of check to time of use" discrepancy that can occur.
/// This is unavoidable if your threads are floating - just live with it.
///
/// To avoid this problem, you have to use pinned threads, assigned to execute on either a specific
/// processor or set of processors that the desired quality you care about (e.g. memory region).
pub struct CurrentTracker {
    pinned_processor_id: Option<ProcessorId>,
    pinned_memory_region_id: Option<MemoryRegionId>,

    // Processors indexed by the processor ID.
    // In theory, there can be gaps in the sequence of processors, which is why these are Option.
    // Processors can appear during execution (even beyond the end of this list), as well, so in
    // theory it is possible that we end up in one of the gaps when looking for a processor.
    all_processors: Vec<Option<Processor>>,

    pal: PlatformFacade,
}

impl CurrentTracker {
    pub(crate) fn new(pal: PlatformFacade) -> Self {
        let all_pal_processors = pal.get_all_processors();
        let max_processor_id = all_pal_processors
            .iter()
            .map(|p| p.id())
            .max()
            .expect("the system must have at least one processor for code to execute");

        let mut all_processors = vec![None; max_processor_id as usize + 1];

        for processor in all_pal_processors {
            all_processors[processor.id() as usize] = Some(Processor::new(processor, pal.clone()));
        }

        Self {
            pal,
            all_processors,
            pinned_processor_id: None,
            pinned_memory_region_id: None,
        }
    }

    pub fn current_processor(&self) -> &Processor {
        let processor_id = self.current_processor_id();

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

    pub fn current_processor_id(&self) -> ProcessorId {
        self.pinned_processor_id
            .unwrap_or_else(|| self.pal.current_processor_id())
    }

    pub fn current_memory_region_id(&self) -> MemoryRegionId {
        self.pinned_memory_region_id
            .unwrap_or_else(|| self.current_processor().memory_region_id())
    }

    /// Notifies the tracker that the current thread has been pinned or unpinned and that,
    /// for `Some` arguments, future requests may be answered with the provided values.
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

#[cfg(test)]
mod tests {
    use mockall::Sequence;
    use nonempty::nonempty;

    use crate::{
        pal::{FakeProcessor, MockPlatform, ProcessorFacade},
        EfficiencyClass,
    };

    use super::*;

    // https://github.com/cloudhead/nonempty/issues/68
    extern crate alloc;

    #[test]
    fn real_does_not_panic() {
        let tracker = CurrentTracker::new(PlatformFacade::real());

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

        let tracker = CurrentTracker::new(PlatformFacade::from_mock(platform));

        assert_eq!(0, tracker.current_processor_id());
        assert_eq!(1, tracker.current_processor_id());
    }

    #[test]
    fn platform_ignored_when_pinned() {
        let mut platform = MockPlatform::new();

        let pal_processors = nonempty![FakeProcessor::with_index(0), FakeProcessor::with_index(1),];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

        platform
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        // We do not expect the tracker to ever ask the platform for the current processor
        // because we directly tell it what processor the current thread has been pinned to.

        let mut tracker = CurrentTracker::new(PlatformFacade::from_mock(platform));

        tracker.update_pin_status(Some(0), Some(0));

        assert_eq!(0, tracker.current_processor_id());
        assert_eq!(0, tracker.current_processor_id());
    }

    #[test]
    fn ghost_processor_does_not_panic() {
        let mut platform = MockPlatform::new();

        // We have processors 0 and 2, leaving a gap at index 1.
        let pal_processors = nonempty![FakeProcessor::with_index(0), FakeProcessor::with_index(2),];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

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

        let tracker = CurrentTracker::new(PlatformFacade::from_mock(platform));

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
            }
        ];

        let pal_processors = pal_processors.map(ProcessorFacade::Fake);

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

        let mut tracker = CurrentTracker::new(PlatformFacade::from_mock(platform));

        // We start in a pinned mode and expect the pinned-to knowledge to be used.
        tracker.update_pin_status(Some(0), Some(0));

        assert_eq!(0, tracker.current_processor_id());
        assert_eq!(0, tracker.current_memory_region_id());

        // We stay pinned, now to a different processor and memory region.
        tracker.update_pin_status(Some(1), Some(1));

        assert_eq!(1, tracker.current_processor_id());
        assert_eq!(1, tracker.current_memory_region_id());

        // Even when pinned to a ghost processor, everything is fine.
        tracker.update_pin_status(Some(100), Some(100));

        assert_eq!(100, tracker.current_processor_id());
        assert_eq!(100, tracker.current_memory_region_id());
        // It must return something (anything), even for ghosts we are pinned to.
        _ = tracker.current_processor();

        // We are now unpinned from the processor but remain pinned to a memory region.
        // We expect requesting the processor to query the platform, but not if we just
        // request the memory region ID.
        tracker.update_pin_status(None, Some(100));

        let processor = tracker.current_processor();
        assert_eq!(0, processor.id());
        assert_eq!(100, tracker.current_memory_region_id());

        // Note that this is a second query to the platform.
        assert_eq!(0, tracker.current_processor_id());

        // Now we are completely unpinned.
        tracker.update_pin_status(None, None);

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
            .expect_get_all_processors_core()
            .return_const(pal_processors);

        let mut tracker = CurrentTracker::new(PlatformFacade::from_mock(platform));

        tracker.update_pin_status(Some(0), None);
    }
}

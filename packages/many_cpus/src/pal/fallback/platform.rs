use std::cell::RefCell;
use std::num::NonZeroUsize;
use std::sync::OnceLock;
use std::thread::ThreadId;

use nonempty::NonEmpty;

use super::ProcessorImpl;
use crate::pal::{AbstractProcessor, Platform, ProcessorFacade};
use crate::{MemoryRegionId, ProcessorId};

thread_local! {
    /// The processor ID assigned to the current thread.
    ///
    /// This is computed from the thread ID on first access and remains stable for the lifetime
    /// of the thread. This simulates a thread being scheduled on a specific processor, even
    /// though we do not actually perform any pinning on unsupported platforms.
    static THREAD_PROCESSOR_ID: RefCell<Option<ProcessorId>> = const { RefCell::new(None) };

    /// The set of processor IDs that the current thread is "pinned" to.
    ///
    /// On unsupported platforms, we do not actually pin threads to processors, but we track
    /// the simulated pinning state to maintain API compatibility.
    static THREAD_PINNED_PROCESSORS: RefCell<Option<NonEmpty<ProcessorId>>> =
        const { RefCell::new(None) };
}

/// Fallback platform implementation for operating systems without native support.
///
/// This implementation provides graceful degradation on unsupported platforms by:
/// - Using `std::thread::available_parallelism()` to determine processor count
/// - Simulating all processors as being in a single memory region (region 0)
/// - Marking all processors are Performance class
/// - Pretending to pin threads without actual OS-level affinity changes
/// - Using stable thread-local processor IDs derived from thread IDs
///
/// This allows code to compile and run on any platform, though without the performance benefits
/// of actual processor pinning and topology awareness.
#[derive(Debug)]
pub(crate) struct BuildTargetPlatform;

static PROCESSOR_COUNT: OnceLock<usize> = OnceLock::new();

/// Singleton instance of `BuildTargetPlatform`, used by public API types
/// to hook up to the correct PAL implementation.
pub(crate) static BUILD_TARGET_PLATFORM: BuildTargetPlatform = BuildTargetPlatform;

impl BuildTargetPlatform {
    pub(crate) const fn new() -> Self {
        Self
    }

    #[expect(clippy::unused_self, reason = "matches Platform trait signature")]
    pub(crate) fn processor_count(&self) -> usize {
        *PROCESSOR_COUNT.get_or_init(|| {
            std::thread::available_parallelism()
                .map(NonZeroUsize::get)
                .unwrap_or(1)
        })
    }

    fn get_processors(&self) -> NonEmpty<ProcessorFacade> {
        let processor_count = self.processor_count();

        let processors = (0..processor_count)
            .map(|id| {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "unrealistic to have more than u32::MAX processors"
                )]
                let id = id as ProcessorId;
                #[cfg(test)]
                {
                    ProcessorFacade::Fallback(ProcessorImpl::new(id))
                }
                #[cfg(not(test))]
                {
                    ProcessorFacade::Target(ProcessorImpl::new(id))
                }
            })
            .collect::<Vec<_>>();

        NonEmpty::from_vec(processors).expect("processor count is at least 1, so this cannot fail")
    }

    #[cfg_attr(test, mutants::skip)] // Some mutations are not testable due to simulated nature of this PAL.
    fn thread_processor_id(thread_id: ThreadId) -> ProcessorId {
        THREAD_PROCESSOR_ID.with_borrow_mut(|cached_id| {
            if let Some(id) = *cached_id {
                return id;
            }

            // Compute a stable processor ID from the thread ID.
            // We use a simple hash to distribute threads across processors.
            let thread_id_hash = {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};

                let mut hasher = DefaultHasher::new();
                thread_id.hash(&mut hasher);
                hasher.finish()
            };

            let processor_count = BUILD_TARGET_PLATFORM.processor_count() as u64;

            #[expect(
                clippy::cast_possible_truncation,
                reason = "result of modulo is guaranteed to be less than processor_count"
            )]
            #[expect(clippy::arithmetic_side_effects, reason = "modulo cannot overflow")]
            let processor_id = (thread_id_hash % processor_count) as ProcessorId;

            *cached_id = Some(processor_id);
            processor_id
        })
    }
}

impl Platform for BuildTargetPlatform {
    fn get_all_processors(&self) -> NonEmpty<ProcessorFacade> {
        self.get_processors()
    }

    fn pin_current_thread_to<P>(&self, processors: &NonEmpty<P>)
    where
        P: AsRef<ProcessorFacade>,
    {
        // We do not actually pin threads on unsupported platforms, but we track the simulated
        // pinning state for API compatibility.
        let processor_ids_vec: Vec<_> = processors.iter().map(|p| p.as_ref().id()).collect();
        let processor_ids = NonEmpty::from_vec(processor_ids_vec)
            .expect("processors is NonEmpty, so this cannot fail");

        THREAD_PINNED_PROCESSORS.with_borrow_mut(|pinned| {
            *pinned = Some(processor_ids);
        });
    }

    #[cfg_attr(test, mutants::skip)] // Some mutations are not testable due to simulated nature of this PAL.
    fn current_processor_id(&self) -> ProcessorId {
        // If the thread is pinned to a single processor, return that processor ID.
        // This matches the behavior of real platforms where pinned threads actually run
        // on the pinned processor.
        THREAD_PINNED_PROCESSORS.with_borrow(|pinned| {
            if let Some(processors) = pinned
                && processors.len() == 1
            {
                return *processors.first();
            }

            // If not pinned or pinned to multiple processors, return the thread's natural ID.
            Self::thread_processor_id(std::thread::current().id())
        })
    }

    #[cfg_attr(test, mutants::skip)] // Some mutations are not testable due to simulated nature of this PAL.
    fn current_thread_processors(&self) -> NonEmpty<ProcessorId> {
        THREAD_PINNED_PROCESSORS.with_borrow(|pinned| {
            pinned.clone().unwrap_or_else(|| {
                // If not pinned, return all processor IDs.
                let ids_vec: Vec<_> = self
                    .get_processors()
                    .iter()
                    .map(AbstractProcessor::id)
                    .collect();
                NonEmpty::from_vec(ids_vec)
                    .expect("processor count is at least 1, so this cannot fail")
            })
        })
    }

    fn max_processor_id(&self) -> ProcessorId {
        // This will never wrap because we are guaranteed at least one processor.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "processor count is guaranteed to fit in u32"
        )]
        let max_id = self.processor_count().wrapping_sub(1) as ProcessorId;
        max_id
    }

    #[cfg_attr(test, mutants::skip)] // Some mutations are not testable due to simulated nature of this PAL.
    fn max_memory_region_id(&self) -> MemoryRegionId {
        // All processors are in memory region 0 on the fallback platform.
        0
    }

    fn max_processor_time(&self) -> f64 {
        // We assume no resource quota restrictions on unsupported platforms.
        #[expect(
            clippy::cast_precision_loss,
            reason = "acceptable precision loss for large processor counts"
        )]
        let max_time = self.processor_count() as f64;
        max_time
    }

    fn active_processor_count(&self) -> usize {
        self.processor_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EfficiencyClass;

    #[test]
    fn has_at_least_one_processor() {
        let platform = BuildTargetPlatform::new();
        assert!(platform.processor_count() >= 1);
        assert!(!platform.get_processors().is_empty());
    }

    #[test]
    fn all_processors_in_region_zero() {
        let platform = BuildTargetPlatform::new();
        for processor in platform.get_processors().iter() {
            assert_eq!(processor.memory_region_id(), 0);
        }
    }

    #[test]
    fn all_processors_are_performance() {
        let platform = BuildTargetPlatform::new();
        for processor in platform.get_processors().iter() {
            assert_eq!(processor.efficiency_class(), EfficiencyClass::Performance);
        }
    }

    #[test]
    fn processor_ids_are_sequential() {
        let platform = BuildTargetPlatform::new();
        for (index, processor) in platform.get_processors().iter().enumerate() {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "test data is small enough to fit"
            )]
            let expected_id = index as ProcessorId;
            assert_eq!(processor.id(), expected_id);
        }
    }

    #[test]
    fn max_processor_id_is_count_minus_one() {
        let platform = BuildTargetPlatform::new();
        let max_id = platform.max_processor_id();
        #[expect(
            clippy::cast_possible_truncation,
            reason = "test data is small enough to fit"
        )]
        let expected_max = (platform.processor_count() - 1) as ProcessorId;
        assert_eq!(max_id, expected_max);
    }

    #[test]
    fn current_processor_id_is_stable_within_thread() {
        let platform = BuildTargetPlatform::new();
        let id1 = platform.current_processor_id();
        let id2 = platform.current_processor_id();
        assert_eq!(id1, id2);
    }

    #[test]
    fn pinning_updates_current_thread_processors() {
        let platform = BuildTargetPlatform::new();

        let processors = platform.get_processors();
        let first_processor = processors.first();
        let single_processor = nonempty::nonempty![first_processor];

        platform.pin_current_thread_to(&single_processor);

        let current = platform.current_thread_processors();
        assert_eq!(current.len(), 1);
        assert_eq!(*current.first(), first_processor.id());
    }

    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "exact comparison is appropriate for this test"
    )]
    fn max_processor_time_equals_processor_count() {
        let platform = BuildTargetPlatform::new();
        #[expect(
            clippy::cast_precision_loss,
            reason = "test comparison with acceptable precision"
        )]
        let expected_time = platform.processor_count() as f64;
        assert_eq!(platform.max_processor_time(), expected_time);
    }
}

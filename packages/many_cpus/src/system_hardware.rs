//! Public handle to a system hardware implementation, supporting both real and fake configurations.
//!
//! This module defines the `SystemHardware` type that wraps either the real hardware or a fake
//! simulated platform (when the `test-util` feature is enabled). All hardware-aware APIs
//! flow from the `SystemHardware` instance, enabling multiple isolated platforms to coexist in
//! parallel tests without interference.

use std::any::type_name;
#[cfg(any(test, feature = "test-util"))]
use std::borrow::Borrow;
use std::sync::{Arc, OnceLock, RwLock};
use std::thread::ThreadId;

use foldhash::HashMap;
use nonempty::NonEmpty;

#[cfg(any(test, feature = "test-util"))]
use crate::fake::FakePlatform;
#[cfg(any(test, feature = "test-util"))]
use crate::fake::HardwareBuilder;
use crate::pal::{AbstractProcessor, Platform as PlatformTrait, PlatformFacade};
use crate::{
    MemoryRegionId, Processor, ProcessorId, ProcessorSet, ProcessorSetBuilder, ResourceQuota,
};

/// The real system hardware singleton, initialized on first access.
static CURRENT_HARDWARE: OnceLock<SystemHardware> = OnceLock::new();

/// Handle to system hardware, providing access to hardware information and tracking.
///
/// A `SystemHardware` can represent either the real hardware (via [`SystemHardware::current()`])
/// or simulated fake hardware for testing (via `SystemHardware::fake()` when `test-util` is
/// enabled).
///
/// # Example
///
/// ```
/// use many_cpus::SystemHardware;
///
/// let hardware = SystemHardware::current();
///
/// // Access hardware information.
/// let max_processors = hardware.max_processor_count();
/// println!("Maximum {max_processors} processors supported");
///
/// // Access thread-specific tracking.
/// let processor_id = hardware.current_processor_id();
/// println!("This thread is currently executing on processor {processor_id}");
///
/// // Build processor sets.
/// let processors = hardware.processors();
/// println!("Available processors: {}", processors.len());
/// ```
#[derive(Clone)]
pub struct SystemHardware {
    inner: Arc<SystemHardwareInner>,
}

/// Internal state of a `SystemHardware` instance.
struct SystemHardwareInner {
    /// The platform abstraction layer implementation.
    platform: PlatformFacade,

    /// Per-thread state tracked for this instance.
    ///
    /// We use `RwLock` because reads are far more common than writes, and we expect minimal
    /// contention since each thread typically only accesses its own entry.
    thread_states: RwLock<HashMap<ThreadId, ThreadState>>,

    /// Processors indexed by processor ID for O(1) lookup.
    ///
    /// Gaps in the processor ID sequence are represented as `None`.
    all_processors_slice: Box<[Option<Processor>]>,

    /// Maximum processor ID reported by the backend.
    max_processor_id: ProcessorId,

    /// Maximum memory region ID reported by the backend.
    max_memory_region_id: MemoryRegionId,

    /// Cached default processor list (respects resource quotas).
    ///
    /// We cache the processor list rather than a full `ProcessorSet` to avoid a reference cycle.
    /// `ProcessorSet` contains a `SystemHardware` which would create a circular reference back
    /// to `SystemHardwareInner` via its `Arc`.
    cached_processors: OnceLock<NonEmpty<Processor>>,

    /// Cached all processors list (ignores resource quotas).
    ///
    /// Same caching strategy as `cached_processors` to avoid reference cycles.
    cached_all_processors: OnceLock<NonEmpty<Processor>>,
}

/// Per-thread state tracked within a hardware instance.
#[derive(Clone, Default)]
struct ThreadState {
    /// The processor ID the thread is pinned to, if pinned to exactly one processor.
    pinned_processor_id: Option<ProcessorId>,

    /// The memory region ID the thread is pinned to, if all pinned processors share a region.
    pinned_memory_region_id: Option<MemoryRegionId>,
}

impl SystemHardware {
    /// Returns a handle to the current real system hardware.
    ///
    /// This singleton represents the actual hardware the process is running on. The instance
    /// is initialized on first access and reused thereafter. All clones are equivalent.
    ///
    /// # Example
    ///
    /// ```
    /// use many_cpus::SystemHardware;
    ///
    /// let hardware = SystemHardware::current();
    /// let processor_count = hardware.max_processor_count();
    /// println!("Running on hardware with up to {processor_count} processors");
    /// ```
    #[must_use]
    pub fn current() -> &'static Self {
        CURRENT_HARDWARE.get_or_init(|| Self::from_platform(PlatformFacade::target()))
    }

    /// Creates fake system hardware for testing purposes.
    ///
    /// This method is only available when the `test-util` feature is enabled. Fake hardware
    /// allows testing code under simulated hardware configurations without requiring the
    /// actual hardware.
    ///
    /// Each fake hardware instance maintains its own state, enabling multiple
    /// fake instances to coexist in parallel tests without interference. Clones
    /// are equivalent and represent the same fake hardware.
    ///
    /// # Example
    ///
    /// ```
    /// use many_cpus::SystemHardware;
    /// use many_cpus::fake::HardwareBuilder;
    /// use new_zealand::nz;
    ///
    /// // Create fake hardware with 4 processors in 2 memory regions.
    /// let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(4), nz!(2)));
    ///
    /// // Use the hardware just like the real one.
    /// assert_eq!(hardware.max_processor_count(), 4);
    /// assert_eq!(hardware.max_memory_region_count(), 2);
    /// ```
    #[cfg(any(test, feature = "test-util"))]
    #[must_use]
    pub fn fake(builder: impl Borrow<HardwareBuilder>) -> Self {
        let backend = FakePlatform::from_builder(builder.borrow());
        Self::from_platform(PlatformFacade::from_fake(backend))
    }

    /// Creates a `SystemHardware` instance using the fallback platform.
    ///
    /// The fallback platform is a simple implementation that does not use platform-specific
    /// APIs and is useful for testing cross-platform behavior.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn fallback() -> Self {
        use crate::pal::fallback::BUILD_TARGET_PLATFORM;

        Self::from_platform(PlatformFacade::Fallback(&BUILD_TARGET_PLATFORM))
    }

    fn from_platform(platform: PlatformFacade) -> Self {
        let all_pal_processors = platform.get_all_processors();
        let max_processor_id = platform.max_processor_id();
        let max_memory_region_id = platform.max_memory_region_id();

        let max_processor_count = (max_processor_id as usize)
            .checked_add(1)
            .expect("unrealistic to have more than an usize worth of processors");
        let mut all_processors = vec![None; max_processor_count];

        for processor in all_pal_processors {
            *all_processors
                .get_mut(processor.id() as usize)
                .expect("encountered processor with ID above max_processor_id") =
                Some(Processor::new(processor));
        }

        Self {
            inner: Arc::new(SystemHardwareInner {
                platform,
                thread_states: RwLock::new(HashMap::default()),
                all_processors_slice: all_processors.into_boxed_slice(),
                max_processor_id,
                max_memory_region_id,
                cached_processors: OnceLock::new(),
                cached_all_processors: OnceLock::new(),
            }),
        }
    }

    /// Returns a processor set containing the default set of processors for the current process.
    ///
    /// The default set potentially includes all processors available to the process but respects
    /// the resource quota imposed by the operating system. This is the set of processors you
    /// should typically use for work scheduling to avoid being penalized by the operating system.
    ///
    /// To customize the processor selection, call [`to_builder()`][ProcessorSet::to_builder]
    /// on the returned set.
    ///
    /// # Example
    ///
    /// ```
    /// use many_cpus::SystemHardware;
    ///
    /// let hardware = SystemHardware::current();
    ///
    /// // Get the default set of processors.
    /// let processors = hardware.processors();
    /// println!("Found {} processors", processors.len());
    ///
    /// // To customize the selection, use to_builder():
    /// let performance_only = processors
    ///     .to_builder()
    ///     .performance_processors_only()
    ///     .take_all();
    /// ```
    #[must_use]
    pub fn processors(&self) -> ProcessorSet {
        let cached = self.inner.cached_processors.get_or_init(|| {
            ProcessorSetBuilder::with_internals(self.clone())
                .enforce_resource_quota()
                .take_all()
                .expect(
                    "there is always at least one processor available because we are running on it",
                )
                .into_processors()
        });

        ProcessorSet::new(cached.clone(), self.clone())
    }

    /// Returns the set of processors that the current thread is pinned to, or `None` if the
    /// thread is not pinned.
    ///
    /// This function only recognizes pinning done via the `many_cpus` package. If you pin a thread
    /// using other means (e.g. `libc`), this function will not be able to detect it.
    ///
    /// # Example
    ///
    /// ```
    /// use many_cpus::SystemHardware;
    ///
    /// let hardware = SystemHardware::current();
    ///
    /// // Check if the current thread is pinned.
    /// if let Some(pinned_processors) = hardware.thread_processors() {
    ///     println!("Thread is pinned to {} processors", pinned_processors.len());
    /// } else {
    ///     println!("Thread is not pinned");
    /// }
    /// ```
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Multiple branches, tested via ProcessorSet::pin_current_thread_to.
    pub fn thread_processors(&self) -> Option<ProcessorSet> {
        // Check if thread is pinned to a single processor.
        if let Some(processor_id) = self.get_pinned_processor_id() {
            let processor = self.get_processor(processor_id).clone();

            return Some(ProcessorSet::new(
                NonEmpty::singleton(processor),
                self.clone(),
            ));
        }

        // Check if thread is pinned to a memory region (but not a single processor).
        if let Some(memory_region_id) = self.get_pinned_memory_region_id() {
            // Get all processors in that memory region.
            let processors: Vec<Processor> = self
                .inner
                .all_processors_slice
                .iter()
                .filter_map(|p| p.as_ref())
                .filter(|p| p.memory_region_id() == memory_region_id)
                .cloned()
                .collect();

            if let Some(processors) = NonEmpty::from_vec(processors) {
                return Some(ProcessorSet::new(processors, self.clone()));
            }
        }

        // Thread is not pinned via many_cpus.
        None
    }

    /// Returns a processor set containing all processors known to the system.
    ///
    /// Unlike [`processors()`][Self::processors], this method ignores resource quotas and
    /// returns all processors regardless of operating system restrictions. This is useful
    /// for system introspection but typically should not be used for work scheduling.
    ///
    /// # Example
    ///
    /// ```
    /// use many_cpus::SystemHardware;
    ///
    /// let hardware = SystemHardware::current();
    ///
    /// // Get all processors, ignoring quotas.
    /// let all = hardware.all_processors();
    /// println!("System has {} processors total", all.len());
    ///
    /// // Compare with the default set (respects quotas).
    /// let default = hardware.processors();
    /// if all.len() > default.len() {
    ///     println!(
    ///         "Resource quota limits usage to {} processors",
    ///         default.len()
    ///     );
    /// }
    /// ```
    #[must_use]
    pub fn all_processors(&self) -> ProcessorSet {
        let cached = self.inner.cached_all_processors.get_or_init(|| {
            ProcessorSetBuilder::with_internals(self.clone())
                .take_all()
                .expect("there is always at least one processor available")
                .into_processors()
        });

        ProcessorSet::new(cached.clone(), self.clone())
    }

    /// Returns the highest possible value for a processor ID.
    ///
    /// Processor IDs are in the range `0..=max_processor_id()`.
    ///
    /// # Example
    ///
    /// ```
    /// use many_cpus::SystemHardware;
    ///
    /// let hardware = SystemHardware::current();
    /// let max_id = hardware.max_processor_id();
    /// println!("Processor IDs range from 0 to {max_id}");
    /// ```
    #[must_use]
    #[inline]
    pub fn max_processor_id(&self) -> ProcessorId {
        self.inner.max_processor_id
    }

    /// Returns the highest possible value for a memory region ID.
    ///
    /// Memory region IDs are in the range `0..=max_memory_region_id()`.
    ///
    /// # Example
    ///
    /// ```
    /// use many_cpus::SystemHardware;
    ///
    /// let hardware = SystemHardware::current();
    /// let max_id = hardware.max_memory_region_id();
    /// println!("Memory region IDs range from 0 to {max_id}");
    /// ```
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // Trivial field accessor, tested via max_memory_region_count.
    pub fn max_memory_region_id(&self) -> MemoryRegionId {
        self.inner.max_memory_region_id
    }

    /// Returns the maximum number of processors the system could have.
    ///
    /// This is calculated as `max_processor_id() + 1`. Note that not all processor IDs
    /// in the range may correspond to actual processors.
    ///
    /// # Example
    ///
    /// ```
    /// use many_cpus::SystemHardware;
    ///
    /// let hardware = SystemHardware::current();
    /// let count = hardware.max_processor_count();
    /// println!("System can have up to {count} processors");
    /// ```
    #[must_use]
    #[inline]
    pub fn max_processor_count(&self) -> usize {
        (self.inner.max_processor_id as usize)
            .checked_add(1)
            .expect("unrealistic to have more than an usize worth of processors")
    }

    /// Returns the maximum number of memory regions the system could have.
    ///
    /// This is calculated as `max_memory_region_id() + 1`. Note that not all memory region IDs
    /// in the range may correspond to actual memory regions.
    ///
    /// # Example
    ///
    /// ```
    /// use many_cpus::SystemHardware;
    ///
    /// let hardware = SystemHardware::current();
    /// let count = hardware.max_memory_region_count();
    /// println!("System can have up to {count} memory regions");
    /// ```
    #[must_use]
    #[inline]
    pub fn max_memory_region_count(&self) -> usize {
        (self.inner.max_memory_region_id as usize)
            .checked_add(1)
            .expect("unrealistic to have more than an usize worth of memory regions")
    }

    /// Obtains a reference to the current processor, for the duration of a callback.
    ///
    /// If all you need is the processor ID or memory region ID, you may see better performance
    /// by querying [`current_processor_id()`][Self::current_processor_id] or
    /// [`current_memory_region_id()`][Self::current_memory_region_id] directly.
    ///
    /// # Example
    ///
    /// ```
    /// use many_cpus::SystemHardware;
    ///
    /// let hardware = SystemHardware::current();
    /// hardware.with_current_processor(|processor| {
    ///     println!(
    ///         "Executing on processor {} (class: {:?})",
    ///         processor.id(),
    ///         processor.efficiency_class()
    ///     );
    /// });
    /// ```
    pub fn with_current_processor<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Processor) -> R,
    {
        let processor_id = self.current_processor_id();
        let processor = self.get_processor(processor_id);
        f(processor)
    }

    /// The ID of the processor currently executing this thread.
    ///
    /// # Example
    ///
    /// ```
    /// use many_cpus::SystemHardware;
    ///
    /// let hardware = SystemHardware::current();
    /// let id = hardware.current_processor_id();
    /// println!("Currently executing on processor {id}");
    /// ```
    #[must_use]
    #[inline]
    pub fn current_processor_id(&self) -> ProcessorId {
        self.get_pinned_processor_id()
            .unwrap_or_else(|| self.inner.platform.current_processor_id())
    }

    /// The memory region ID of the processor currently executing this thread.
    ///
    /// # Example
    ///
    /// ```
    /// use many_cpus::SystemHardware;
    ///
    /// let hardware = SystemHardware::current();
    /// let region = hardware.current_memory_region_id();
    /// println!("Currently in memory region {region}");
    /// ```
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // Composition of tested methods.
    pub fn current_memory_region_id(&self) -> MemoryRegionId {
        self.get_pinned_memory_region_id().unwrap_or_else(|| {
            let processor_id = self.current_processor_id();
            self.get_processor(processor_id).memory_region_id()
        })
    }

    /// Whether the current thread is pinned to a single processor.
    ///
    /// Threads may be pinned to any number of processors, not just one - this only checks for
    /// the scenario where the thread is pinned to one specific processor.
    ///
    /// This function only recognizes pinning done via the `many_cpus` package. If you pin a thread
    /// using other means (e.g. `libc`), this function will not be able to detect it.
    ///
    /// # Example
    ///
    /// ```
    /// use many_cpus::SystemHardware;
    ///
    /// let hardware = SystemHardware::current();
    ///
    /// // Threads are typically not pinned unless you pin them yourself.
    /// assert!(!hardware.is_thread_processor_pinned());
    /// ```
    #[must_use]
    #[inline]
    pub fn is_thread_processor_pinned(&self) -> bool {
        self.get_pinned_processor_id().is_some()
    }

    /// Whether the current thread is pinned to one or more processors that are all
    /// in the same memory region.
    ///
    /// This function only recognizes pinning done via the `many_cpus` package. If you pin a thread
    /// using other means (e.g. `libc`), this function will not be able to detect it.
    ///
    /// # Example
    ///
    /// ```
    /// use many_cpus::SystemHardware;
    ///
    /// let hardware = SystemHardware::current();
    ///
    /// // Threads are typically not pinned unless you pin them yourself.
    /// assert!(!hardware.is_thread_memory_region_pinned());
    /// ```
    #[must_use]
    #[inline]
    pub fn is_thread_memory_region_pinned(&self) -> bool {
        self.get_pinned_memory_region_id().is_some()
    }

    /// Returns the resource quota imposed on this process by the operating system.
    ///
    /// # Example
    ///
    /// ```
    /// use many_cpus::SystemHardware;
    ///
    /// let hardware = SystemHardware::current();
    /// let quota = hardware.resource_quota();
    /// println!("Max processor time: {:.2}", quota.max_processor_time());
    /// ```
    #[must_use]
    #[inline]
    pub fn resource_quota(&self) -> ResourceQuota {
        let max_processor_time = self.inner.platform.max_processor_time();
        ResourceQuota::new(max_processor_time)
    }

    /// Returns the number of currently active processors on the system.
    ///
    /// This includes processors that are not available to the current process and
    /// not part of any processor set. This is rarely what you want and is typically only
    /// relevant when interacting with operating system APIs that specifically work with
    /// machine-level processor counts.
    ///
    /// # Example
    ///
    /// ```
    /// use many_cpus::SystemHardware;
    ///
    /// let hardware = SystemHardware::current();
    /// let count = hardware.active_processor_count();
    /// println!("System has {count} active processors");
    /// ```
    #[must_use]
    #[inline]
    pub fn active_processor_count(&self) -> usize {
        self.inner.platform.active_processor_count()
    }

    /// Updates the pinning status for the current thread.
    ///
    /// This is called internally when a thread is pinned to a processor set.
    ///
    /// # Panics
    ///
    /// Panics if `processor_id` is `Some` but `memory_region_id` is `None`, as this is an
    /// invalid state (pinning to a processor implies pinning to its memory region).
    pub(crate) fn update_pin_status(
        &self,
        processor_id: Option<ProcessorId>,
        memory_region_id: Option<MemoryRegionId>,
    ) {
        assert!(
            !(memory_region_id.is_none() && processor_id.is_some()),
            "if processor is pinned, memory region is obviously also pinned"
        );

        let thread_id = std::thread::current().id();
        let mut states = self
            .inner
            .thread_states
            .write()
            .expect("thread state lock should never be poisoned");

        let state = states.entry(thread_id).or_default();
        state.pinned_processor_id = processor_id;
        state.pinned_memory_region_id = memory_region_id;
    }

    /// Gets the pinned processor ID for the current thread, if any.
    fn get_pinned_processor_id(&self) -> Option<ProcessorId> {
        let thread_id = std::thread::current().id();
        let states = self
            .inner
            .thread_states
            .read()
            .expect("thread state lock should never be poisoned");

        states.get(&thread_id)?.pinned_processor_id
    }

    /// Gets the pinned memory region ID for the current thread, if any.
    fn get_pinned_memory_region_id(&self) -> Option<MemoryRegionId> {
        let thread_id = std::thread::current().id();
        let states = self
            .inner
            .thread_states
            .read()
            .expect("thread state lock should never be poisoned");

        states.get(&thread_id)?.pinned_memory_region_id
    }

    /// Gets a processor by ID, with fallback to any available processor if not found.
    fn get_processor(&self, processor_id: ProcessorId) -> &Processor {
        let processor = self.inner.all_processors_slice.get(processor_id as usize);

        if let Some(Some(processor)) = processor {
            processor
        } else {
            // The current processor is one we do not know. This can happen if hardware changed
            // at runtime. Return an arbitrary processor as a fallback.
            self.inner
                .all_processors_slice
                .iter()
                .find_map(|p| p.as_ref())
                .expect("the system must have at least one processor for code to execute")
        }
    }

    /// Returns a reference to the platform abstraction layer.
    pub(crate) fn platform(&self) -> &PlatformFacade {
        &self.inner.platform
    }
}

// We have no API contract for the Debug output format.
#[cfg_attr(coverage_nightly, coverage(off))]
impl std::fmt::Debug for SystemHardware {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let processor_count = self
            .inner
            .all_processors_slice
            .iter()
            .filter(|p| p.is_some())
            .count();

        f.debug_struct(type_name::<Self>())
            .field("processor_count", &processor_count)
            .field("max_processor_id", &self.inner.max_processor_id)
            .field("max_memory_region_id", &self.inner.max_memory_region_id)
            .finish()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use itertools::Itertools;

    use super::*;

    #[test]
    #[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
    fn current_hardware_is_singleton() {
        let h1 = SystemHardware::current();
        let h2 = SystemHardware::current();

        // Both references should point to the same inner data.
        assert!(Arc::ptr_eq(&h1.inner, &h2.inner));
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
    fn returns_valid_processor_id() {
        let hardware = SystemHardware::current();

        let id = hardware.current_processor_id();

        // The processor ID should be within the valid range.
        assert!(id <= hardware.max_processor_id());
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
    fn counts_are_positive_values() {
        let hardware = SystemHardware::current();

        // There must be at least one processor and one memory region.
        assert!(hardware.max_processor_count() >= 1);
        assert!(hardware.max_memory_region_count() >= 1);
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
    fn pin_status_tracking_per_thread() {
        let hardware = SystemHardware::current();

        // Initially not pinned.
        assert!(!hardware.is_thread_processor_pinned());
        assert!(!hardware.is_thread_memory_region_pinned());

        // Update pin status.
        hardware.update_pin_status(Some(0), Some(0));

        // Now pinned.
        assert!(hardware.is_thread_processor_pinned());
        assert!(hardware.is_thread_memory_region_pinned());

        // Clear pin status.
        hardware.update_pin_status(None, None);

        // No longer pinned.
        assert!(!hardware.is_thread_processor_pinned());
        assert!(!hardware.is_thread_memory_region_pinned());
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
    fn processors_returns_set() {
        let hardware = SystemHardware::current();
        let processors = hardware.processors();

        assert!(processors.len() >= 1);
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
    fn pinned_current_processor_id_is_unique() {
        // We spawn a thread on every processor and check that the processor ID is unique.
        let hw = SystemHardware::current();

        let mut processor_ids = hw
            .processors()
            .spawn_threads(|processor| {
                let processor_id = processor.id();

                let current_processor_id = hw.current_processor_id();
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

    #[test]
    #[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
    fn active_processor_count_is_at_least_processor_set_len() {
        // The system may have processors that are not part of any processor set (e.g. because
        // they are not available for the current process) but cannot have more processors in any
        // processor set than the active processor count.
        let hw = SystemHardware::current();
        let all_processors = hw.all_processors();

        let active_processors = hw.active_processor_count();

        // It is OK if all_processors() returns fewer (there are more constraints than "is active").
        assert!(active_processors >= all_processors.len());
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "unavoidable f64-usize casting but we know the value is positive"
    )]
    fn resource_quota_is_followed_by_default() {
        let hw = SystemHardware::current();
        let max_processor_time = hw.resource_quota().max_processor_time();
        let processors = hw.processors();

        // Must be at least 1, since 0 processors is invalid set size.
        let quota_to_processor_time = (max_processor_time.floor() as usize).max(1);

        assert_eq!(quota_to_processor_time, processors.len());
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
    #[should_panic]
    fn panic_if_pinned_processor_with_unpinned_memory_region() {
        let hardware = SystemHardware::current();
        hardware.update_pin_status(Some(0), None);
    }
}

#[cfg(all(test, feature = "test-util"))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests_fake {
    use new_zealand::nz;

    use crate::fake::{HardwareBuilder, ProcessorBuilder};
    use crate::{EfficiencyClass, SystemHardware};

    #[test]
    fn fake_hardware_with_simple_config() {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(4), nz!(1)));

        assert_eq!(hardware.max_processor_count(), 4);
        assert_eq!(hardware.max_memory_region_count(), 1);
    }

    #[test]
    fn fake_hardware_with_memory_regions() {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(6), nz!(3)));

        assert_eq!(hardware.max_processor_count(), 6);
        assert_eq!(hardware.max_memory_region_count(), 3);
    }

    #[test]
    fn fake_hardware_with_custom_processors() {
        let hardware = SystemHardware::fake(
            HardwareBuilder::new()
                .processor(
                    ProcessorBuilder::new()
                        .id(0)
                        .memory_region(0)
                        .efficiency_class(EfficiencyClass::Performance),
                )
                .processor(
                    ProcessorBuilder::new()
                        .id(1)
                        .memory_region(1)
                        .efficiency_class(EfficiencyClass::Efficiency),
                ),
        );

        assert_eq!(hardware.max_processor_count(), 2);
        assert_eq!(hardware.max_memory_region_count(), 2);
    }

    #[test]
    fn fake_hardware_returns_valid_processor_id() {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(8), nz!(1)));

        let id = hardware.current_processor_id();

        // The ID should be within the configured range.
        assert!(id < 8);
    }

    #[test]
    fn fake_hardware_pin_status_tracking() {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(4), nz!(1)));

        // Initially not pinned.
        assert!(!hardware.is_thread_processor_pinned());
        assert!(!hardware.is_thread_memory_region_pinned());

        // Update pin status.
        hardware.update_pin_status(Some(0), Some(0));

        // Now pinned.
        assert!(hardware.is_thread_processor_pinned());
        assert!(hardware.is_thread_memory_region_pinned());

        // Clear pin status.
        hardware.update_pin_status(None, None);

        // No longer pinned.
        assert!(!hardware.is_thread_processor_pinned());
        assert!(!hardware.is_thread_memory_region_pinned());
    }

    #[test]
    fn fake_hardware_with_hybrid_processors() {
        // 4 performance + 2 efficiency processors.
        let hardware = SystemHardware::fake(
            HardwareBuilder::new()
                .processor(ProcessorBuilder::new().efficiency_class(EfficiencyClass::Performance))
                .processor(ProcessorBuilder::new().efficiency_class(EfficiencyClass::Performance))
                .processor(ProcessorBuilder::new().efficiency_class(EfficiencyClass::Performance))
                .processor(ProcessorBuilder::new().efficiency_class(EfficiencyClass::Performance))
                .processor(ProcessorBuilder::new().efficiency_class(EfficiencyClass::Efficiency))
                .processor(ProcessorBuilder::new().efficiency_class(EfficiencyClass::Efficiency)),
        );

        assert_eq!(hardware.max_processor_count(), 6);
        assert_eq!(hardware.active_processor_count(), 6);
    }

    #[test]
    fn fake_hardware_with_resource_quota() {
        let hardware = SystemHardware::fake(
            HardwareBuilder::from_counts(nz!(8), nz!(1)).max_processor_time(2.5),
        );

        let quota = hardware.resource_quota();

        // The quota should reflect our configuration.
        assert!((quota.max_processor_time() - 2.5).abs() < f64::EPSILON);
    }

    #[test]
    fn multiple_fake_hardware_are_isolated() {
        // Create two different fake hardware instances.
        let hardware1 = SystemHardware::fake(HardwareBuilder::from_counts(nz!(4), nz!(1)));
        let hardware2 = SystemHardware::fake(HardwareBuilder::from_counts(nz!(8), nz!(1)));

        // They should have different configurations.
        assert_eq!(hardware1.max_processor_count(), 4);
        assert_eq!(hardware2.max_processor_count(), 8);

        // Pin status on one should not affect the other.
        hardware1.update_pin_status(Some(0), Some(0));

        assert!(hardware1.is_thread_processor_pinned());
        assert!(!hardware2.is_thread_processor_pinned());
    }

    #[test]
    fn fake_hardware_clones_share_state() {
        let hardware1 = SystemHardware::fake(HardwareBuilder::from_counts(nz!(4), nz!(1)));
        let hardware2 = hardware1.clone();

        // Pin status on one should affect the other since they share state.
        hardware1.update_pin_status(Some(0), Some(0));

        assert!(hardware1.is_thread_processor_pinned());
        assert!(hardware2.is_thread_processor_pinned());
    }

    #[test]
    fn fake_hardware_processors_returns_set() {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(4), nz!(1)));
        let processors = hardware.processors();

        assert_eq!(processors.len(), 4);
    }

    #[test]
    fn thread_processors_returns_none_when_not_pinned() {
        // Run in new thread to ensure clean pin state.
        std::thread::spawn(|| {
            let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(4), nz!(1)));

            // Without any pinning, thread_processors() should return None.
            let result = hardware.thread_processors();
            assert!(result.is_none());
        })
        .join()
        .unwrap();
    }

    #[test]
    fn thread_processors_returns_single_processor_when_pinned_to_one() {
        std::thread::spawn(|| {
            let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(4), nz!(1)));

            // Pin to a single processor.
            let single = hardware.processors().take(nz!(1)).unwrap();
            single.pin_current_thread_to();

            // thread_processors() should return exactly that processor.
            let result = hardware.thread_processors().unwrap();
            assert_eq!(result.len(), 1);
            assert_eq!(
                result.processors().first().id(),
                single.processors().first().id()
            );
        })
        .join()
        .unwrap();
    }

    #[test]
    fn thread_processors_returns_all_region_processors_when_pinned_to_same_region() {
        std::thread::spawn(|| {
            // Create 4 processors, all in region 0.
            let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(4), nz!(1)));

            // Pin to 2 processors in the same region.
            let two = hardware.processors().take(nz!(2)).unwrap();
            two.pin_current_thread_to();

            // thread_processors() should return all processors in that memory region.
            let result = hardware.thread_processors().unwrap();

            // With same memory region, we get all processors in that region.
            assert_eq!(result.len(), 4);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn max_processor_id_returns_configured_value() {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(8), nz!(4)));

        // With 8 processors, max ID should be 7 (0-indexed).
        assert_eq!(hardware.max_processor_id(), 7);
    }

    #[test]
    fn max_memory_region_id_returns_configured_value() {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(8), nz!(4)));

        // With 4 memory regions, max ID should be 3 (0-indexed).
        assert_eq!(hardware.max_memory_region_id(), 3);
    }
}

use std::sync::{Arc, OnceLock};

use arc_swap::{ArcSwap, ArcSwapOption, AsRaw};
use many_cpus::MemoryRegionId;

use crate::{
    hw_info_client::{HardwareInfoClient, HardwareInfoClientFacade},
    hw_tracker_client::{HardwareTrackerClient, HardwareTrackerClientFacade},
};

/// Provides access to an instance of `T` that is locally cached in the current memory region.
///
/// Refer to [crate-level documentation][crate] for more information.
#[derive(Debug)]
#[linked::object]
pub struct RegionCached<T>
where
    T: Clone + Send + Sync + 'static,
{
    // If the current thread is pinned to a memory region, we just reference the regional state.
    // Otherwise, we have to look up the regional state from the global state on every access
    // because we do not know what region we are in (it might change for every call).
    // If this is `None`, we are in a mode where we need to perform the lookup every time.
    regional_state: Option<Arc<RegionalState<T>>>,

    global_state: Arc<GlobalState<T>>,

    hardware_tracker: HardwareTrackerClientFacade,
}

impl<T> RegionCached<T>
where
    T: Clone + Send + Sync + 'static,
{
    /// Creates a new instance of `RegionCached` with the given initial value.
    ///
    /// The instance may be cloned and shared between threads following the linked object patterns.
    /// Every instance from the same family of objects will reference the same region-cached value.
    ///
    /// This type is internally used by the [`region_cached!`][1] macro but can also be used
    /// independently of that macro, typically via a [`PerThread`][2] wrapper that automatically
    /// manager the per-thread instance lifecycle and delivery across threads.
    ///
    /// [1]: crate::region_cached
    /// [2]: linked::PerThread
    pub fn new(initial_value: T) -> Self {
        Self::with_clients(
            initial_value,
            HardwareInfoClientFacade::real(),
            HardwareTrackerClientFacade::real(),
        )
    }

    pub(crate) fn with_clients(
        initial_value: T,
        hardware_info: HardwareInfoClientFacade,
        hardware_tracker: HardwareTrackerClientFacade,
    ) -> Self {
        let memory_region_count = hardware_info.max_memory_region_count();

        let global_state = Arc::new(GlobalState::new(initial_value, memory_region_count));

        linked::new!(Self {
            regional_state: Self::try_locate_regional_state(
                &global_state,
                hardware_tracker.clone()
            ),
            global_state: Arc::clone(&global_state),
            hardware_tracker: hardware_tracker.clone(),
        })
    }

    fn try_locate_regional_state(
        global_state: &Arc<GlobalState<T>>,
        hardware_tracker: HardwareTrackerClientFacade,
    ) -> Option<Arc<RegionalState<T>>> {
        if !hardware_tracker.is_thread_memory_region_pinned() {
            return None;
        }

        // This thread is pinned to a specific memory region, so we can directly access the
        // state of that region and skip the regional state lookup on every access.
        let memory_region_id = hardware_tracker.current_memory_region_id();
        Some(global_state.with_regional_state(memory_region_id, Arc::clone))
    }

    /// Executes the provided function with a reference to the cached value
    /// in the current memory region.
    ///
    /// # Example
    ///
    /// ```
    /// use linked::PerThread;
    /// use region_cached::{RegionCached};
    ///
    /// let favorite_color_global = PerThread::new(RegionCached::new("blue".to_string()));
    ///
    /// // This localizes the value for the current thread, accessing data
    /// // in the current thread's active memory region.
    /// let favorite_color = favorite_color_global.local();
    ///
    /// let len = favorite_color.with_cached(|color| color.len());
    /// assert_eq!(len, 4);
    /// ```
    pub fn with_cached<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        // If we are in a fixed memory region, we can just use the value directly.
        if let Some(value) = self.regional_state.as_ref() {
            return self.with_in_region(value, f);
        }

        // Otherwise, we need to identify our memory region look up the region-specific
        // value from the global state. This is the slow path - pin your threads for max happiness.

        // We fix the memory region ID at this point. It may be that the thread migrates to a
        // different memory region during the rest of this function - we do not care about that.
        let memory_region_id = self.hardware_tracker.current_memory_region_id();

        self.global_state
            .with_regional_state(memory_region_id, |regional_state| {
                self.with_in_region(regional_state, f)
            })
    }

    fn with_in_region<F, R>(&self, regional_state: &RegionalState<T>, mut f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        loop {
            // If the read fails, we get our `f` callback returned back to us.
            match regional_state.try_with_value(f) {
                Ok(result) => return result,
                Err(callback) => f = callback,
            }

            loop {
                // If we got here then the regional state is not initialized. Let's initialize it.
                // We now need a value to initialize the region state with. The latest written value
                // (for our weakly consistent definition of "latest") is stored in the global state.
                //
                // We need to protect against concurrent initialization causing torn initialization,
                // where when two threads set values A and B, and some regions end up seeing A and
                // some end up seeing B, depending on how things were ordered between the threads.
                // To be clear, we do not care whether we end up with A or B, but we do care that
                // all regions see the same value.
                //
                // We do this by verifying after our initialization that the initial value is still
                // the same as when we started the initialization. If it is not, we re-initialize
                // to "catch up" to any new changes that have occurred.
                let initial_value = self.global_state.latest_value.load();

                // We use the raw pointer to compare the value before and after.
                let initial_value_raw = initial_value.as_raw();

                regional_state.set(&initial_value);

                let initial_value_after = self.global_state.latest_value.load();
                let initial_value_after_raw = initial_value_after.as_raw();

                if initial_value_raw == initial_value_after_raw {
                    // We are done - the universe did not change during initialization.
                    break;
                }
            }
        }
    }

    /// Publishes a new value to all memory regions.
    ///
    /// The update will be applied to all memory regions in a [weakly consistent manner][1].
    ///
    /// # Example
    ///
    /// ```
    /// use linked::PerThread;
    /// use region_cached::{RegionCached};
    ///
    /// let favorite_color_global = PerThread::new(RegionCached::new("blue".to_string()));
    ///
    /// // This localizes the value for the current thread, accessing data
    /// // in the current thread's active memory region.
    /// let favorite_color = favorite_color_global.local();
    ///
    /// favorite_color.set_global("red".to_string());
    /// ```
    ///
    /// Updating the value is [weakly consistent][1]. Do not expect the update to be
    /// immediately visible. Even on the same thread, it is only guaranteed to be
    /// immediately visible if the thread is pinned to a specific memory region.
    ///
    /// ```
    /// use linked::PerThread;
    /// use many_cpus::ProcessorSet;
    /// use region_cached::{RegionCached};
    /// use std::num::NonZero;
    ///
    /// let favorite_color_global = PerThread::new(RegionCached::new("blue".to_string()));
    ///
    /// // We can use this to pin a thread to a specific processor, to demonstrate a
    /// // situation where you can rely on consistency guarantees for immediate visibility.
    /// let one_processor = ProcessorSet::builder()
    ///     .take(NonZero::new(1).unwrap())
    ///     .unwrap();
    ///
    /// one_processor.spawn_thread(move |processor_set| {
    ///     let processor = processor_set.processors().first();
    ///     println!("Thread pinned to processor {} in memory region {}",
    ///         processor.id(),
    ///         processor.memory_region_id()
    ///     );
    ///
    ///     // This localizes the value for the current thread, accessing data
    ///     // in the current thread's active memory region.
    ///     let favorite_color = favorite_color_global.local();
    ///
    ///     favorite_color.set_global("red".to_string());
    ///
    ///     // This thread is pinned to a specific processor, so it is guaranteed to stay
    ///     // within the same memory region (== on the same physical hardware). This means
    ///     // that an update to a region-cached value is immediately visible.
    ///     let color = favorite_color.with_cached(|color| color.clone());
    ///     assert_eq!(color, "red");
    /// }).join().unwrap();
    /// ```
    ///
    /// [1]: crate#consistency-guarantees
    pub fn set_global(&self, value: T) {
        // The first thing we do is update the latest value in the global state. This ensures that
        // any new regional states that get initialized will get our latest updated value.
        self.global_state
            .latest_value
            .store(Arc::new(value.clone()));

        // Now all we need to do is invalidate the current value in all regions.
        // Each region will reinitialize itself automatically on next access.
        self.global_state.invalidate_regions();
    }
}

impl<T> RegionCached<T>
where
    T: Clone + Copy + Send + Sync + 'static,
{
    /// Gets a copy of the cached value in the current memory region.
    ///
    /// # Example
    ///
    /// ```
    /// use linked::PerThread;
    /// use region_cached::{RegionCached};
    ///
    /// let current_access_token_global = PerThread::new(RegionCached::new(0x123100));
    ///
    /// // This localizes the value for the current thread, accessing data
    /// // in the current thread's active memory region.
    /// let current_access_token = current_access_token_global.local();
    ///
    /// let token = current_access_token.get_cached();
    /// assert_eq!(token, 0x123100);
    /// ```
    pub fn get_cached(&self) -> T {
        self.with_cached(|v| *v)
    }
}

#[derive(Debug)]
struct GlobalState<T>
where
    T: Clone + Send + Sync + 'static,
{
    /// The latest value written into the region-cached variable from any thread. This is only used
    /// to create regional clones for local caching and is never read directly in any hot path.
    latest_value: ArcSwap<T>,

    // We cannot avoid the array itself being cross-region accessed but the RegionalState items
    // inside are at least initialized lazily and on the correct region, so we can ensure that
    // they are allocated in that memory region (assuming the allocator cooperates).
    //
    // Accessing this can be skipped for threads that are pinned in one specific memory region,
    // as they then have direct access to the `Arc<RegionalState>`, which is the fastest path.
    regional_states: Box<[OnceLock<Arc<RegionalState<T>>>]>,
}

impl<T> GlobalState<T>
where
    T: Clone + Send + Sync + 'static,
{
    fn new(initial_value: T, memory_region_count: usize) -> Self {
        let mut regional_states = Vec::with_capacity(memory_region_count);

        for _ in 0..memory_region_count {
            regional_states.push(OnceLock::new());
        }

        let initial_value = ArcSwap::from_pointee(initial_value);

        Self {
            latest_value: initial_value,
            regional_states: regional_states.into_boxed_slice(),
        }
    }

    /// Executes a function on the regional state for the given memory region.
    fn with_regional_state<F, R>(&self, memory_region_id: MemoryRegionId, f: F) -> R
    where
        F: FnOnce(&Arc<RegionalState<T>>) -> R,
    {
        let slot = &self.regional_states[memory_region_id as usize];

        // The entire purpose of that OnceLock is to ensure this Arc::new() happens
        // when the current thread is executing in the correct memory region, to place
        // the regional state of every region in that specific region.
        let regional_state = slot.get_or_init(|| Arc::new(RegionalState::new()));

        f(regional_state)
    }

    fn invalidate_regions(&self) {
        for slot in &self.regional_states {
            // If it is already `None`, it will already get initialized on the next access.
            // It might already be in the process of being initialized by another thread, which
            // is fine - once initialized, it will by default be in the invalidated state.
            if let Some(state) = slot.get() {
                state.clear()
            }
        }
    }
}

#[derive(Debug)]
struct RegionalState<T>
where
    T: Clone + Send + Sync + 'static,
{
    /// The value shared (via `Arc`) between all threads in the same memory region.
    ///
    /// This is `None` if the value for this region has not been initialized yet.
    /// It will be initialized on first access from this region.
    ///
    /// We use ArcSwap here because it offers very good multithreaded read performance.
    /// In single-threaded and write-heavy scenarios, RwLock is faster but those
    /// are not the scenarios we target - we expect the data to be cached for long
    /// periods and read from many threads, with writes happening not so often.
    value: ArcSwapOption<T>,
}

impl<T> RegionalState<T>
where
    T: Clone + Send + Sync + 'static,
{
    fn new() -> Self {
        Self {
            value: ArcSwapOption::const_empty(),
        }
    }

    /// Attempts to execute a function on the value stored in this regional state.
    ///
    /// Returns back the unused function via `Err` if the value in this regional state is not yet
    /// initialized. In such a case, you should call `initialize()` and try again.
    fn try_with_value<F, R>(&self, f: F) -> Result<R, F>
    where
        F: FnOnce(&T) -> R,
    {
        let reader = self.value.load();

        if let Some(ref value) = *reader {
            return Ok(f(value));
        }

        Err(f)
    }

    fn set(&self, value: &T) {
        self.value.store(Some(Arc::new(value.clone())));
    }

    fn clear(&self) {
        self.value.store(None);
    }
}

#[cfg(test)]
mod tests {
    use std::ptr;
    use std::sync::Arc;

    use crate::{
        RegionCachedCopyExt, RegionCachedExt, hw_info_client::MockHardwareInfoClient,
        hw_tracker_client::MockHardwareTrackerClient, region_cached,
    };

    use super::*;

    #[cfg(not(miri))] // Miri does not support talking to the real platform.
    #[test]
    fn real_smoke_test() {
        region_cached! {
            static FAVORITE_COLOR: String = "blue".to_string();
            static FAVORITE_NUMBER: i32 = 42;
        }

        FAVORITE_COLOR.with_cached(|color| {
            assert_eq!(*color, "blue");
        });

        FAVORITE_COLOR.set_global("red".to_string());

        FAVORITE_COLOR.with_cached(|color| {
            assert_eq!(*color, "red");
        });

        assert_eq!(FAVORITE_NUMBER.get_cached(), 42);
    }

    #[cfg(not(miri))] // Miri does not support talking to the real platform.
    #[test]
    fn with_non_const_initial_value() {
        region_cached!(static FAVORITE_COLOR: Arc<String> = Arc::new("blue".to_string()));

        FAVORITE_COLOR.with_cached(|color| {
            assert_eq!(**color, "blue");
        });
    }

    #[test]
    fn different_regions_have_different_clones() {
        let mut hardware_tracker = MockHardwareTrackerClient::new();

        hardware_tracker
            .expect_is_thread_memory_region_pinned()
            .return_const(false);

        hardware_tracker
            .expect_current_memory_region_id()
            .times(1)
            .return_const(0 as MemoryRegionId);
        hardware_tracker
            .expect_current_memory_region_id()
            .times(1)
            .return_const(1 as MemoryRegionId);
        hardware_tracker
            .expect_current_memory_region_id()
            .times(1)
            .return_const(9 as MemoryRegionId);

        let hardware_tracker = HardwareTrackerClientFacade::from_mock(hardware_tracker);

        let mut hardware_info = MockHardwareInfoClient::new();

        hardware_info
            .expect_max_memory_region_count()
            .return_const(10_usize);

        let hardware_info = HardwareInfoClientFacade::from_mock(hardware_info);

        let local =
            RegionCached::with_clients(|| "foo".to_string(), hardware_info, hardware_tracker);

        let value1 = local.with_cached(ptr::from_ref);
        let value2 = local.with_cached(ptr::from_ref);
        let value3 = local.with_cached(ptr::from_ref);

        assert_ne!(value1, value2);
        assert_ne!(value1, value3);
    }

    #[test]
    fn initial_value_propagates_to_all_regions() {
        let mut hardware_tracker = MockHardwareTrackerClient::new();

        hardware_tracker
            .expect_is_thread_memory_region_pinned()
            .return_const(false);

        hardware_tracker
            .expect_current_memory_region_id()
            .times(1)
            .return_const(0 as MemoryRegionId);
        hardware_tracker
            .expect_current_memory_region_id()
            .times(1)
            .return_const(1 as MemoryRegionId);
        hardware_tracker
            .expect_current_memory_region_id()
            .times(1)
            .return_const(9 as MemoryRegionId);

        let hardware_tracker = HardwareTrackerClientFacade::from_mock(hardware_tracker);

        let mut hardware_info = MockHardwareInfoClient::new();

        hardware_info
            .expect_max_memory_region_count()
            .return_const(10_usize);

        let hardware_info = HardwareInfoClientFacade::from_mock(hardware_info);

        let local = RegionCached::with_clients(42, hardware_info, hardware_tracker);

        assert_eq!(local.get_cached(), 42);
        assert_eq!(local.get_cached(), 42);
        assert_eq!(local.get_cached(), 42);
    }

    #[test]
    fn update_propagates_to_all_regions() {
        let mut hardware_tracker = MockHardwareTrackerClient::new();

        hardware_tracker
            .expect_is_thread_memory_region_pinned()
            .return_const(false);

        // Initial read.
        hardware_tracker
            .expect_current_memory_region_id()
            .times(1)
            .return_const(5 as MemoryRegionId);

        // Reads to verify.
        hardware_tracker
            .expect_current_memory_region_id()
            .times(1)
            .return_const(0 as MemoryRegionId);
        hardware_tracker
            .expect_current_memory_region_id()
            .times(1)
            .return_const(1 as MemoryRegionId);
        hardware_tracker
            .expect_current_memory_region_id()
            .times(1)
            .return_const(9 as MemoryRegionId);

        let hardware_tracker = HardwareTrackerClientFacade::from_mock(hardware_tracker);

        let mut hardware_info = MockHardwareInfoClient::new();

        hardware_info
            .expect_max_memory_region_count()
            .return_const(10_usize);

        let hardware_info = HardwareInfoClientFacade::from_mock(hardware_info);

        let local = RegionCached::with_clients(42, hardware_info, hardware_tracker);

        assert_eq!(local.get_cached(), 42);
        local.set_global(43);

        assert_eq!(local.get_cached(), 43);
        assert_eq!(local.get_cached(), 43);
        assert_eq!(local.get_cached(), 43);
    }

    #[test]
    fn immediate_set_propagates_to_all_regions() {
        let mut hardware_tracker = MockHardwareTrackerClient::new();

        hardware_tracker
            .expect_is_thread_memory_region_pinned()
            .return_const(false);

        // Reads to verify.
        hardware_tracker
            .expect_current_memory_region_id()
            .times(1)
            .return_const(0 as MemoryRegionId);
        hardware_tracker
            .expect_current_memory_region_id()
            .times(1)
            .return_const(1 as MemoryRegionId);
        hardware_tracker
            .expect_current_memory_region_id()
            .times(1)
            .return_const(9 as MemoryRegionId);

        let hardware_tracker = HardwareTrackerClientFacade::from_mock(hardware_tracker);

        let mut hardware_info = MockHardwareInfoClient::new();

        hardware_info
            .expect_max_memory_region_count()
            .return_const(10_usize);

        let hardware_info = HardwareInfoClientFacade::from_mock(hardware_info);

        let local = RegionCached::with_clients(42, hardware_info, hardware_tracker);

        local.set_global(43);

        assert_eq!(local.get_cached(), 43);
        assert_eq!(local.get_cached(), 43);
        assert_eq!(local.get_cached(), 43);
    }

    #[test]
    fn pinned_thread_has_direct_access_to_regional_state() {
        let mut hardware_tracker = MockHardwareTrackerClient::new();

        hardware_tracker
            .expect_is_thread_memory_region_pinned()
            .return_const(true);

        // Even though we read 3 times, we only expect this to be called once and then never again
        // because we are a pinned thread and the RegionCached will directly access the state.
        hardware_tracker
            .expect_current_memory_region_id()
            .times(1)
            .return_const(0 as MemoryRegionId);

        let hardware_tracker = HardwareTrackerClientFacade::from_mock(hardware_tracker);

        let mut hardware_info = MockHardwareInfoClient::new();

        hardware_info
            .expect_max_memory_region_count()
            .return_const(10_usize);

        let hardware_info = HardwareInfoClientFacade::from_mock(hardware_info);

        let local = RegionCached::with_clients(42, hardware_info, hardware_tracker);

        assert_eq!(local.get_cached(), 42);
        assert_eq!(local.get_cached(), 42);
        assert_eq!(local.get_cached(), 42);
    }
}

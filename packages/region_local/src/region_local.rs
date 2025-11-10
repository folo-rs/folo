use std::sync::{Arc, OnceLock};

use arc_swap::ArcSwapOption;
use many_cpus::MemoryRegionId;
use rsevents::{Awaitable, EventState, ManualResetEvent};

use crate::{
    HardwareInfoClient, HardwareInfoClientFacade, HardwareTrackerClient,
    HardwareTrackerClientFacade,
};

/// Provides access to an instance of `T` whose values are local to the current memory region.
/// Callers from different memory regions will observe different instances of `T`.
///
/// Refer to [crate-level documentation][crate] for more information.
#[derive(Debug)]
#[linked::object]
pub struct RegionLocal<T>
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

impl<T> RegionLocal<T>
where
    T: Clone + Send + Sync + 'static,
{
    /// Creates a new instance of `RegionLocal`, providing a callback to create the initial value.
    ///
    /// This callback will be called on a thread in the memory region where the value will be used.
    ///
    /// The instance of `RegionLocal` may be cloned and shared between threads using mechanisms
    /// of the [linked object pattern][3]. Every instance from the same family of objects in the
    /// same memory region will reference the same region-local value.
    ///
    /// This type is internally used by the [`region_local!`][1] macro but can also be used
    /// independently of that macro, typically via a [`PerThread`][2] wrapper that automatically
    /// manages the per-thread instance lifecycle and delivery across threads.
    ///
    /// [1]: crate::region_local
    /// [2]: linked::InstancePerThread
    /// [3]: linked
    #[must_use]
    pub fn new(initializer: fn() -> T) -> Self {
        Self::with_clients(
            initializer,
            &HardwareInfoClientFacade::real(),
            HardwareTrackerClientFacade::real(),
        )
    }

    #[must_use]
    pub(crate) fn with_clients(
        initializer: fn() -> T,
        hardware_info: &HardwareInfoClientFacade,
        hardware_tracker: HardwareTrackerClientFacade,
    ) -> Self {
        let memory_region_count = hardware_info.max_memory_region_count();

        let global_state = Arc::new(GlobalState::new(initializer, memory_region_count));

        linked::new!(Self {
            regional_state: Self::try_locate_regional_state(&global_state, &hardware_tracker),
            global_state: Arc::clone(&global_state),
            hardware_tracker: hardware_tracker.clone(),
        })
    }

    fn try_locate_regional_state(
        global_state: &Arc<GlobalState<T>>,
        hardware_tracker: &HardwareTrackerClientFacade,
    ) -> Option<Arc<RegionalState<T>>> {
        if !hardware_tracker.is_thread_memory_region_pinned() {
            return None;
        }

        // This thread is pinned to a specific memory region, so we can directly access the
        // state of that region and skip the regional state lookup on every access.
        let memory_region_id = hardware_tracker.current_memory_region_id();
        Some(global_state.with_regional_state(memory_region_id, Arc::clone))
    }

    /// Executes the provided function with a reference to the value stored
    /// in the current memory region.
    ///
    /// # Example
    ///
    /// ```
    /// use linked::InstancePerThread;
    /// use region_local::RegionLocal;
    ///
    /// let favorite_color_regional = InstancePerThread::new(RegionLocal::new(|| "blue".to_string()));
    ///
    /// // This localizes the object to the current thread. Reuse this object when possible.
    /// let favorite_color = favorite_color_regional.acquire();
    ///
    /// let len = favorite_color.with_local(|color| color.len());
    /// assert_eq!(len, 4);
    /// ```
    pub fn with_local<F, R>(&self, f: F) -> R
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

            // If we got here then the regional state is not initialized. Let's initialize it.
            regional_state.initialize(self.global_state.initializer);
        }
    }

    /// Publishes a new value to all threads in the current memory region.
    ///
    /// The update will be applied to all threads in the current memory region
    /// in a [weakly consistent manner][1]. The updated value will not be visible
    /// in other memory regions - storage is independent for each memory region.
    ///
    /// # Example
    ///
    /// ```
    /// use linked::InstancePerThread;
    /// use region_local::RegionLocal;
    ///
    /// let favorite_color_regional = InstancePerThread::new(RegionLocal::new(|| "blue".to_string()));
    ///
    /// // This localizes the object to the current thread. Reuse this object when possible.
    /// let favorite_color = favorite_color_regional.acquire();
    ///
    /// favorite_color.set_local("red".to_string());
    /// ```
    ///
    /// Updating the value is [weakly consistent][1]. Do not expect the update to be
    /// immediately visible. Even on the same thread, it is only guaranteed to be
    /// immediately visible if the thread is pinned to a specific memory region.
    ///
    /// ```
    /// use std::num::NonZero;
    ///
    /// use linked::InstancePerThread;
    /// use many_cpus::ProcessorSet;
    /// use region_local::RegionLocal;
    ///
    /// let favorite_color_regional = InstancePerThread::new(RegionLocal::new(|| "blue".to_string()));
    ///
    /// // We can use this to pin a thread to a specific processor, to demonstrate a
    /// // situation where you can rely on consistency guarantees for immediate visibility.
    /// let one_processor = ProcessorSet::builder()
    ///     .take(NonZero::new(1).unwrap())
    ///     .unwrap();
    ///
    /// one_processor
    ///     .spawn_thread(move |processor_set| {
    ///         let processor = processor_set.processors().first();
    ///         println!(
    ///             "Thread pinned to processor {} in memory region {}",
    ///             processor.id(),
    ///             processor.memory_region_id()
    ///         );
    ///
    ///         // This localizes the object to the current thread. Reuse this object when possible.
    ///         let favorite_color = favorite_color_regional.acquire();
    ///
    ///         favorite_color.set_local("red".to_string());
    ///
    ///         // This thread is pinned to a specific processor, so it is guaranteed to stay
    ///         // within the same memory region (== on the same physical hardware). This means
    ///         // that an update to a region-local value is immediately visible.
    ///         let color = favorite_color.with_local(|color| color.clone());
    ///         assert_eq!(color, "red");
    ///     })
    ///     .join()
    ///     .unwrap();
    /// ```
    ///
    /// [1]: crate#consistency-guarantees
    pub fn set_local(&self, value: T) {
        // We fix the memory region ID at this point. It may be that the thread migrates to a
        // different memory region during the rest of this function - we do not care about that.
        let memory_region_id = self.hardware_tracker.current_memory_region_id();

        self.global_state
            .with_regional_state(memory_region_id, |regional_state| {
                regional_state.set(value);
            });
    }
}

impl<T> RegionLocal<T>
where
    T: Clone + Copy + Send + Sync + 'static,
{
    /// Gets a copy of the value from the current memory region.
    ///
    /// # Example
    ///
    /// ```
    /// use linked::InstancePerThread;
    /// use region_local::RegionLocal;
    ///
    /// let current_access_token_regional = InstancePerThread::new(RegionLocal::new(|| 0x123100));
    ///
    /// // This localizes the object to the current thread. Reuse this object when possible.
    /// let current_access_token = current_access_token_regional.acquire();
    ///
    /// let token = current_access_token.get_local();
    /// assert_eq!(token, 0x123100);
    /// ```
    #[inline]
    #[must_use]
    pub fn get_local(&self) -> T {
        self.with_local(|v| *v)
    }
}

#[derive(Debug)]
struct GlobalState<T>
where
    T: Clone + Send + Sync + 'static,
{
    /// The initializer that any new region is initialized with. This is only used to create the
    /// initial value for each memory region.
    initializer: fn() -> T,

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
    #[must_use]
    fn new(initializer: fn() -> T, memory_region_count: usize) -> Self {
        let mut regional_states = Vec::with_capacity(memory_region_count);

        for _ in 0..memory_region_count {
            regional_states.push(OnceLock::new());
        }

        Self {
            initializer,
            regional_states: regional_states.into_boxed_slice(),
        }
    }

    /// Executes a function on the regional state for the given memory region.
    fn with_regional_state<F, R>(&self, memory_region_id: MemoryRegionId, f: F) -> R
    where
        F: FnOnce(&Arc<RegionalState<T>>) -> R,
    {
        let slot = &self.regional_states.get(memory_region_id as usize).expect(
            "memory region ID was out of bounds - the platform lied about how many there are",
        );

        // The entire purpose of that OnceLock is to ensure this Arc::new() happens
        // when the current thread is executing in the correct memory region, to place
        // the regional state of every region in that specific region.
        let regional_state = slot.get_or_init(|| Arc::new(RegionalState::new()));

        f(regional_state)
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
    /// We use `ArcSwap` here because it offers very good multithreaded read performance.
    /// In single-threaded and write-heavy scenarios, `RwLock` is faster but those
    /// are not the scenarios we target - we expect the data to be local for long
    /// periods and read from many threads, with writes happening not so often.
    value: ArcSwapOption<RegionalValue<T>>,
}

impl<T> RegionalState<T>
where
    T: Clone + Send + Sync + 'static,
{
    #[must_use]
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

        if let Some(ref value) = *reader
            && let RegionalValue::Ready(value) = &**value
        {
            return Ok(f(value));
        }

        Err(f)
    }

    /// Initializes the region-local value to an initial value, potentially accepting
    /// a value from another thread already doing the same.
    #[cfg_attr(test, mutants::skip)] // Several mutations result in infinite looping.
    fn initialize(&self, initializer: fn() -> T) {
        // This is a conditional swap - we only initialize if we can swap in our "initializing"
        // value onto a clean slate. If someone else got there first, we line up behind them
        // and wait for them to finish before we do anything.

        loop {
            let reader = self.value.load();

            if let Some(ref value) = *reader {
                // Something is already happening.

                if let RegionalValue::Initializing(manual_reset_event) = &**value {
                    // We are waiting for someone else to finish initializing.
                    manual_reset_event.wait();
                }

                // Something has finished happening! Whatever it was, we are now initialized.
                break;
            }

            // Nothing is happening. We may be the first to start initializing.
            let attempt_signal = Arc::new(ManualResetEvent::new(EventState::Unset));
            let attempt = RegionalValue::<T>::Initializing(Arc::clone(&attempt_signal));

            let previous_value = self.value.compare_and_swap(reader, Some(Arc::new(attempt)));

            if !previous_value.is_none() {
                // Someone raced ahead of us. Re-enter loop.
                continue;
            }

            // We must ensure that if initialization panics, we reset the state
            // and signal any waiting threads to prevent them from waiting forever.
            let cleanup_signal = Arc::clone(&attempt_signal);
            let cleanup_self = self; // Create a reference for the cleanup
            let cleanup_guard = scopeguard::guard((), move |()| {
                // If we're still in panic mode when this guard executes, reset the
                // initializing state to None and signal waiters so they can retry.
                cleanup_self.value.store(None);
                cleanup_signal.set();
            });

            let new_value = RegionalValue::Ready(initializer());
            self.value.store(Some(Arc::new(new_value)));

            // We are done initializing. Notify all waiters that they can continue.
            attempt_signal.set();

            // Disarm the cleanup guard since initialization succeeded.
            scopeguard::ScopeGuard::into_inner(cleanup_guard);

            break;
        }
    }

    // Skip mutating - would lead to infinite loop as it looks just like another thread
    // constantly resetting the value, so the conflict resolver will never finish.
    #[cfg_attr(test, mutants::skip)]
    fn set(&self, value: T) {
        self.value
            .store(Some(Arc::new(RegionalValue::Ready(value))));
    }
}

#[derive(derive_more::Debug)]
enum RegionalValue<T> {
    /// One thread has declared that it has started initializing the value.
    ///
    /// It is possible that multiple threads to attempt to start initializing (i.e. to set this
    /// state). To avoid double initialization, the second caller must perform a conditional swap
    /// and become a waiter if it sees someone else got there first.
    ///
    /// This is only used for the first initialization, to avoid every thread attempting to
    /// initialize when they start doing work together at the same time, e.g. on app startup,
    /// because that would create a memory spike as they all try to initialize. Only one can win
    /// this race, so we want to stop the others doing unnecessary work.
    Initializing(#[debug(ignore)] Arc<ManualResetEvent>),

    /// The value has been initialized and is ready for use.
    Ready(T),
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::{ptr, thread};

    use super::*;
    use crate::{MockHardwareInfoClient, MockHardwareTrackerClient, RegionLocalExt, region_local};

    #[cfg(not(miri))] // Miri does not support talking to the real platform.
    #[test]
    fn real_smoke_test() {
        region_local! {
            static FAVORITE_COLOR: String = "blue".to_string();
        }

        FAVORITE_COLOR.with_local(|color| {
            assert_eq!(*color, "blue");
        });

        FAVORITE_COLOR.set_local("red".to_string());

        FAVORITE_COLOR.with_local(|color| {
            assert_eq!(*color, "red");
        });
    }

    #[cfg(not(miri))] // Miri does not support talking to the real platform.
    #[test]
    fn with_non_const_initial_value() {
        region_local!(static FAVORITE_COLOR: Arc<String> = Arc::new("blue".to_string()));

        FAVORITE_COLOR.with_local(|color| {
            assert_eq!(**color, "blue");
        });
    }

    #[cfg(not(miri))] // Miri does not support talking to the real platform.
    #[test]
    fn non_static() {
        let favorite_color_linked =
            linked::InstancePerThread::new(RegionLocal::new(|| "blue".to_string()));

        let favorite_color = favorite_color_linked.acquire();

        favorite_color.with_local(|color| {
            assert_eq!(*color, "blue");
        });

        thread::spawn(move || {
            let favorite_color = favorite_color_linked.acquire();

            favorite_color.with_local(|color| {
                assert_eq!(*color, "blue");
            });

            favorite_color.set_local("red".to_string());

            favorite_color.with_local(|color| {
                assert_eq!(*color, "red");
            });
        })
        .join()
        .unwrap();

        // We do not know whether the other thread was in the same memory region,
        // so we cannot assume that the value is the same in the main thread now.
    }

    #[cfg(not(miri))] // Miri does not support talking to the real platform.
    #[test]
    fn non_static_sync() {
        let favorite_color_linked =
            linked::InstancePerThreadSync::new(RegionLocal::new(|| "blue".to_string()));

        let favorite_color = favorite_color_linked.acquire();

        favorite_color.with_local(|color| {
            assert_eq!(*color, "blue");
        });

        thread::spawn(move || {
            let favorite_color = favorite_color_linked.acquire();

            favorite_color.with_local(|color| {
                assert_eq!(*color, "blue");
            });

            favorite_color.set_local("red".to_string());

            favorite_color.with_local(|color| {
                assert_eq!(*color, "red");
            });
        })
        .join()
        .unwrap();

        // We do not know whether the other thread was in the same memory region,
        // so we cannot assume that the value is the same in the main thread now.
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
            RegionLocal::with_clients(|| "foo".to_string(), &hardware_info, hardware_tracker);

        let value1 = local.with_local(ptr::from_ref);
        let value2 = local.with_local(ptr::from_ref);
        let value3 = local.with_local(ptr::from_ref);

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

        let local = RegionLocal::with_clients(|| 42, &hardware_info, hardware_tracker);

        assert_eq!(local.get_local(), 42);
        assert_eq!(local.get_local(), 42);
        assert_eq!(local.get_local(), 42);
    }

    #[test]
    fn update_does_not_propagate_to_all_regions() {
        let mut hardware_tracker = MockHardwareTrackerClient::new();

        hardware_tracker
            .expect_is_thread_memory_region_pinned()
            .return_const(false);

        // Initial read + write + read.
        hardware_tracker
            .expect_current_memory_region_id()
            .times(3)
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

        let local = RegionLocal::with_clients(|| 42, &hardware_info, hardware_tracker);

        assert_eq!(local.get_local(), 42);
        local.set_local(43);
        assert_eq!(local.get_local(), 43);

        assert_eq!(local.get_local(), 42);
        assert_eq!(local.get_local(), 42);
        assert_eq!(local.get_local(), 42);
    }

    #[test]
    fn immediate_set_does_not_propagate_to_all_regions() {
        let mut hardware_tracker = MockHardwareTrackerClient::new();

        hardware_tracker
            .expect_is_thread_memory_region_pinned()
            .return_const(false);

        // Initial write + read.
        hardware_tracker
            .expect_current_memory_region_id()
            .times(2)
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

        let local = RegionLocal::with_clients(|| 42, &hardware_info, hardware_tracker);

        local.set_local(43);
        assert_eq!(local.get_local(), 43);

        assert_eq!(local.get_local(), 42);
        assert_eq!(local.get_local(), 42);
        assert_eq!(local.get_local(), 42);
    }

    #[test]
    fn does_not_relocalize_when_pinned() {
        let mut hardware_tracker = MockHardwareTrackerClient::new();

        hardware_tracker
            .expect_is_thread_memory_region_pinned()
            .return_const(true);

        // We expect this to only ever be called once because we are pinned.
        hardware_tracker
            .expect_current_memory_region_id()
            .times(1)
            .return_const(3 as MemoryRegionId);

        let hardware_tracker = HardwareTrackerClientFacade::from_mock(hardware_tracker);

        let mut hardware_info = MockHardwareInfoClient::new();

        hardware_info
            .expect_max_memory_region_count()
            .return_const(10_usize);

        let hardware_info = HardwareInfoClientFacade::from_mock(hardware_info);

        let local = RegionLocal::with_clients(|| 42, &hardware_info, hardware_tracker);

        assert_eq!(local.get_local(), 42);
        assert_eq!(local.get_local(), 42);
        assert_eq!(local.get_local(), 42);
    }

    #[test]
    fn callback_panic_leaves_consistent_state() {
        let mut hardware_tracker = MockHardwareTrackerClient::new();

        hardware_tracker
            .expect_is_thread_memory_region_pinned()
            .return_const(false);

        // First successful call to initialize.
        hardware_tracker
            .expect_current_memory_region_id()
            .times(1)
            .return_const(0 as MemoryRegionId);

        // Second call where callback panics.
        hardware_tracker
            .expect_current_memory_region_id()
            .times(1)
            .return_const(0 as MemoryRegionId);

        // Third call to verify state is still consistent.
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

        let local = RegionLocal::with_clients(|| 42, &hardware_info, hardware_tracker);

        // First call succeeds and initializes the region.
        assert_eq!(local.get_local(), 42);

        // Second call with panicking closure.
        let panic_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            local.with_local(|_| {
                panic!("User callback panicked!");
            })
        }));
        assert!(panic_result.is_err());

        // Third call should still work and return the initialized value.
        assert_eq!(local.get_local(), 42);
    }

    #[test]
    fn callback_panic_during_initialization() {
        let mut hardware_tracker = MockHardwareTrackerClient::new();

        hardware_tracker
            .expect_is_thread_memory_region_pinned()
            .return_const(false);

        // Call where callback panics during initialization.
        hardware_tracker
            .expect_current_memory_region_id()
            .times(1)
            .return_const(0 as MemoryRegionId);

        // Second call to verify initialization can still proceed.
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

        let local = RegionLocal::with_clients(|| 42, &hardware_info, hardware_tracker);

        // First call with panicking closure should fail but not break state.
        let panic_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            local.with_local(|_| {
                panic!("User callback panicked during initialization!");
            })
        }));
        assert!(panic_result.is_err());

        // Second call should successfully initialize and work.
        assert_eq!(local.get_local(), 42);
    }

    #[test]
    #[cfg(not(miri))] // Test uses thread::sleep which is not supported by Miri
    fn initializer_panic_does_not_block_other_threads() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        // Since we need a function pointer for the initializer, we'll use a different approach.
        // We'll create a type that panics during Drop, which will be called by the initializer.
        static mut PANIC_ON_FIRST_CALL: bool = true;

        fn panicking_initializer() -> i32 {
            // SAFETY: We access a mutable static variable in test code.
            // This is safe because tests are single-threaded with respect to this static.
            let should_panic = unsafe { PANIC_ON_FIRST_CALL };
            if should_panic {
                // SAFETY: We modify a mutable static variable in test code.
                // This is safe because tests are single-threaded with respect to this static.
                unsafe {
                    PANIC_ON_FIRST_CALL = false;
                }
                panic!("Initializer panicked!");
            }
            42
        }

        let mut hardware_tracker = MockHardwareTrackerClient::new();
        hardware_tracker
            .expect_is_thread_memory_region_pinned()
            .return_const(false);

        // Calls from both threads to the same region.
        hardware_tracker
            .expect_current_memory_region_id()
            .times(2)
            .return_const(0 as MemoryRegionId);

        let hardware_tracker = HardwareTrackerClientFacade::from_mock(hardware_tracker);

        let mut hardware_info = MockHardwareInfoClient::new();
        hardware_info
            .expect_max_memory_region_count()
            .return_const(10_usize);

        let hardware_info = HardwareInfoClientFacade::from_mock(hardware_info);

        let local =
            RegionLocal::with_clients(panicking_initializer, &hardware_info, hardware_tracker);

        let barrier = Arc::new(Barrier::new(2));
        let local = Arc::new(local);

        let barrier1 = Arc::clone(&barrier);
        let local1 = Arc::clone(&local);
        let handle1 = thread::spawn(move || {
            barrier1.wait();
            // This thread will trigger the panicking initializer.
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| local1.get_local()))
        });

        let barrier2 = Arc::clone(&barrier);
        let local2 = Arc::clone(&local);
        let handle2 = thread::spawn(move || {
            barrier2.wait();
            // This thread should succeed after the first one panics.
            // Give the first thread a small head start.
            thread::sleep(std::time::Duration::from_millis(50));
            local2.get_local()
        });

        let result1 = handle1.join().expect("Thread 1 should not panic");
        let result2 = handle2.join().expect("Thread 2 should not panic");

        // First thread should have caught the panic.
        result1.unwrap_err();
        // Second thread should have succeeded.
        assert_eq!(result2, 42);
    }
}

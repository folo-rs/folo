use std::sync::{Arc, OnceLock};

use arc_swap::ArcSwapOption;
use many_cpus::MemoryRegionId;

use crate::{
    hw_info_client::{HardwareInfoClient, HardwareInfoClientFacade},
    hw_tracker_client::{HardwareTrackerClient, HardwareTrackerClientFacade},
};

/// The backing type behind static variables in a `region_local!` block. User code would generally
/// use extension methods on the [`RegionLocalExt`] type for simpler syntax.
///
/// Refer to [crate-level documentation][crate] for more information.
#[derive(Debug)]
#[linked::object]
pub struct RegionLocalStatic<T>
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

impl<T> RegionLocalStatic<T>
where
    T: Clone + Send + Sync + 'static,
{
    /// Note: this function exists to serve the inner workings of the `region_local!` macro and
    /// should not be used directly. It is not part of the public API and may be removed or changed
    /// at any time.
    #[doc(hidden)]
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

    /// Executes the provided function with a reference to the stored value.
    pub fn with<F, R>(&self, f: F) -> R
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
            let initial_value = self.global_state.initial_value.clone();
            regional_state.set(initial_value);
        }
    }

    /// Updates the stored value in the current memory region.
    ///
    /// The update will not be visible in other memory regions.
    pub fn set(&self, value: T) {
        // We fix the memory region ID at this point. It may be that the thread migrates to a
        // different memory region during the rest of this function - we do not care about that.
        let memory_region_id = self.hardware_tracker.current_memory_region_id();

        self.global_state
            .with_regional_state(memory_region_id, |regional_state| {
                regional_state.set(value);
            })
    }
}

impl<T> RegionLocalStatic<T>
where
    T: Clone + Copy + Send + Sync + 'static,
{
    /// Gets a copy of the stored value.
    pub fn get(&self) -> T {
        self.with(|v| *v)
    }
}

#[derive(Debug)]
struct GlobalState<T>
where
    T: Clone + Send + Sync + 'static,
{
    /// The initial value that any new region is initialized with. This is only used to assign the
    /// initial value in each memory region.
    ///
    /// We do not currently drop this because due to multiple threads racing to initialize a
    /// memory region, it can be difficult to determine when it is truly "unused". Something
    /// to consider in a future iteration.
    initial_value: T,

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

        Self {
            initial_value,
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
    /// are not the scenarios we target - we expect the data to be local for long
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

    fn set(&self, value: T) {
        self.value.store(Some(Arc::new(value)));
    }
}

/// Marks static variables in the macro block as region-local.
///
/// Refer to [crate-level documentation][crate] for more information.
#[macro_export]
macro_rules! region_local {
    () => {};

    ($(#[$attr:meta])* $vis:vis static $NAME:ident: $t:ty = $initial_value:expr; $($rest:tt)*) => (
        $crate::region_local!($(#[$attr])* $vis static $NAME: $t = $initial_value);
        $crate::region_local!($($rest)*);
    );

    ($(#[$attr:meta])* $vis:vis static $NAME:ident: $t:ty = $initial_value:expr) => {
        linked::instance_per_thread! {
            $(#[$attr])* $vis static $NAME: $crate::RegionLocalStatic<$t> =
                $crate::RegionLocalStatic::new($initial_value);
        }
    };
}

/// Extension trait that adds convenience methods to region-local static variables
/// in a `region_local!` block.
pub trait RegionLocalExt<T> {
    /// Executes the provided function with a reference to the stored value
    /// local to the current memory region.
    fn with_local<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&T) -> R;

    /// Updates the stored value in the current memory region.
    fn set_local(&self, value: T);
}

/// Extension trait that adds convenience methods to region-local static variables
/// in a `region_local!` block, specifically for `Copy` types.
pub trait RegionLocalCopyExt<T>
where
    T: Copy,
{
    /// Gets a copy of the stored value local to the current memory region.
    fn get_local(&self) -> T;
}

impl<T> RegionLocalExt<T> for linked::PerThreadStatic<RegionLocalStatic<T>>
where
    T: Clone + Send + Sync + 'static,
{
    fn with_local<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        self.with(|inner| inner.with(f))
    }

    fn set_local(&self, value: T) {
        self.with(|inner| inner.set(value));
    }
}

impl<T> RegionLocalCopyExt<T> for linked::PerThreadStatic<RegionLocalStatic<T>>
where
    T: Clone + Copy + Send + Sync + 'static,
{
    fn get_local(&self) -> T {
        self.with(|inner| inner.get())
    }
}

#[cfg(test)]
mod tests {
    use std::ptr;
    use std::sync::Arc;

    use crate::region_local;
    use crate::{
        hw_info_client::MockHardwareInfoClient, hw_tracker_client::MockHardwareTrackerClient,
    };

    use super::*;

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
            RegionLocalStatic::with_clients(|| "foo".to_string(), hardware_info, hardware_tracker);

        let value1 = local.with(ptr::from_ref);
        let value2 = local.with(ptr::from_ref);
        let value3 = local.with(ptr::from_ref);

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

        let local = RegionLocalStatic::with_clients(42, hardware_info, hardware_tracker);

        assert_eq!(local.get(), 42);
        assert_eq!(local.get(), 42);
        assert_eq!(local.get(), 42);
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

        let local = RegionLocalStatic::with_clients(42, hardware_info, hardware_tracker);

        assert_eq!(local.get(), 42);
        local.set(43);
        assert_eq!(local.get(), 43);

        assert_eq!(local.get(), 42);
        assert_eq!(local.get(), 42);
        assert_eq!(local.get(), 42);
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

        let local = RegionLocalStatic::with_clients(42, hardware_info, hardware_tracker);

        local.set(43);
        assert_eq!(local.get(), 43);

        assert_eq!(local.get(), 42);
        assert_eq!(local.get(), 42);
        assert_eq!(local.get(), 42);
    }
}

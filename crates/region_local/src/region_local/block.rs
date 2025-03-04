use std::sync::RwLock;

use many_cpus::MemoryRegionId;

use crate::region_local::{
    hw_info_client::{HardwareInfoClient, HardwareInfoClientFacade},
    hw_tracker_client::{HardwareTrackerClient, HardwareTrackerClientFacade},
};

/// The backing type behind variables in a `region_local!` block. On read, returns a copy of the
/// value stored in the same memory region as the processor currently executing the code.
///
/// You can think of region-local variables as an additional level of caching between the
/// processor's L3 caches and main memory - it uses the capacity of main memory but guarantees
/// the best performance as far as accessing main memory is concerned. Contrast to reading arbitrary
/// data from main memory, which may be in far-away memory regions that are slower to access.
///
/// # Consistency guarantees
///
/// Writes are partly synchronized and eventually consistent, with an undefined order of resolving
/// simultaneous writes. Writes from the same thread are resolved sequentially, with the last write
/// from that thread winning from among other writes from that thread.
///
/// Writes are immediately visible from the originating thread, with the caveats that:
/// 1. Eventually consistent writes from other threads may be applied at any time, such as between
///    a write and an immediately following read.
/// 2. A thread may migrate to a new memory region between the write and read operations, which
///    invalidates any causal link between the two operations.
///
/// In general, you can only have firm expectations about the data seen by a sequence of reads if
/// the writes are always performed from a single thread.
///
/// # Example
///
/// This type is used via the `region_local!` macro, which works in a very similar manner to the
/// `thread_local!` macro. Within the macro block, define one or more static variables, then read
/// via `.with()` or update the value via `.set()`.
///
/// ```
/// use folo_state::region_local;
///
/// region_local! {
///     static FAVORITE_COLOR: String = "blue".to_string();
/// }
///
/// fn foo() {
///     FAVORITE_COLOR.with(|color| {
///         println!("My favorite color is {color}");
///     });
///
///     FAVORITE_COLOR.set("red".to_string());
/// }
/// ```
#[derive(Debug)]
pub struct RegionLocalKey<T>
where
    T: Clone + 'static,
{
    state: RwLock<State<T>>,

    hardware_info: HardwareInfoClientFacade,
    hardware_tracker: HardwareTrackerClientFacade,
}

#[derive(Debug)]
enum State<T> {
    // The instance has been created but not yet initialized, all we have is the initial value.
    Created {
        initial_value: T,
    },

    // The instance has been initialized, we have one slot per memory region.
    Initialized {
        region_values: Box<[Option<Box<T>>]>,
    },
}

impl<T> State<T> {
    const fn new(initial_value: T) -> Self {
        Self::Created { initial_value }
    }
}

impl<T> RegionLocalKey<T>
where
    T: Clone + 'static,
{
    /// Note: this function exists to serve the inner workings of the `region_local!` macro and
    /// should not be used directly. It is not part of the public API and may be removed or changed
    /// at any time.
    #[doc(hidden)]
    pub const fn new(value: T) -> Self {
        Self::with_clients(
            value,
            HardwareInfoClientFacade::real(),
            HardwareTrackerClientFacade::real(),
        )
    }

    pub(crate) const fn with_clients(
        value: T,
        hardware_info: HardwareInfoClientFacade,
        hardware_tracker: HardwareTrackerClientFacade,
    ) -> Self {
        Self {
            state: RwLock::new(State::new(value)),
            hardware_info,
            hardware_tracker,
        }
    }

    /// Executes the provided closure with a reference to the stored value.
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        // Horrible inefficient implementation just to get tests to pass.
        //
        // Optimize: avoid the various checks and such.
        // Optimize: can we do better than RwLock?
        // Optimize: region_values itself may be stored outside the current memory region...

        let memory_region_id = self.hardware_tracker.current_memory_region_id();
        self.with_in_region(memory_region_id, f)
    }

    fn with_in_region<R>(&self, memory_region_id: MemoryRegionId, f: impl FnOnce(&T) -> R) -> R {
        {
            // Optimistic case - a value already exists for this memory region.
            let state = self.state.read().expect(ERR_POISONED_LOCK);

            if let Some(value) = Self::try_read_core(memory_region_id, &*state) {
                return f(value);
            }
        }

        {
            // Fallback case - we need to obtain a value for this memory region.
            let mut state = self.state.write().expect(ERR_POISONED_LOCK);

            // It could be that something already assigned the value, so check again first.
            if let Some(value) = Self::try_read_core(memory_region_id, &*state) {
                return f(value);
            }

            self.initialize_slot(memory_region_id, &mut *state);

            // We release the write lock here to avoid holding it while calling the closure.
        }

        // If we got here, we did a write, so recurse back to go back to the optimistic case
        // and read the value that we just wrote, because optimism is now guaranteed to win.
        self.with_in_region(memory_region_id, f)
    }

    fn try_read_core(memory_region_id: MemoryRegionId, state: &State<T>) -> Option<&T> {
        match state {
            State::Created { .. } => {}
            State::Initialized { region_values } => {
                // This bounds check could only fail if the platform
                // lied about the maximum memory region ID.
                let slot = region_values.get(memory_region_id as usize);

                if let Some(Some(value)) = slot {
                    return Some(value);
                }
            }
        }

        None
    }

    fn initialize_slot(&self, memory_region_id: MemoryRegionId, state: &mut State<T>) {
        // If the state is not yet initialized, we initialize it now, consuming the initial
        // value. Otherwise, we simply clone the initial value from the first filled slot.
        // The values in the slots are all the same, so it does not matter which we pick.

        match state {
            State::Created { initial_value } => {
                let mut region_values =
                    vec![None; self.hardware_info.max_memory_region_id() as usize + 1];
                region_values[memory_region_id as usize] = Some(Box::new(initial_value.clone()));

                *state = State::Initialized {
                    region_values: region_values.into_boxed_slice(),
                }
            }
            State::Initialized { region_values } => {
                let boxed_value = region_values.iter().find_map(|v| v.as_ref()).expect(
                    "if we are initialized, we must have at least one value in region_values",
                );

                region_values[memory_region_id as usize] = Some(boxed_value.clone());
            }
        }
    }

    /// Updates the stored value.
    ///
    /// The update will be applied to all memory regions in an eventually consistent manner.
    pub fn set(&self, value: T) {
        let memory_region_id = self.hardware_tracker.current_memory_region_id();

        let mut state = self.state.write().expect(ERR_POISONED_LOCK);

        match &mut *state {
            State::Created { initial_value } => {
                *initial_value = value;
            }
            State::Initialized { region_values } => {
                for (index, slot) in region_values.iter_mut().enumerate() {
                    if index == memory_region_id as usize {
                        // The current memory region can get a clone immediately.
                        *slot = Some(Box::new(value.clone()));
                    } else {
                        // Other memory regions will copy on read.
                        *slot = None;
                    }
                }
            }
        }
    }
}

impl<T> RegionLocalKey<T>
where
    T: Clone + Copy + 'static,
{
    /// Gets a copy of the stored value.
    pub fn get(&self) -> T {
        self.with(|v| *v)
    }
}

const ERR_POISONED_LOCK: &str = "poisoned lock - safe execution no longer possible";

/// See [RegionLocalKey].
#[macro_export]
macro_rules! region_local {
    () => {};

    ($(#[$attr:meta])* $vis:vis static $NAME:ident: $t:ty = $e:expr; $($rest:tt)*) => (
        $crate::region_local!($(#[$attr])* $vis static $NAME: $t = $e);
        $crate::region_local!($($rest)*);
    );

    ($(#[$attr:meta])* $vis:vis static $NAME:ident: $t:ty = $e:expr) => {
        $(#[$attr])* $vis static $NAME: $crate::RegionLocalKey<$t> =
            $crate::RegionLocalKey::new($e);
    };
}

#[cfg(test)]
mod tests {
    use std::ptr;

    use crate::region_local;
    use crate::region_local::{
        hw_info_client::MockHardwareInfoClient, hw_tracker_client::MockHardwareTrackerClient,
    };

    use super::*;

    #[test]
    fn real_smoke_test() {
        region_local! {
            static FAVORITE_COLOR: &str = "blue";
        }

        FAVORITE_COLOR.with(|color| {
            assert_eq!(*color, "blue");
        });

        FAVORITE_COLOR.set("red");

        FAVORITE_COLOR.with(|color| {
            assert_eq!(*color, "red");
        });
    }

    #[test]
    fn different_regions_have_different_clones() {
        let mut hardware_tracker = MockHardwareTrackerClient::new();

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
            .expect_max_memory_region_id()
            .return_const(9 as MemoryRegionId);

        let hardware_info = HardwareInfoClientFacade::from_mock(hardware_info);

        let local =
            RegionLocalKey::with_clients("foo".to_string(), hardware_info, hardware_tracker);

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
            .expect_max_memory_region_id()
            .return_const(9 as MemoryRegionId);

        let hardware_info = HardwareInfoClientFacade::from_mock(hardware_info);

        let local = RegionLocalKey::with_clients(42, hardware_info, hardware_tracker);

        assert_eq!(local.get(), 42);
        assert_eq!(local.get(), 42);
        assert_eq!(local.get(), 42);
    }

    #[test]
    fn update_propagates_to_all_regions() {
        let mut hardware_tracker = MockHardwareTrackerClient::new();

        // Initial read + write.
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
            .expect_max_memory_region_id()
            .return_const(9 as MemoryRegionId);

        let hardware_info = HardwareInfoClientFacade::from_mock(hardware_info);

        let local = RegionLocalKey::with_clients(42, hardware_info, hardware_tracker);

        assert_eq!(local.get(), 42);
        local.set(43);

        assert_eq!(local.get(), 43);
        assert_eq!(local.get(), 43);
        assert_eq!(local.get(), 43);
    }

    #[test]
    fn immediate_set_propagates_to_all_regions() {
        let mut hardware_tracker = MockHardwareTrackerClient::new();

        // Initial write.
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
            .expect_max_memory_region_id()
            .return_const(9 as MemoryRegionId);

        let hardware_info = HardwareInfoClientFacade::from_mock(hardware_info);

        let local = RegionLocalKey::with_clients(42, hardware_info, hardware_tracker);

        local.set(43);

        assert_eq!(local.get(), 43);
        assert_eq!(local.get(), 43);
        assert_eq!(local.get(), 43);
    }
}

use std::sync::RwLock;

use folo_hw::MemoryRegionId;

use crate::region_local::hw::{Hardware, HardwareFacade};

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
    hardware: HardwareFacade,
}

#[derive(Debug)]
struct State<T> {
    // Can grow if we find a memory region that we have not seen before.
    region_values: Vec<Option<Box<T>>>,

    // Consumed on first read, when we assign it to the current memory region.
    // Discarded on first write, if a write occurs before a read.
    initial_value: Option<T>,
}

impl<T> State<T> {
    const fn new(initial_value: T) -> Self {
        Self {
            region_values: Vec::new(),
            initial_value: Some(initial_value),
        }
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
        Self::with_hardware(value, HardwareFacade::real())
    }

    pub(crate) const fn with_hardware(value: T, hardware: HardwareFacade) -> Self {
        Self {
            state: RwLock::new(State::new(value)),
            hardware,
        }
    }

    /// Executes the provided closure with a reference to the stored value.
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        // Horrible inefficient implementation just to get tests to pass.
        //
        // Optimize: avoid the various checks and such.
        // Optimize: can we do better than RwLock?
        // Optimize: region_values itself may be stored outside the current memory region...

        let memory_region_id = self.hardware.current_memory_region_id();
        self.with_in_region(memory_region_id, f)
    }

    fn with_in_region<R>(&self, memory_region_id: MemoryRegionId, f: impl FnOnce(&T) -> R) -> R {
        {
            // Optimistic case - a value already exists for this memory region.
            let state = self.state.read().expect(ERR_POISONED_LOCK);

            if let Some(Some(value)) = state.region_values.get(memory_region_id as usize) {
                return f(value);
            }
        }

        {
            // Fallback case - we need to obtain a value for this memory region.
            let mut state = self.state.write().expect(ERR_POISONED_LOCK);

            // It could be that something already assigned the value, so check again first.
            if let Some(Some(value)) = state.region_values.get(memory_region_id as usize) {
                return f(value);
            }

            // If there is an initial value, we use that. Otherwise, we take the first value
            // that we find in the region_values vector (they are all the same value).
            let value = Self::create_local_value(&mut state);

            Self::write_value(memory_region_id, &mut state, value);

            // We release the write lock here to avoid holding it while calling the closure.
        }

        // If we got here, we did a write, so recurse back to go back to the optimistic case
        // and read the value that we just wrote, because optimism is now guaranteed to win.
        self.with_in_region(memory_region_id, f)
    }

    fn create_local_value(state: &mut State<T>) -> Box<T> {
        if let Some(value) = state.initial_value.take() {
            Box::new(value)
        } else {
            let value = state
                .region_values
                .iter()
                .find_map(|v| v.as_ref())
                .expect("if the initial value has been used up, we must have at least one value in region_values");

            value.clone()
        }
    }

    /// Updates the stored value.
    ///
    /// The update will be applied to all memory regions in an eventually consistent manner.
    pub fn set(&self, value: T) {
        let memory_region_id = self.hardware.current_memory_region_id();

        let mut state = self.state.write().expect(ERR_POISONED_LOCK);

        state.initial_value = None;
        state.region_values.clear();

        Self::write_value(memory_region_id, &mut state, Box::new(value));
    }

    fn write_value(memory_region_id: MemoryRegionId, state: &mut State<T>, value: Box<T>) {
        // Ensure that we have enough space in the region_values vector.
        while state.region_values.len() <= memory_region_id as usize {
            state.region_values.push(None);
        }

        state.region_values[memory_region_id as usize] = Some(value);
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
    use crate::region_local::hw::MockHardware;

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
        let mut hardware = MockHardware::new();

        hardware
            .expect_current_memory_region_id()
            .times(1)
            .return_const(0 as MemoryRegionId);
        hardware
            .expect_current_memory_region_id()
            .times(1)
            .return_const(1 as MemoryRegionId);
        hardware
            .expect_current_memory_region_id()
            .times(1)
            .return_const(9 as MemoryRegionId);

        let hardware = HardwareFacade::from_mock(hardware);

        let local = RegionLocalKey::with_hardware("foo".to_string(), hardware);

        let value1 = local.with(ptr::from_ref);
        let value2 = local.with(ptr::from_ref);
        let value3 = local.with(ptr::from_ref);

        assert_ne!(value1, value2);
        assert_ne!(value1, value3);
    }

    #[test]
    fn initial_value_propagates_to_all_regions() {
        let mut hardware = MockHardware::new();

        hardware
            .expect_current_memory_region_id()
            .times(1)
            .return_const(0 as MemoryRegionId);
        hardware
            .expect_current_memory_region_id()
            .times(1)
            .return_const(1 as MemoryRegionId);
        hardware
            .expect_current_memory_region_id()
            .times(1)
            .return_const(9 as MemoryRegionId);

        let hardware = HardwareFacade::from_mock(hardware);

        let local = RegionLocalKey::with_hardware(42, hardware);

        assert_eq!(local.get(), 42);
        assert_eq!(local.get(), 42);
        assert_eq!(local.get(), 42);
    }

    #[test]
    fn update_propagates_to_all_regions() {
        let mut hardware = MockHardware::new();

        // Initial read + write.
        hardware
            .expect_current_memory_region_id()
            .times(2)
            .return_const(5 as MemoryRegionId);

        // Reads to verify.
        hardware
            .expect_current_memory_region_id()
            .times(1)
            .return_const(0 as MemoryRegionId);
        hardware
            .expect_current_memory_region_id()
            .times(1)
            .return_const(1 as MemoryRegionId);
        hardware
            .expect_current_memory_region_id()
            .times(1)
            .return_const(9 as MemoryRegionId);

        let hardware = HardwareFacade::from_mock(hardware);

        let local = RegionLocalKey::with_hardware(42, hardware);

        assert_eq!(local.get(), 42);
        local.set(43);

        assert_eq!(local.get(), 43);
        assert_eq!(local.get(), 43);
        assert_eq!(local.get(), 43);
    }

    #[test]
    fn immediate_set_propagates_to_all_regions() {
        let mut hardware = MockHardware::new();

        // Initial write.
        hardware
            .expect_current_memory_region_id()
            .times(1)
            .return_const(5 as MemoryRegionId);

        // Reads to verify.
        hardware
            .expect_current_memory_region_id()
            .times(1)
            .return_const(0 as MemoryRegionId);
        hardware
            .expect_current_memory_region_id()
            .times(1)
            .return_const(1 as MemoryRegionId);
        hardware
            .expect_current_memory_region_id()
            .times(1)
            .return_const(9 as MemoryRegionId);

        let hardware = HardwareFacade::from_mock(hardware);

        let local = RegionLocalKey::with_hardware(42, hardware);

        local.set(43);

        assert_eq!(local.get(), 43);
        assert_eq!(local.get(), 43);
        assert_eq!(local.get(), 43);
    }
}

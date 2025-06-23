use std::alloc::Layout;
use std::num::NonZero;
use std::ptr::NonNull;

use crate::MemorySlab;

/// The result of reserving memory in a [`MemoryPool`].
///
/// Contains both the coordinates for accessing the memory and a pointer to the reserved memory.
/// Acts as both the reservation and the key - the user must return this to the pool to release
/// the memory capacity.
///    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    ///
    /// use memory_slab::MemoryPool;
    ///
    /// let layout = Layout::new::<i64>();
    /// let mut pool = MemoryPool::new(layout);
    ///
    /// let reservation = pool.reserve();
    ///
    /// // Write to the memory pointer
    /// unsafe {
    ///     // SAFETY: The pointer is valid and aligned for i64, and we own the memory.
    ///     reservation.ptr().cast::<i64>().as_ptr().write(-123);
    /// }
    ///
    /// // Read from the memory pointer
    /// let value = unsafe {
    ///     // SAFETY: The pointer is valid and the memory was just initialized.
    ///     reservation.ptr().cast::<i64>().as_ptr().read()
    /// };
    /// assert_eq!(value, -123);
    ///
    /// // The reservation must be returned to release the memory
    /// pool.release(reservation);
    /// ```
#[derive(Debug)]
pub struct PoolReservation {
    /// The coordinates of the memory block within the pool structure.
    coordinates: MemoryBlockCoordinates,

    /// A pointer to the reserved memory block.
    ptr: NonNull<()>,
}

impl PoolReservation {
    /// Returns a pointer to the reserved memory block.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    ///
    /// use memory_slab::MemoryPool;
    ///
    /// let layout = Layout::new::<f64>();
    /// let mut pool = MemoryPool::new(layout);
    /// let reservation = pool.reserve();
    ///
    /// // Write data to the reserved memory
    /// unsafe {
    ///     // SAFETY: The pointer is valid and aligned for f64, and we own the memory.
    ///     let ptr = reservation.ptr().cast::<f64>();
    ///     ptr.as_ptr().write(3.14159);
    /// }
    ///
    /// // Read data back from the memory
    /// let value = unsafe {
    ///     // SAFETY: The pointer is valid and the memory was just initialized.
    ///     let ptr = reservation.ptr().cast::<f64>();
    ///     ptr.as_ptr().read()
    /// };
    /// assert_eq!(value, 3.14159);
    /// # pool.release(reservation);
    /// ```
    #[must_use]
    pub fn ptr(&self) -> NonNull<()> {
        self.ptr
    }
}

/// An object pool of unbounded size that provides type-erased memory allocation.
///
/// A dynamically growing memory pool that manages multiple fixed-capacity slabs internally
/// and automatically allocates new slabs when needed.
///
/// The pool returns a [`PoolReservation`] for each reserved memory block, which acts as both
/// the key and provides direct access to the memory pointer.
///
/// # Out of band access
///
/// The collection does not create or keep references to the memory blocks, so it is valid to access
/// memory via pointers and to create custom references to memory from unsafe code even when not
/// holding an exclusive reference to the collection.
///
/// # Resource usage
///
/// As of today, the collection never shrinks, though future versions may offer facilities to do so.
///    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    ///
    /// use memory_slab::MemoryPool;
    ///
    /// let layout = Layout::new::<u32>();
    /// let mut pool = MemoryPool::new(layout);
    ///
    /// // Reserve memory and get a reservation
    /// let reservation = pool.reserve();
    ///
    /// // Write to the memory
    /// unsafe {
    ///     // SAFETY: The pointer is valid and aligned for u32, and we own the memory.
    ///     reservation.ptr().cast::<u32>().as_ptr().write(42);
    /// }
    ///
    /// // Read from the memory
    /// let value = unsafe {
    ///     // SAFETY: The pointer is valid and the memory was just initialized.
    ///     reservation.ptr().cast::<u32>().as_ptr().read()
    /// };
    /// assert_eq!(value, 42);
    ///
    /// // Release the memory back to the pool
    /// pool.release(reservation);
    /// ```
#[derive(Debug)]
pub struct MemoryPool {
    /// The layout of memory blocks managed by this pool.
    item_layout: Layout,

    /// The capacity of each individual slab in the pool.
    slab_capacity: NonZero<usize>,

    /// The slabs that provide the storage of the pool.
    /// We use a Vec here to allow for dynamic capacity growth.
    ///
    /// For now, we only grow this Vec but in theory, one could implement shrinking as well
    /// by removing empty slabs.
    slabs: Vec<MemorySlab>,

    /// Lowest index of any slab that has a vacant slot, if known. We use this to avoid scanning
    /// the entire collection for vacant slots when reserving memory. This being `None` does not
    /// imply that there are no vacant slots, it just means we do not know what slab they are in.
    /// In other words, this is a cache, not the ground truth - we set it to `None` when we lose
    /// confidence that the data is still valid but when we have no need to look up the new value.
    slab_with_vacant_slot_index: Option<usize>,
}

/// Today, we assemble the pool from memory slabs, each containing a fixed number of memory blocks.
///
/// In the future, we may choose to be smarter about this, e.g. choosing the slab size dynamically
/// based on the size of the memory layout in order to match a memory page size, or another similar criterion.
/// This is why the parameter is also not exposed in the public API - we may want to change how we
/// perform the memory layout in a future version.
#[cfg(not(miri))]
const DEFAULT_SLAB_CAPACITY: usize = 128;

// Under Miri, we use a smaller slab capacity because Miri test runtime scales by memory usage.
#[cfg(miri)]
const DEFAULT_SLAB_CAPACITY: usize = 16;

impl MemoryPool {
    /// Creates a new [`MemoryPool`] with the specified item memory layout.
    ///
    /// The pool starts empty and will automatically grow as needed when memory is reserved.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    ///
    /// use memory_slab::MemoryPool;
    ///
    /// // Create a pool for storing u64 values
    /// let layout = Layout::new::<u64>();
    /// let pool = MemoryPool::new(layout);
    ///
    /// assert_eq!(pool.len(), 0);
    /// assert!(pool.is_empty());
    /// assert_eq!(pool.item_layout(), layout);
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the layout has zero size.
    #[must_use]
    pub fn new(item_layout: Layout) -> Self {
        Self::with_slab_capacity(item_layout, NonZero::new(DEFAULT_SLAB_CAPACITY).unwrap())
    }

    /// Creates a new [`MemoryPool`] with the specified memory layout and slab capacity.
    ///
    /// The slab capacity determines how many items each internal slab can hold. This can be
    /// useful for performance tuning in specific scenarios, though the default capacity
    /// should work well for most use cases.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    /// use std::num::NonZero;
    ///
    /// use memory_slab::MemoryPool;
    ///
    /// // Create a pool with custom slab capacity
    /// let layout = Layout::new::<u32>();
    /// let slab_capacity = NonZero::new(64).unwrap();
    /// let pool = MemoryPool::with_slab_capacity(layout, slab_capacity);
    ///
    /// assert_eq!(pool.len(), 0);
    /// assert!(pool.is_empty());
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the layout has zero size.
    #[must_use]
    pub fn with_slab_capacity(item_layout: Layout, slab_capacity: NonZero<usize>) -> Self {
        assert!(
            item_layout.size() > 0,
            "MemoryPool must have non-zero memory block size"
        );

        Self {
            item_layout,
            slab_capacity,
            slabs: Vec::new(),
            slab_with_vacant_slot_index: None,
        }
    }

    /// Returns the memory layout used by items in this pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    ///
    /// use memory_slab::MemoryPool;
    ///
    /// let layout = Layout::new::<u128>();
    /// let pool = MemoryPool::new(layout);
    ///
    /// assert_eq!(pool.item_layout(), layout);
    /// assert_eq!(pool.item_layout().size(), std::mem::size_of::<u128>());
    /// ```
    #[must_use]
    pub fn item_layout(&self) -> Layout {
        self.item_layout
    }

    /// The number of memory blocks in the pool that have been reserved.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    ///
    /// use memory_slab::MemoryPool;
    ///
    /// let layout = Layout::new::<i32>();
    /// let mut pool = MemoryPool::new(layout);
    ///
    /// assert_eq!(pool.len(), 0);
    ///
    /// let reservation1 = pool.reserve();
    /// assert_eq!(pool.len(), 1);
    ///
    /// let reservation2 = pool.reserve();
    /// assert_eq!(pool.len(), 2);
    ///
    /// pool.release(reservation1);
    /// assert_eq!(pool.len(), 1);
    /// # pool.release(reservation2);
    /// ```
    #[must_use]
    pub fn len(&self) -> usize {
        self.slabs.iter().map(MemorySlab::len).sum()
    }

    /// The number of memory blocks the pool can accommodate without additional resource allocation.
    ///
    /// This is the total capacity, including any existing reservations. The capacity may grow
    /// automatically when [`reserve()`] is called and no space is available.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    ///
    /// use memory_slab::MemoryPool;
    ///
    /// let layout = Layout::new::<u8>();
    /// let mut pool = MemoryPool::new(layout);
    ///
    /// // New pool starts with zero capacity
    /// assert_eq!(pool.capacity(), 0);
    ///
    /// // Reserving memory may increase capacity
    /// let reservation = pool.reserve();
    /// assert!(pool.capacity() > 0);
    /// assert!(pool.capacity() >= pool.len());
    /// # pool.release(reservation);
    /// ```
    ///
    /// [`reserve()`]: Self::reserve
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.slabs
            .len()
            .checked_mul(self.slab_capacity.get())
            .expect("capacity calculation cannot overflow for reasonable slab counts")
    }

    /// Whether the pool has no reservations.
    ///
    /// An empty pool may still be holding unused memory capacity.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    ///
    /// use memory_slab::MemoryPool;
    ///
    /// let layout = Layout::new::<u16>();
    /// let mut pool = MemoryPool::new(layout);
    ///
    /// assert!(pool.is_empty());
    ///
    /// let reservation = pool.reserve();
    /// assert!(!pool.is_empty());
    ///
    /// pool.release(reservation);
    /// assert!(pool.is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slabs.iter().all(MemorySlab::is_empty)
    }

    /// Reserves memory in the pool and returns a reservation that acts as both the key and pointer.
    ///
    /// The returned [`PoolReservation`] provides direct access to the memory via [`PoolReservation::ptr()`]
    /// and must be returned to the pool via [`release()`] to free the memory.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    ///
    /// use memory_slab::MemoryPool;
    ///
    /// let layout = Layout::new::<u64>();
    /// let mut pool = MemoryPool::new(layout);
    ///
    /// // Reserve memory
    /// let reservation = pool.reserve();
    ///
    /// // Write data to the reserved memory
    /// unsafe {
    ///     // SAFETY: The pointer is valid and aligned for u64, and we own the memory.
    ///     reservation
    ///         .ptr()
    ///         .cast::<u64>()
    ///         .as_ptr()
    ///         .write(0xDEADBEEF_CAFEBABE);
    /// }
    ///
    /// // Read data back
    /// let value = unsafe {
    ///     // SAFETY: The pointer is valid and the memory was just initialized.
    ///     reservation.ptr().cast::<u64>().as_ptr().read()
    /// };
    /// assert_eq!(value, 0xDEADBEEF_CAFEBABE);
    ///
    /// // Must release the reservation to free the memory
    /// pool.release(reservation);
    /// ```
    ///
    /// [`release()`]: Self::release
    #[must_use]
    pub fn reserve(&mut self) -> PoolReservation {
        let slab_index = self.index_of_slab_with_vacant_slot();
        let slab = self
            .slabs
            .get_mut(slab_index)
            .expect("we just verified that there is a slab with a vacant slot at this index");

        // We invalidate the "slab with vacant slot" cache here if this is the last vacant slot.
        let predicted_slab_filled_slots = slab
            .len()
            .checked_add(1)
            .expect("we cannot overflow because there is at least one free slot, so it means there must be room to increment");

        if predicted_slab_filled_slots == self.slab_capacity.get() {
            self.slab_with_vacant_slot_index = None;
        }

        let reservation = slab.reserve();
        let coordinates = MemoryBlockCoordinates::from_parts(
            slab_index,
            reservation.index(),
            self.slab_capacity.get(),
        );

        PoolReservation {
            coordinates,
            ptr: reservation.ptr(),
        }
    }

    /// Releases memory previously reserved.
    ///
    /// The [`PoolReservation`] is consumed by this operation and cannot be used afterward.
    /// The memory becomes available for future reservations.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    ///
    /// use memory_slab::MemoryPool;
    ///
    /// let layout = Layout::new::<i32>();
    /// let mut pool = MemoryPool::new(layout);
    ///
    /// let reservation = pool.reserve();
    /// assert_eq!(pool.len(), 1);
    ///
    /// // Release the reservation
    /// pool.release(reservation);
    /// assert_eq!(pool.len(), 0);
    /// assert!(pool.is_empty());
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the reservation is not associated with a memory block.
    pub fn release(&mut self, reservation: PoolReservation) {
        let coordinates = reservation.coordinates;

        let Some(slab) = self.slabs.get_mut(coordinates.slab_index) else {
            panic!("reservation was not associated with a memory block in the pool")
        };

        slab.release(coordinates.index_in_slab);

        // There is now a vacant slot in this slab! We may want to remember this for fast reservations.
        // We try to remember the lowest index of a slab with a vacant slot, so we
        // fill the collection from the start (to enable easier shrinking later).
        if self
            .slab_with_vacant_slot_index
            .is_none_or(|current| current > coordinates.slab_index)
        {
            self.slab_with_vacant_slot_index = Some(coordinates.slab_index);
        }
    }

    #[must_use]
    fn index_of_slab_with_vacant_slot(&mut self) -> usize {
        if let Some(index) = self.slab_with_vacant_slot_index {
            // If we have this cached, we return it immediately.
            // This is a performance optimization to avoid scanning the entire collection.
            return index;
        }

        // We lookup the first slab with some free space, filling the collection from the start.
        let index = if let Some((index, _)) = self
            .slabs
            .iter()
            .enumerate()
            .find(|(_, slab)| !slab.is_full())
        {
            index
        } else {
            // All slabs are full, so we need to expand capacity.
            self.slabs
                .push(MemorySlab::new(self.item_layout, self.slab_capacity));

            self.slabs
                .len()
                .checked_sub(1)
                .expect("we just pushed a slab, so this cannot overflow because len >= 1")
        };

        // We update the cache. The caller is responsible for invalidating this when needed.
        self.slab_with_vacant_slot_index = Some(index);
        index
    }

    #[cfg_attr(test, mutants::skip)] // This is essentially test logic, mutation is meaningless.
    #[cfg(debug_assertions)]
    #[expect(dead_code, reason = "we will probably use it later")]
    pub(crate) fn integrity_check(&self) {
        for slab in &self.slabs {
            slab.integrity_check();
        }
    }
}

#[derive(Debug)]
struct MemoryBlockCoordinates {
    slab_index: usize,
    index_in_slab: usize,
}

impl MemoryBlockCoordinates {
    #[must_use]
    fn from_parts(slab: usize, index_in_slab: usize, _slab_capacity: usize) -> Self {
        Self {
            slab_index: slab,
            index_in_slab,
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::undocumented_unsafe_blocks,
    clippy::multiple_unsafe_ops_per_block,
    clippy::items_after_statements,
    clippy::indexing_slicing,
    clippy::needless_range_loop,
    reason = "test code doesn't need the same safety rigor as production code"
)]
mod tests {
    use std::alloc::Layout;

    use super::*;
    #[test]
    fn smoke_test() {
        let layout = Layout::new::<u32>();
        let mut pool = MemoryPool::new(layout);

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());

        let reservation_a = pool.reserve();
        let reservation_b = pool.reserve();
        let reservation_c = pool.reserve();

        assert_eq!(pool.len(), 3);
        assert!(!pool.is_empty());
        assert!(pool.capacity() >= 3);

        // Write some values
        unsafe {
            reservation_a.ptr().cast::<u32>().as_ptr().write(42);
            reservation_b.ptr().cast::<u32>().as_ptr().write(43);
            reservation_c.ptr().cast::<u32>().as_ptr().write(44);
        }

        // Read them back via reservation pointer
        unsafe {
            assert_eq!(reservation_a.ptr().cast::<u32>().as_ptr().read(), 42);
            assert_eq!(reservation_b.ptr().cast::<u32>().as_ptr().read(), 43);
            assert_eq!(reservation_c.ptr().cast::<u32>().as_ptr().read(), 44);
        }

        pool.release(reservation_b);

        let reservation_d = pool.reserve();

        unsafe {
            reservation_d.ptr().cast::<u32>().as_ptr().write(45);
            assert_eq!(reservation_a.ptr().cast::<u32>().as_ptr().read(), 42);
            assert_eq!(reservation_c.ptr().cast::<u32>().as_ptr().read(), 44);
            assert_eq!(reservation_d.ptr().cast::<u32>().as_ptr().read(), 45);
        }
    }

    #[test]
    #[should_panic]
    fn release_nonexistent_panics() {
        let layout = Layout::new::<u32>();
        let mut pool = MemoryPool::new(layout);

        // Create a fake reservation with invalid coordinates
        let fake_reservation = PoolReservation {
            coordinates: MemoryBlockCoordinates {
                slab_index: 0,
                index_in_slab: 0,
            },
            ptr: NonNull::dangling(),
        };
        pool.release(fake_reservation);
    }
    #[test]
    fn reservater_works() {
        let layout = Layout::new::<u64>();
        let mut pool = MemoryPool::new(layout);

        let reservation = pool.reserve();

        unsafe {
            reservation
                .ptr()
                .cast::<u64>()
                .as_ptr()
                .write(0x1234567890ABCDEF);
            assert_eq!(
                reservation.ptr().cast::<u64>().as_ptr().read(),
                0x1234567890ABCDEF
            );
        }
    }
    #[test]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "test uses small values that fit in u32"
    )]
    fn multi_slab_growth() {
        let layout = Layout::new::<u32>();
        let mut pool = MemoryPool::new(layout);

        // Reserve more items than a single slab can hold to test growth
        let mut reservations = Vec::new();
        for i in 0..200 {
            let reservation = pool.reserve();
            unsafe {
                reservation.ptr().cast::<u32>().as_ptr().write(i);
            }
            reservations.push(reservation);
        }

        assert_eq!(pool.len(), 200);
        assert!(pool.capacity() >= 200);

        // Verify all values are still accessible
        for (i, reservation) in reservations.iter().enumerate() {
            unsafe {
                assert_eq!(reservation.ptr().cast::<u32>().as_ptr().read(), i as u32);
            }
        }
    }
    #[test]
    fn different_layouts() {
        // Test with different sized types
        let layout_u64 = Layout::new::<u64>();
        let mut pool_u64 = MemoryPool::new(layout_u64);
        let reservation = pool_u64.reserve();
        unsafe {
            reservation
                .ptr()
                .cast::<u64>()
                .as_ptr()
                .write(0x1234567890ABCDEF);
            assert_eq!(
                reservation.ptr().cast::<u64>().as_ptr().read(),
                0x1234567890ABCDEF
            );
        }

        // Test with larger struct
        #[repr(C)]
        struct LargeStruct {
            a: u64,
            b: u64,
            c: u64,
            d: u64,
        }

        let layout_large = Layout::new::<LargeStruct>();
        let mut pool_large = MemoryPool::new(layout_large);

        let reservation = pool_large.reserve();
        unsafe {
            reservation
                .ptr()
                .cast::<LargeStruct>()
                .as_ptr()
                .write(LargeStruct {
                    a: 1,
                    b: 2,
                    c: 3,
                    d: 4,
                });
            let value = reservation.ptr().cast::<LargeStruct>().as_ptr().read();
            assert_eq!(value.a, 1);
            assert_eq!(value.b, 2);
            assert_eq!(value.c, 3);
            assert_eq!(value.d, 4);
        }
    }

    #[test]
    #[should_panic]
    fn zero_size_layout_is_panic() {
        let layout = Layout::from_size_align(0, 1).unwrap();
        drop(MemoryPool::new(layout));
    }
    #[test]
    fn stress_test_repeated_reserve_release() {
        let layout = Layout::new::<usize>();
        let mut pool = MemoryPool::new(layout);

        // Reserve and release many items to test slab management
        for iteration in 0..10 {
            let mut reservations = Vec::new();
            for i in 0..50 {
                let reservation = pool.reserve();
                unsafe {
                    reservation
                        .ptr()
                        .cast::<usize>()
                        .as_ptr()
                        .write(iteration * 100 + i);
                }
                reservations.push(reservation);
            }

            // Release every other item
            let mut remaining_reservations = Vec::new();
            for (index, reservation) in reservations.into_iter().enumerate() {
                if index % 2 == 0 {
                    pool.release(reservation);
                } else {
                    remaining_reservations.push(reservation);
                }
            }

            // Verify remaining items
            for (index, reservation) in remaining_reservations.iter().enumerate() {
                let expected_value = iteration * 100 + (index * 2 + 1); // Odd indices
                unsafe {
                    assert_eq!(
                        reservation.ptr().cast::<usize>().as_ptr().read(),
                        expected_value
                    );
                }
            }

            // Release remaining items
            for reservation in remaining_reservations {
                pool.release(reservation);
            }
        }

        assert!(pool.is_empty());
    }
}

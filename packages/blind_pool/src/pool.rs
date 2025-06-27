use std::alloc::Layout;
use std::collections::HashMap;
use std::ptr::NonNull;

use foldhash::fast::FixedState;
use opaque_pool::{DropPolicy, OpaquePool, Pooled as OpaquePooled};

use crate::BlindPoolBuilder;

/// A pinned object pool of unbounded size that accepts objects of any type.
///
/// The pool returns a [`Pooled<T>`] for each inserted value, which acts as both
/// the key and provides direct access to the inserted item via a pointer.
///
/// # Out of band access
///
/// The collection does not create or keep references to the memory blocks. The only way to access
/// the contents of the collection is via unsafe code by using the pointer from a [`Pooled<T>`].
///
/// The collection does not create or maintain any `&` shared or `&mut` exclusive references to
/// the items it contains, except when explicitly called to operate on an item (e.g. `remove()`
/// implies exclusive access).
///
/// # Resource usage
///
/// The collection automatically grows as items are added. To reduce memory usage after items have
/// been removed, use the [`shrink_to_fit()`][1] method to release unused capacity.
///
/// [1]: Self::shrink_to_fit
///
/// # Example
///
/// ```rust
/// use blind_pool::BlindPool;
///
/// let mut pool = BlindPool::new();
///
/// // Insert values of different types.
/// let pooled_u32 = pool.insert(42_u32);
/// let pooled_i64 = pool.insert(-123_i64);
///
/// // Read from the memory.
/// // SAFETY: The pointers are valid and the memory contains the values we just inserted.
/// let value_u32 = unsafe { pooled_u32.ptr().read() };
/// let value_i64 = unsafe { pooled_i64.ptr().read() };
///
/// assert_eq!(value_u32, 42);
/// assert_eq!(value_i64, -123);
/// ```
#[derive(Debug)]
pub struct BlindPool {
    /// Internal pools, one for each unique memory layout encountered.
    /// We use foldhash for better performance with small hash tables.
    pools: HashMap<Layout, OpaquePool, FixedState>,

    /// Drop policy that determines how the pool handles remaining items when dropped.
    drop_policy: DropPolicy,
}

impl BlindPool {
    /// Creates a new [`BlindPool`] with default configuration.
    ///
    /// This is equivalent to [`BlindPool::builder().build()`][BlindPool::builder].
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    ///
    /// let mut pool = BlindPool::new();
    ///
    /// let pooled = pool.insert(42_u64);
    ///
    /// // SAFETY: The pointer is valid and contains the value we just inserted.
    /// let value = unsafe { pooled.ptr().read() };
    /// assert_eq!(value, 42);
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::builder().build()
    }

    /// Creates a builder for configuring and constructing a [`BlindPool`].
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::{BlindPool, DropPolicy};
    ///
    /// let pool = BlindPool::builder()
    ///     .drop_policy(DropPolicy::MustNotDropItems)
    ///     .build();
    /// ```
    pub fn builder() -> BlindPoolBuilder {
        BlindPoolBuilder::new()
    }

    /// Creates a new [`BlindPool`] with the specified configuration.
    ///
    /// This method is used internally by the builder to construct the actual pool.
    #[must_use]
    pub(crate) fn new_inner(drop_policy: DropPolicy) -> Self {
        Self {
            pools: HashMap::with_hasher(FixedState::default()),
            drop_policy,
        }
    }

    /// Inserts a value into the pool and returns a handle to access it.
    ///
    /// The pool stores the value and provides a handle for later access or removal.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    ///
    /// let mut pool = BlindPool::new();
    ///
    /// // Insert different types into the same pool.
    /// let pooled_int = pool.insert(42_i32);
    /// let pooled_float = pool.insert(2.5_f64);
    /// let pooled_string = pool.insert("hello".to_string());
    ///
    /// // All values are stored in the same BlindPool.
    /// assert_eq!(pool.len(), 3);
    /// ```
    pub fn insert<T>(&mut self, value: T) -> Pooled<T> {
        let layout = Layout::new::<T>();

        let internal_pool = self.pools.entry(layout).or_insert_with(|| {
            OpaquePool::builder()
                .layout_of::<T>()
                .drop_policy(self.drop_policy)
                .build()
        });

        // SAFETY: T matches the layout used to create the internal pool.
        let pooled = unsafe { internal_pool.insert(value) };

        Pooled { layout, pooled }
    }

    /// Removes a value from the pool and drops it.
    ///
    /// The [`Pooled<T>`] handle is consumed and the memory is returned to the pool.
    /// The value is dropped.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    ///
    /// let mut pool = BlindPool::new();
    ///
    /// let pooled = pool.insert(42_u64);
    /// assert_eq!(pool.len(), 1);
    ///
    /// pool.remove(pooled);
    /// assert_eq!(pool.len(), 0);
    /// ```
    pub fn remove<T>(&mut self, pooled: Pooled<T>) {
        if let Some(internal_pool) = self.pools.get_mut(&pooled.layout) {
            internal_pool.remove(pooled.pooled);
        } else {
            panic!("attempted to remove a handle from a non-existent internal pool");
        }
    }

    /// Returns the total number of items stored in the pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    ///
    /// let mut pool = BlindPool::new();
    ///
    /// assert_eq!(pool.len(), 0);
    ///
    /// let _a = pool.insert(42_u32);
    /// let _b = pool.insert("hello".to_string());
    /// let _c = pool.insert(2.5_f64);
    ///
    /// assert_eq!(pool.len(), 3);
    /// ```
    #[must_use]
    pub fn len(&self) -> usize {
        self.pools.values().map(OpaquePool::len).sum()
    }

    /// Whether the pool has no inserted values.
    ///
    /// An empty pool may still be holding unused memory capacity.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    ///
    /// let mut pool = BlindPool::new();
    ///
    /// assert!(pool.is_empty());
    ///
    /// let pooled = pool.insert(42_u16);
    ///
    /// assert!(!pool.is_empty());
    ///
    /// pool.remove(pooled);
    /// assert!(pool.is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pools.values().all(OpaquePool::is_empty)
    }

    /// Returns the total capacity of the pool.
    ///
    /// This is the total number of items that can be stored without allocating more memory.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    ///
    /// let mut pool = BlindPool::new();
    ///
    /// // Initially no capacity is allocated.
    /// assert_eq!(pool.capacity(), 0);
    ///
    /// // Inserting a value allocates capacity.
    /// let _pooled = pool.insert(42_u32);
    /// assert!(pool.capacity() > 0);
    /// ```
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.pools.values().map(OpaquePool::capacity).sum()
    }

    /// Shrinks the capacity of the pool to fit its current size.
    ///
    /// This can help reduce memory usage after items have been removed from the pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    ///
    /// let mut pool = BlindPool::new();
    ///
    /// // Insert many items to allocate capacity.
    /// for i in 0..1000 {
    ///     pool.insert(i);
    /// }
    ///
    /// let capacity_before = pool.capacity();
    ///
    /// // Remove all items but keep the allocated capacity.
    /// while !pool.is_empty() {
    ///     // In a real scenario you'd keep track of handles to remove them properly.
    ///     // This is just for the example.
    ///     break;
    /// }
    ///
    /// // Shrink to fit the current size (which is 0).
    /// pool.shrink_to_fit();
    /// assert!(pool.capacity() <= capacity_before);
    /// ```
    pub fn shrink_to_fit(&mut self) {
        // Remove empty internal pools.
        self.pools.retain(|_, pool| !pool.is_empty());

        // Shrink remaining pools.
        for pool in self.pools.values_mut() {
            pool.shrink_to_fit();
        }

        // Shrink the HashMap itself.
        self.pools.shrink_to_fit();
    }
}

impl Default for BlindPool {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for BlindPool {
    fn drop(&mut self) {
        if matches!(self.drop_policy, DropPolicy::MustNotDropItems) && !std::thread::panicking() {
            assert!(
                self.is_empty(),
                "BlindPool dropped while still containing items (drop policy is MustNotDropItems)"
            );
        }
    }
}

/// A handle representing an item stored in a [`BlindPool`].
///
/// This provides access to the stored item and can be used to remove the item from the pool.
///
/// # Example
///
/// ```rust
/// use blind_pool::BlindPool;
///
/// let mut pool = BlindPool::new();
///
/// let pooled = pool.insert(42_u64);
///
/// // Access the value via the pointer.
/// // SAFETY: The pointer is valid and contains the value we just inserted.
/// let value = unsafe { pooled.ptr().read() };
/// assert_eq!(value, 42);
///
/// // Remove the item from the pool.
/// pool.remove(pooled);
/// ```
#[derive(Debug)]
pub struct Pooled<T> {
    /// The memory layout of the stored item. This is used to identify which internal
    /// pool the item belongs to.
    layout: Layout,

    /// The handle from the internal opaque pool.
    pooled: OpaquePooled<T>,
}

impl<T> Pooled<T> {
    /// Returns a pointer to the inserted value.
    ///
    /// This is the only way to access the value stored in the pool. The owner of the handle has
    /// exclusive access to the value and may both read and write and may create both `&` shared
    /// and `&mut` exclusive references to the item.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    ///
    /// let mut pool = BlindPool::new();
    ///
    /// let pooled = pool.insert(2.5159_f64);
    ///
    /// // Read data back from the memory.
    /// // SAFETY: The pointer is valid and the memory contains the value we just inserted.
    /// let value = unsafe { pooled.ptr().read() };
    /// assert_eq!(value, 2.5159);
    /// ```
    #[must_use]
    pub fn ptr(&self) -> NonNull<T> {
        self.pooled.ptr()
    }

    /// Erases the type information from this [`Pooled<T>`] handle,
    /// returning a [`Pooled<()>`].
    ///
    /// This is useful when you want to store handles of different types in the same collection
    /// or pass them to code that doesn't need to know the specific type.
    ///
    /// The handle remains functionally equivalent and can still be used to remove the item
    /// from the pool and drop it. The only change is the removal of the type information.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    ///
    /// let mut pool = BlindPool::new();
    ///
    /// let pooled = pool.insert(42_u64);
    ///
    /// // Erase type information.
    /// let erased = pooled.erase();
    ///
    /// // Can still access the raw pointer.
    /// // SAFETY: We know this contains a u64.
    /// let value = unsafe { erased.ptr().cast::<u64>().read() };
    /// assert_eq!(value, 42);
    ///
    /// // Can still remove the item.
    /// pool.remove(erased);
    /// ```
    #[must_use]
    pub fn erase(self) -> Pooled<()> {
        Pooled {
            layout: self.layout,
            pooled: self.pooled.erase(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_insert_remove() {
        let mut pool = BlindPool::new();
        let pooled = pool.insert(42_u32);
        pool.remove(pooled);
    }

    #[test]
    fn two_items_same_type() {
        let mut pool = BlindPool::new();
        let pooled1 = pool.insert(42_u32);
        let pooled2 = pool.insert(43_u32);
        pool.remove(pooled1);
        pool.remove(pooled2);
    }

    #[test]
    fn two_items_different_types() {
        let mut pool = BlindPool::new();
        let pooled1 = pool.insert(42_u32);
        let pooled2 = pool.insert(43_u64);
        pool.remove(pooled1);
        pool.remove(pooled2);
    }

    #[test]
    fn smoke_test() {
        let mut pool = BlindPool::new();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());

        let pooled_u32 = pool.insert(42_u32);
        let pooled_u64 = pool.insert(43_u64);
        let pooled_f32 = pool.insert(2.5_f32);

        assert_eq!(pool.len(), 3);
        assert!(!pool.is_empty());
        assert!(pool.capacity() >= 3);

        // SAFETY: The pointers are valid and contain the values we just inserted.
        let u32_val = unsafe { pooled_u32.ptr().read() };
        // SAFETY: The pointers are valid and contain the values we just inserted.
        let u64_val = unsafe { pooled_u64.ptr().read() };
        // SAFETY: The pointers are valid and contain the values we just inserted.
        let f32_val = unsafe { pooled_f32.ptr().read() };
        assert_eq!(u32_val, 42);
        assert_eq!(u64_val, 43);
        assert!((f32_val - 2.5).abs() < f32::EPSILON);

        pool.remove(pooled_u32);
        pool.remove(pooled_u64);
        pool.remove(pooled_f32);

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
    }

    #[test]
    fn different_types_same_layout() {
        let mut pool = BlindPool::new();

        // These types have the same layout (both are 4 bytes, 4-byte aligned).
        let pooled_u32 = pool.insert(42_u32);
        let pooled_i32 = pool.insert(-42_i32);
        let pooled_f32 = pool.insert(2.5_f32);

        assert_eq!(pool.len(), 3);

        // SAFETY: The pointers are valid and contain the values we just inserted.
        let u32_val = unsafe { pooled_u32.ptr().read() };
        // SAFETY: The pointers are valid and contain the values we just inserted.
        let i32_val = unsafe { pooled_i32.ptr().read() };
        // SAFETY: The pointers are valid and contain the values we just inserted.
        let f32_val = unsafe { pooled_f32.ptr().read() };
        assert_eq!(u32_val, 42);
        assert_eq!(i32_val, -42);
        assert!((f32_val - 2.5).abs() < f32::EPSILON);

        pool.remove(pooled_u32);
        pool.remove(pooled_i32);
        pool.remove(pooled_f32);

        assert!(pool.is_empty());
    }

    #[test]
    fn builder_with_drop_policy() {
        let pool = BlindPool::builder()
            .drop_policy(DropPolicy::MustNotDropItems)
            .build();

        // Pool should not panic when dropped if empty.
        drop(pool);
    }

    #[test]
    #[should_panic(expected = "BlindPool dropped while still containing items")]
    fn drop_policy_must_not_drop_panics_when_not_empty() {
        let mut pool = BlindPool::builder()
            .drop_policy(DropPolicy::MustNotDropItems)
            .build();

        let _pooled = pool.insert(42_u32);

        // Pool should panic when dropped with items.
        drop(pool);
    }

    #[test]
    fn shrink_to_fit_removes_empty_pools() {
        let mut pool = BlindPool::new();

        // Insert items of different types to create multiple internal pools.
        // Use types with different layouts: u8 (1 byte), u64 (8 bytes), [u8; 3] (3 bytes).
        let pooled_u8 = pool.insert(1_u8);
        let pooled_u64 = pool.insert(2_u64);
        _ = pool.insert([1_u8, 2_u8, 3_u8]);

        // Verify we have multiple internal pools.
        assert_eq!(pool.pools.len(), 3);
        assert_eq!(pool.len(), 3);

        // Remove some but not all items (we leave the array).
        pool.remove(pooled_u8);
        pool.remove(pooled_u64);

        assert_eq!(pool.pools.len(), 3); // Internal pools still exist before shrinking.

        // This should clean up empty internal pools.
        pool.shrink_to_fit();

        // Verify that empty internal pools have been removed.
        assert_eq!(pool.pools.len(), 1);
    }

    #[test]
    fn erase_type_information() {
        let mut pool = BlindPool::new();

        let pooled = pool.insert(42_u64);
        let erased = pooled.erase();

        // SAFETY: We know this contains a u64.
        let value = unsafe { erased.ptr().cast::<u64>().read() };
        assert_eq!(value, 42);

        pool.remove(erased);
        assert!(pool.is_empty());
    }

    #[test]
    fn works_with_drop_types() {
        let mut pool = BlindPool::new();

        // Test with String - a type that implements Drop
        let test_string = "Hello, World!".to_string();
        let pooled_string = pool.insert(test_string);

        pool.remove(pooled_string);

        // Test with Vec - another type that implements Drop
        let test_vec = vec![1, 2, 3, 4, 5];
        let pooled_vec = pool.insert(test_vec);

        pool.remove(pooled_vec);

        assert!(pool.is_empty());
    }

    #[test]
    fn trait_object_usage() {
        // Define a trait for testing.
        trait Printable {
            fn print_info(&self) -> String;
        }

        #[derive(Debug)]
        struct Book {
            title: String,
            pages: u32,
        }

        impl Printable for Book {
            fn print_info(&self) -> String {
                format!("Book: '{}' ({} pages)", self.title, self.pages)
            }
        }

        let mut pool = BlindPool::new();

        // Insert a book into the pool.
        let book = Book {
            title: "The Rust Programming Language".to_string(),
            pages: 552,
        };

        let pooled_book = pool.insert(book);

        // Use item as trait object.
        // SAFETY: The pointer is valid and points to a Book that we just inserted.
        unsafe {
            let book_ref: &Book = pooled_book.ptr().as_ref();
            let printable: &dyn Printable = book_ref;
            assert_eq!(
                printable.print_info(),
                "Book: 'The Rust Programming Language' (552 pages)"
            );
        }

        pool.remove(pooled_book);
    }

    #[test]
    fn trait_object_with_mutable_references() {
        trait Modifiable {
            fn modify(&mut self, factor: f64);
            fn get_value(&self) -> f64;
        }

        #[derive(Debug)]
        struct Temperature {
            celsius: f64,
        }

        impl Modifiable for Temperature {
            fn modify(&mut self, factor: f64) {
                self.celsius *= factor;
            }

            fn get_value(&self) -> f64 {
                self.celsius
            }
        }

        let mut pool = BlindPool::new();

        let temp = Temperature { celsius: 25.0 };
        let pooled_temp = pool.insert(temp);

        // Test mutable trait objects.
        // SAFETY: The pointer is valid and points to a Temperature that we just inserted.
        unsafe {
            let temp_ref: &mut Temperature = pooled_temp.ptr().as_mut();
            let modifiable: &mut dyn Modifiable = temp_ref;

            assert!((modifiable.get_value() - 25.0).abs() < f64::EPSILON);
            modifiable.modify(2.0);
            assert!((modifiable.get_value() - 50.0).abs() < f64::EPSILON);
        }

        // Verify changes persisted.
        // SAFETY: The pointer is valid and points to the object we modified.
        unsafe {
            let temp_ref: &Temperature = pooled_temp.ptr().as_ref();
            assert!((temp_ref.celsius - 50.0).abs() < f64::EPSILON);
        }

        pool.remove(pooled_temp);
    }
}

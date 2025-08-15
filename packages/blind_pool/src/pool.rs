use std::sync::{Arc, Mutex};

use crate::constants::ERR_POISONED_LOCK;
use crate::{BlindPoolBuilder, Pooled, RawBlindPool, RawPooled};

/// A thread-safe wrapper around [`RawBlindPool`] that provides automatic resource management
/// and reference counting.
///
/// This is the main pool type that provides automatic resource management through reference
/// counting. For manual resource management, use [`RawBlindPool`] instead.
///
/// This type acts as a cloneable handle to a shared pool instance. Multiple handles
/// can exist simultaneously, and the underlying pool remains alive as long as at least one
/// handle exists.
///
/// Items inserted into the pool are automatically removed when all references to them are
/// dropped, eliminating the need for manual resource management.
///
/// # Thread Safety
///
/// This type is thread-safe and can be safely shared across multiple threads.
///
/// # Example
///
/// ```rust
/// use std::thread;
/// use blind_pool::BlindPool;
///
/// let pool = BlindPool::new();
///
/// // Clone the pool handle to share across threads.
/// let pool_clone = pool.clone();
///
/// let handle = thread::spawn(move || {
///     let item_handle = pool_clone.insert(42_u64);
///     *item_handle
/// });
///
/// let value = handle.join().unwrap();
/// assert_eq!(value, 42);
/// ```
#[derive(Clone, Debug)]
pub struct BlindPool {
    /// The shared pool instance protected by a mutex for thread safety.
    inner: Arc<Mutex<RawBlindPool>>,
}

impl From<RawBlindPool> for BlindPool {
    /// Creates a new [`BlindPool`] from an existing raw pool.
    ///
    /// The provided pool is consumed and wrapped in thread-safe reference counting.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::{BlindPool, RawBlindPool, DropPolicy};
    ///
    /// // Create a configured raw pool.
    /// let raw_pool = RawBlindPool::builder()
    ///     .drop_policy(DropPolicy::MayDropItems)
    ///     .build_raw();
    ///
    /// // Convert to pool.
    /// let pool = BlindPool::from(raw_pool);
    /// ```
    fn from(pool: RawBlindPool) -> Self {
        Self {
            inner: Arc::new(Mutex::new(pool)),
        }
    }
}

impl BlindPool {
    /// Creates a new [`BlindPool`] with default configuration.
    ///
    /// This is the equivalent of creating a raw pool and wrapping it in management.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    ///
    /// let pool = BlindPool::new();
    ///
    /// let u32_handle = pool.insert(42_u32);
    /// let string_handle = pool.insert("hello".to_string());
    ///
    /// // Access values through dereferencing.
    /// assert_eq!(*u32_handle, 42);
    /// assert_eq!(*string_handle, "hello");
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::from(RawBlindPool::new())
    }

    /// Returns a builder for creating a [`BlindPool`] with custom configuration.
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

    /// Inserts a value into the pool and returns a handle to access it.
    ///
    /// The returned handle automatically manages the lifetime of the inserted value.
    /// When all handles to the value are dropped, the value is automatically removed
    /// from the pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    ///
    /// let pool = BlindPool::new();
    ///
    /// let u32_handle = pool.insert(42_u32);
    /// let string_handle = pool.insert("hello".to_string());
    ///
    /// // Access values through dereferencing.
    /// assert_eq!(*u32_handle, 42);
    /// assert_eq!(*string_handle, "hello");
    /// ```
    pub fn insert<T>(&self, value: T) -> Pooled<T> {
        let pooled = {
            let mut pool = self.inner.lock().expect(ERR_POISONED_LOCK);
            pool.insert(value)
        };

        Pooled::new(pooled, self.clone())
    }

    /// Returns the total number of items currently stored in the pool.
    ///
    /// This operation may block if another thread is currently accessing the pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    ///
    /// let pool = BlindPool::new();
    ///
    /// assert_eq!(pool.len(), 0);
    ///
    /// let _item1 = pool.insert(42_u32);
    /// let _item2 = pool.insert("hello".to_string());
    ///
    /// assert_eq!(pool.len(), 2);
    /// ```
    #[must_use]
    pub fn len(&self) -> usize {
        let pool = self.inner.lock().expect(ERR_POISONED_LOCK);
        pool.len()
    }

    /// Returns whether the pool has no inserted values.
    ///
    /// This operation may block if another thread is currently accessing the pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    ///
    /// let pool = BlindPool::new();
    ///
    /// assert!(pool.is_empty());
    ///
    /// let item = pool.insert(42_u32);
    /// assert!(!pool.is_empty());
    ///
    /// drop(item);
    /// assert!(pool.is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        let pool = self.inner.lock().expect(ERR_POISONED_LOCK);
        pool.is_empty()
    }

    /// Removes an item from the pool using its handle.
    ///
    /// This is an internal method used by [`Pooled`] when it is dropped.
    /// It should not be called directly by user code.
    pub(crate) fn remove<T>(&self, pooled: RawPooled<T>) {
        let mut pool = self.inner.lock().expect(ERR_POISONED_LOCK);
        pool.remove(pooled);
    }
}

impl Default for BlindPool {
    fn default() -> Self {
        Self::new()
    }
}

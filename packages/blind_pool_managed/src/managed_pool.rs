use std::sync::{Arc, Mutex};

use blind_pool::BlindPool;

use crate::{constants::ERR_POISONED_LOCK, ManagedPooled};

/// A thread-safe wrapper around [`BlindPool`] that provides automatic resource management
/// and reference counting.
///
/// This type acts as a cloneable handle to a shared [`BlindPool`] instance. Multiple handles
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
/// use blind_pool_managed::ManagedBlindPool;
///
/// let pool = ManagedBlindPool::from(BlindPool::new());
///
/// // Clone the pool handle to share across threads.
/// let pool_clone = pool.clone();
///
/// let handle = thread::spawn(move || {
///     let managed_item = pool_clone.insert(42_u64);
///     *managed_item
/// });
///
/// let value = handle.join().unwrap();
/// assert_eq!(value, 42);
/// ```
#[derive(Clone, Debug)]
pub struct ManagedBlindPool {
    /// The shared pool instance protected by a mutex for thread safety.
    inner: Arc<Mutex<BlindPool>>,
}

impl From<BlindPool> for ManagedBlindPool {
    /// Creates a new [`ManagedBlindPool`] from an existing [`BlindPool`].
    ///
    /// The provided pool is consumed and wrapped in thread-safe reference counting.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::{BlindPool, DropPolicy};
    /// use blind_pool_managed::ManagedBlindPool;
    ///
    /// // Create a configured pool.
    /// let pool = BlindPool::builder()
    ///     .drop_policy(DropPolicy::MayDropItems)
    ///     .build();
    ///
    /// // Convert to managed pool.
    /// let managed_pool = ManagedBlindPool::from(pool);
    /// ```
    fn from(pool: BlindPool) -> Self {
        Self {
            inner: Arc::new(Mutex::new(pool)),
        }
    }
}

impl ManagedBlindPool {
    /// Inserts a value into the pool and returns a managed handle to access it.
    ///
    /// The returned handle automatically manages the lifetime of the inserted value.
    /// When all handles to the value are dropped, the value is automatically removed
    /// from the pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    /// use blind_pool_managed::ManagedBlindPool;
    ///
    /// let pool = ManagedBlindPool::from(BlindPool::new());
    ///
    /// let managed_u32 = pool.insert(42_u32);
    /// let managed_string = pool.insert("hello".to_string());
    ///
    /// // Access values through dereferencing.
    /// assert_eq!(*managed_u32, 42);
    /// assert_eq!(*managed_string, "hello");
    /// ```
    pub fn insert<T>(&self, value: T) -> ManagedPooled<T> {
        let pooled = {
            let mut pool = self.inner.lock().expect(ERR_POISONED_LOCK);
            pool.insert(value)
        };

        ManagedPooled::new(pooled, self.clone())
    }

    /// Returns the total number of items currently stored in the pool.
    ///
    /// This operation may block if another thread is currently accessing the pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    /// use blind_pool_managed::ManagedBlindPool;
    ///
    /// let pool = ManagedBlindPool::from(BlindPool::new());
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
    /// use blind_pool_managed::ManagedBlindPool;
    ///
    /// let pool = ManagedBlindPool::from(BlindPool::new());
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
    /// This is an internal method used by [`ManagedPooled`] when it is dropped.
    /// It should not be called directly by user code.
    pub(crate) fn remove<T>(&self, pooled: blind_pool::Pooled<T>) {
        let mut pool = self.inner.lock().expect(ERR_POISONED_LOCK);
        pool.remove(pooled);
    }
}

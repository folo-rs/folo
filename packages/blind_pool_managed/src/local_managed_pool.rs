use std::cell::RefCell;
use std::rc::Rc;

use blind_pool::BlindPool;

use crate::LocalManagedPooled;

/// A single-threaded wrapper around [`BlindPool`] that provides automatic resource management
/// and reference counting.
///
/// This type acts as a cloneable handle to a shared [`BlindPool`] instance. Multiple handles
/// can exist simultaneously, and the underlying pool remains alive as long as at least one
/// handle exists.
///
/// Items inserted into the pool are automatically removed when all references to them are
/// dropped, eliminating the need for manual resource management.
///
/// # Single-threaded Design
///
/// This type is designed for single-threaded use and is neither [`Send`] nor [`Sync`].
/// For multi-threaded scenarios, use [`crate::ManagedBlindPool`] instead.
///
/// # Example
///
/// ```rust
/// use blind_pool::BlindPool;
/// use blind_pool_managed::LocalManagedBlindPool;
///
/// let pool = LocalManagedBlindPool::from(BlindPool::new());
///
/// // Clone the pool handle for use in different parts of the code.
/// let pool_clone = pool.clone();
///
/// let managed_item = pool_clone.insert(42_u64);
/// assert_eq!(*managed_item, 42);
/// ```
#[derive(Clone, Debug)]
pub struct LocalManagedBlindPool {
    /// The shared pool instance protected by a `RefCell` for single-threaded interior mutability.
    inner: Rc<RefCell<BlindPool>>,
}

impl From<BlindPool> for LocalManagedBlindPool {
    /// Creates a new [`LocalManagedBlindPool`] from an existing [`BlindPool`].
    ///
    /// The provided pool is consumed and wrapped in single-threaded reference counting.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::{BlindPool, DropPolicy};
    /// use blind_pool_managed::LocalManagedBlindPool;
    ///
    /// // Create a configured pool.
    /// let pool = BlindPool::builder()
    ///     .drop_policy(DropPolicy::MayDropItems)
    ///     .build();
    ///
    /// // Convert to local managed pool.
    /// let managed_pool = LocalManagedBlindPool::from(pool);
    /// ```
    fn from(pool: BlindPool) -> Self {
        Self {
            inner: Rc::new(RefCell::new(pool)),
        }
    }
}

impl LocalManagedBlindPool {
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
    /// use blind_pool_managed::LocalManagedBlindPool;
    ///
    /// let pool = LocalManagedBlindPool::from(BlindPool::new());
    ///
    /// let managed_u32 = pool.insert(42_u32);
    /// let managed_string = pool.insert("hello".to_string());
    ///
    /// // Access values through dereferencing.
    /// assert_eq!(*managed_u32, 42);
    /// assert_eq!(*managed_string, "hello");
    /// ```
    pub fn insert<T>(&self, value: T) -> LocalManagedPooled<T> {
        let pooled = {
            let mut pool = self.inner.borrow_mut();
            pool.insert(value)
        };

        LocalManagedPooled::new(pooled, self.clone())
    }

    /// Returns the total number of items currently stored in the pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    /// use blind_pool_managed::LocalManagedBlindPool;
    ///
    /// let pool = LocalManagedBlindPool::from(BlindPool::new());
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
        let pool = self.inner.borrow();
        pool.len()
    }

    /// Returns whether the pool has no inserted values.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    /// use blind_pool_managed::LocalManagedBlindPool;
    ///
    /// let pool = LocalManagedBlindPool::from(BlindPool::new());
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
        let pool = self.inner.borrow();
        pool.is_empty()
    }

    /// Removes an item from the pool using its handle.
    ///
    /// This is an internal method used by [`LocalManagedPooled`] when it is dropped.
    /// It should not be called directly by user code.
    pub(crate) fn remove<T>(&self, pooled: blind_pool::Pooled<T>) {
        let mut pool = self.inner.borrow_mut();
        pool.remove(pooled);
    }
}

//! This package provides [`ManagedBlindPool`], a thread-safe wrapper around [`BlindPool`] with
//! automatic resource management and reference counting.
//!
//! It extends [`BlindPool`] with thread-safe, cloneable handles and automatic cleanup of pooled
//! items when they are no longer referenced.
//!
//! # Features
//!
//! - **Thread-safe**: Safe concurrent access from multiple threads
//! - **Cloneable handles**: The pool itself acts as a lightweight handle that can be cloned freely
//! - **Automatic cleanup**: Items are automatically removed from the pool when the last reference is dropped
//! - **Reference counting**: Manages the lifetime of pooled items automatically
//! - **Type safety**: Maintains all the type safety guarantees of the underlying `BlindPool`
//! - **No manual removal**: No need to explicitly remove items - they are cleaned up automatically
//!
//! # Variants
//!
//! This package provides two variants:
//!
//! - **Thread-safe**: [`ManagedBlindPool`] and [`ManagedPooled<T>`] for multi-threaded use
//! - **Single-threaded**: [`LocalManagedBlindPool`] and [`LocalManagedPooled<T>`] for single-threaded use with better performance
//!
//! # Example
//!
//! ```rust
//! use blind_pool::BlindPool;
//! use blind_pool_managed::ManagedBlindPool;
//!
//! // Create a managed pool from a regular BlindPool.
//! let pool = ManagedBlindPool::from(BlindPool::new());
//!
//! // The pool can be cloned and shared across threads.
//! let pool_clone = pool.clone();
//!
//! // Insert values and get managed handles.
//! let managed_u32 = pool.insert(42_u32);
//! let managed_string = pool.insert("hello".to_string());
//!
//! // Access values through dereferencing.
//! assert_eq!(*managed_u32, 42);
//! assert_eq!(*managed_string, "hello");
//!
//! // Values are automatically removed when all references are dropped.
//! drop(managed_u32);
//! drop(managed_string);
//! ```

mod constants;
mod local_managed_pool;
mod local_managed_pooled;
mod managed_pool;
mod managed_pooled;

pub use local_managed_pool::*;
pub use local_managed_pooled::*;
pub use managed_pool::*;
pub use managed_pooled::*;

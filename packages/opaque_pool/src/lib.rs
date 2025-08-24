//! A type-erased, pinned object pool with stable memory addresses and efficient typed access.
//!
//! This crate provides [`OpaquePool`], a dynamically growing pool that can store objects of any
//! type as long as they match a [`std::alloc::Layout`] defined at pool creation. The pool provides
//! stable memory addresses, automatic value dropping, and two handle types for different usage
//! patterns.
//!
//! # Key Features
//!
//! - **Type-erased storage**: Works with any type matching the pool's [`std::alloc::Layout`]
//! - **Stable memory addresses**: Items never move once inserted (always pinned)
//! - **Two handle types**: [`PooledMut<T>`] for exclusive access, [`Pooled<T>`] for shared access
//! - **Safe removal patterns**: [`PooledMut<T>`] prevents double-use
//! - **Automatic value dropping**: Values are properly dropped when removed or pool is dropped
//! - **Dynamic growth**: Pool capacity expands automatically as needed
//! - **Memory efficiency**: High-density slab allocation minimizes overhead
//! - **Trait object support**: Built-in support for casting to trait objects
//! - **Pinning support**: Safe [`std::pin::Pin<&T>`] and [`std::pin::Pin<&mut T>`] access to pooled values
//! - **Flexible drop policies**: Configure behavior when pool is dropped with remaining items
//! - **Thread mobility**: Pool can be moved between threads (but not shared without synchronization)
//!
//! # Handle Types
//!
//! ## [`PooledMut<T>`] - Exclusive Access
//!
//! Returned by insertion methods, provides exclusive ownership and prevents accidental double-use:
//! - Cannot be copied or cloned
//! - Safe removal methods that consume the handle
//! - Can be converted to [`Pooled<T>`] for sharing via [`into_shared()`](PooledMut::into_shared)
//! - Implements [`std::ops::Deref`] and [`std::ops::DerefMut`] for direct value access
//!
//! ## [`Pooled<T>`] - Shared Access
//!
//! Created from [`PooledMut<T>`] or returned by some methods, allows multiple references:
//! - Can be copied and cloned freely
//! - Multiple handles can refer to the same pooled value
//! - Removal requires `unsafe` code to prevent use-after-free
//! - Implements [`std::ops::Deref`] for direct shared value access
//!
//! The pool itself never holds references to its contents, allowing you to control
//! aliasing and ensure Rust's borrowing rules are respected.
//!
//! # Examples
//!
//! ## Basic Usage with Exclusive Handles
//!
//! ```rust
//! use opaque_pool::OpaquePool;
//!
//! // Create a pool for String values
//! let mut pool = OpaquePool::builder().layout_of::<String>().build();
//!
//! // Insert a value and get an exclusive handle
//! // SAFETY: String matches the layout used to create the pool
//! let item = unsafe { pool.insert("Hello, World!".to_string()) };
//!
//! // Access the value safely through Deref
//! assert_eq!(&*item, "Hello, World!");
//! assert_eq!(item.len(), 13);
//!
//! // Remove the value (consumes the handle, preventing reuse)
//! pool.remove_mut(item);
//! ```
//!
//! ## Shared Access Pattern
//!
//! ```rust
//! use opaque_pool::OpaquePool;
//!
//! let mut pool = OpaquePool::builder().layout_of::<String>().build();
//!
//! // SAFETY: String matches the layout used to create the pool
//! let item = unsafe { pool.insert("Shared".to_string()) };
//!
//! // Convert to shared handle for copying
//! let shared = item.into_shared();
//! let shared_copy = shared; // Can copy freely
//!
//! // Access the value
//! assert_eq!(&*shared_copy, "Shared");
//!
//! // Removal requires unsafe (caller ensures no other copies are used)
//! // SAFETY: No other copies of the handle will be used after this call
//! unsafe { pool.remove(&shared_copy) };
//! ```
//!
//! ## Working with Different Types (Same Layout)
//!
//! ```rust
//! use std::alloc::Layout;
//!
//! use opaque_pool::OpaquePool;
//!
//! // Create a pool for u64-sized values
//! let layout = Layout::new::<u64>();
//! let mut pool = OpaquePool::builder().layout(layout).build();
//!
//! // Insert different types with the same layout
//! // SAFETY: u64 matches the pool's layout
//! let num = unsafe { pool.insert(42_u64) };
//! // SAFETY: i64 has the same layout as u64
//! let signed = unsafe { pool.insert(-123_i64) };
//!
//! // Access the values using modern Deref patterns
//! assert_eq!(*num, 42);
//! assert_eq!(*signed, -123);
//!
//! pool.remove_mut(num);
//! pool.remove_mut(signed);
//! ```

mod builder;
mod coordinates;
mod drop_policy;
mod dropper;
mod pool;
mod pooled;
mod pooled_mut;
mod slab;

pub use builder::*;
pub(crate) use coordinates::*;
pub use drop_policy::*;
pub(crate) use dropper::*;
pub use pool::OpaquePool;
pub use pooled::Pooled;
pub use pooled_mut::PooledMut;
pub(crate) use slab::*;

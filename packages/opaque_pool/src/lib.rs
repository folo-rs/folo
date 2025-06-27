//! This crate provides [`OpaquePool`], a dynamically growing pool of objects that is largely
//! ignorant of the type of the items within, as long as they match a [`std::alloc::Layout`]
//! defined at pool creation.
//!
//! It offers stable memory addresses and efficient typed insertion with automatic value dropping.
//!
//! # Type-erased value management
//!
//! The pool works with any type that matches the layout specified at creation time. Values are
//! inserted using the unsafe [`OpaquePool::insert()`] method, which requires the caller to
//! ensure the type matches the pool's layout. The pool automatically handles dropping of values
//! when they are removed.
//!
//! The pool itself does not hold or create any `&` shared or `&mut` exclusive references to its
//! contents, allowing the caller to decide who and when can obtain a reference to the inserted
//! values. The caller is responsible for ensuring that Rust aliasing rules are respected.
//!
//! # Features
//!
//! - **Type-erased memory management**: Works with any memory layout via [`std::alloc::Layout`].
//! - **Stable addresses**: Memory addresses remain valid until explicitly removed.
//! - **Automatic dropping**: Values are properly dropped when removed from the pool.
//! - **Layout safety**: Debug builds verify that inserted types match the pool's layout.
//! - **Dynamic growth**: Pool capacity grows automatically as needed.
//! - **Efficient allocation**: Uses high density slabs to minimize allocation overhead.
//! - **Stable Rust**: No unstable Rust features required.
//! - **Optional leak detection**: Pool can be configured to panic on drop if values are still present.
//!
//! # Example
//!
//! ```rust
//! use std::alloc::Layout;
//!
//! use opaque_pool::OpaquePool;
//!
//! // Create a pool for storing values that match the layout of `u64`.
//! let layout = Layout::new::<u64>();
//! let mut pool = OpaquePool::builder().layout(layout).build();
//!
//! // Insert values into the pool.
//! // SAFETY: The layout of u64 matches the pool's item layout.
//! let pooled1 = unsafe { pool.insert(42_u64) };
//! // SAFETY: The layout of u64 matches the pool's item layout.
//! let pooled2 = unsafe { pool.insert(123_u64) };
//!
//! // Read data back from the pooled items.
//! let value1 = unsafe {
//!     // SAFETY: The pointer is valid and the value was just inserted.
//!     pooled1.ptr().read()
//! };
//!
//! let value2 = unsafe {
//!     // SAFETY: The pointer is valid and the value was just inserted.
//!     pooled2.ptr().read()
//! };
//!
//! assert_eq!(value1, 42);
//! assert_eq!(value2, 123);
//!
//! // Remove values from the pool. The values are automatically dropped.
//! // The pool treats all items as pinned, so the removed value is not returned
//! // because that would violate the promise of pinning the objects.
//! pool.remove(pooled1);
//! pool.remove(pooled2);
//!
//! assert!(pool.is_empty());
//! ```

mod builder;
mod drop_policy;
mod dropper;
mod pool;
mod slab;

pub use builder::*;
pub use drop_policy::*;
pub(crate) use dropper::*;
pub use pool::*;
pub(crate) use slab::*;

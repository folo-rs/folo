//! A pool that inserts typed values into pinned memory with automatic drop handling.
//!
//! This crate provides [`DatalessPool`], a dynamically growing pool that inserts values into
//! memory with a specific [`std::alloc::Layout`] defined at pool creation. It offers stable
//! memory addresses and efficient typed insertion with automatic value dropping.
//!
//! # Type-erased value management
//!
//! The pool works with any type that matches the layout specified at creation time. Values are
//! inserted using the unsafe [`DatalessPool::insert()`] method, which requires the caller to
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
//! - **Panic safety**: Pool panics on drop if values are still present (leak detection).
//!
//! # Example
//!
//! ```rust
//! use std::alloc::Layout;
//!
//! use dataless_pool::DatalessPool;
//!
//! // Create a pool for storing values that match the layout of `u64`.
//! let layout = Layout::new::<u64>();
//! let mut pool = DatalessPool::new(layout);
//!
//! // Insert values into the pool.
//! // SAFETY: The layout of u64 matches the pool's layout.
//! let pooled1 = unsafe { pool.insert(42_u64) };
//! // SAFETY: The layout of u64 matches the pool's layout.
//! let pooled2 = unsafe { pool.insert(123_u64) };
//!
//! // Read data back from the pooled items.
//! let value1 = unsafe {
//!     // SAFETY: The pointer is valid and the value was just inserted.
//!     pooled1.ptr().cast::<u64>().read()
//! };
//!
//! let value2 = unsafe {
//!     // SAFETY: The pointer is valid and the value was just inserted.
//!     pooled2.ptr().cast::<u64>().read()
//! };
//!
//! assert_eq!(value1, 42);
//! assert_eq!(value2, 123);
//!
//! // Remove values from the pool. The values are automatically dropped.
//! pool.remove(pooled1);
//! pool.remove(pooled2);
//!
//! assert!(pool.is_empty());
//! ```
//!
//! # Safety
//!
//! While the pool management itself is safe, accessing the memory requires `unsafe` code
//! since the pool works with type-erased memory. Users are responsible for:
//!
//! - Writing and reading data with the correct type that matches the layout.
//! - Not accessing memory after the reservation has been released.
//! - Ensuring proper initialization before reading.
//!
//! The reservation-based design helps prevent some common errors by making it impossible
//! to release memory without the original reservation object.

mod dropper;
mod pool;
mod slab;

#[allow(
    unused_imports,
    reason = "Dropper is part of the crate's public API but not yet used by other modules"
)]
pub(crate) use dropper::*;
pub use pool::*;
pub(crate) use slab::*;

//! A pool that reserves pinned memory without placing any data in it,
//! leaving placement of the data up to the owner.
//!
//! This crate provides [`DatalessPool`], a dynamically growing pool of memory capacity that works
//! with opaque memory blocks on a specific [`std::alloc::Layout`] defined at pool creation. It
//! offers stable memory addresses and efficient reservation-based memory management.
//!
//! # Placing data into the reserved memory
//!
//! The caller is expected to use unsafe code via pointers to read and write the data to be stored
//! in the reserved capacity.
//!
//! The pool itself does not hold or create any `&` shared or `&mut` exclusive references to its
//! contents, allowing the caller to decide who and when can obtain a reference to the reserved
//! memory. The caller is responsible for ensuring that Rust aliasing rules are respected.
//!
//! # Features
//!
//! - **Type-erased memory management**: Works with any memory layout via [`std::alloc::Layout`].
//! - **Stable addresses**: Memory addresses remain valid until explicitly released.
//! - **Dynamic growth**: Pool automatically expands as needed.
//! - **Efficient allocation**: Uses high density slabs to minimize allocation overhead.
//! - **Stable Rust**: No unstable Rust features required.
//! - **Some guardrails**: Prevents releasing memory without a reservation, though requires
//!   `unsafe` for data access.
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
//! // Reserve memory capacity for two items.
//! let reservation1 = pool.reserve();
//! let reservation2 = pool.reserve();
//!
//! // Write data to the first reservation.
//! unsafe {
//!     // SAFETY: The pointer is valid and aligned for u64, and we own the memory.
//!     reservation1.ptr().cast::<u64>().write(42);
//! }
//!
//! // Write data to the second reservation.
//! unsafe {
//!     // SAFETY: The pointer is valid and aligned for u64, and we own the memory.
//!     reservation2.ptr().cast::<u64>().write(123);
//! }
//!
//! // Read data back from the first reservation.
//! let value1 = unsafe {
//!     // SAFETY: The pointer is valid and the memory was just initialized.
//!     reservation1.ptr().cast::<u64>().read()
//! };
//!
//! // Read data back from the second reservation.
//! let value2 = unsafe {
//!     // SAFETY: The pointer is valid and the memory was just initialized.
//!     reservation2.ptr().cast::<u64>().read()
//! };
//!
//! assert_eq!(value1, 42);
//! assert_eq!(value2, 123);
//!
//! // Release memory back to the pool.
//! // SAFETY: The reserved memory contains u64 data which is Copy and has no destructor,
//! // so no destructors need to be called before releasing the memory.
//! unsafe {
//!     pool.release(reservation1);
//!     pool.release(reservation2);
//! }
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

//! A memory pool allocation library for type-erased memory management.
//!
//! This crate provides [`MemoryPool`], a dynamically growing memory pool that works
//! with opaque memory blocks. It offers stable memory addresses and efficient
//! reservation-based memory management.
//!
//! # Overview
//!
//! The [`MemoryPool`] manages memory through reservations represented by [`PoolReservation`]
//! objects. Each reservation acts as both a key and provides direct access to the allocated
//! memory. This design ensures that memory can only be released by returning the original
//! reservation, preventing use-after-free errors.
//!
//! # Features
//!
//! - **Type-erased memory management**: Works with any memory layout via [`std::alloc::Layout`]
//! - **Stable addresses**: Memory addresses remain valid until explicitly released
//! - **Dynamic growth**: Pool automatically expands as needed
//! - **Efficient allocation**: Uses internal slabs to minimize allocation overhead
//! - **Safe API**: Reservation-based design prevents common memory management errors
//!
//! # Example
//!
//! ```rust
//! use std::alloc::Layout;
//!
//! use memory_slab::MemoryPool;
//!
//! // Create a pool for storing u64 values
//! let layout = Layout::new::<u64>();
//! let mut pool = MemoryPool::new(layout);
//!
//! // Reserve memory and get reservations
//! let reservation1 = pool.reserve();
//! let reservation2 = pool.reserve();
//!
//! // Write data to the first reservation
//! unsafe {
//!     // SAFETY: The pointer is valid and aligned for u64, and we own the memory.
//!     reservation1.ptr().cast::<u64>().as_ptr().write(42);
//! }
//!
//! // Write data to the second reservation
//! unsafe {
//!     // SAFETY: The pointer is valid and aligned for u64, and we own the memory.
//!     reservation2.ptr().cast::<u64>().as_ptr().write(123);
//! }
//!
//! // Read data back from the first reservation
//! let value1 = unsafe {
//!     // SAFETY: The pointer is valid and the memory was just initialized.
//!     reservation1.ptr().cast::<u64>().as_ptr().read()
//! };
//!
//! // Read data back from the second reservation
//! let value2 = unsafe {
//!     // SAFETY: The pointer is valid and the memory was just initialized.
//!     reservation2.ptr().cast::<u64>().as_ptr().read()
//! };
//!
//! assert_eq!(value1, 42);
//! assert_eq!(value2, 123);
//!
//! // Release memory back to the pool
//! pool.release(reservation1);
//! pool.release(reservation2);
//!
//! assert!(pool.is_empty());
//! ```
//!
//! # Safety
//!
//! While the pool management itself is safe, accessing the memory requires `unsafe` code
//! since the pool works with type-erased memory. Users are responsible for:
//!
//! - Writing and reading data with the correct type that matches the layout
//! - Not accessing memory after the reservation has been released
//! - Ensuring proper initialization before reading
//!
//! The reservation-based design helps prevent some common errors by making it impossible
//! to release memory without the original reservation object.

mod pool;
mod slab;

pub use pool::{MemoryPool, PoolReservation};
pub(crate) use slab::MemorySlab;

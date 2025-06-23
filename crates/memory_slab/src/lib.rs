//! A memory pool allocation library for type-erased memory management.
//!
//! This crate provides `MemoryPool`, a dynamically growing memory pool that works
//! with opaque memory blocks. It offers stable memory addresses and efficient
//! reservation-based memory management.

mod pool;
mod slab;

pub use pool::{MemoryPool, PoolReservation};
pub(crate) use slab::MemorySlab;

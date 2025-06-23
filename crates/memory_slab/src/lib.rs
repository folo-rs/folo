//! A memory slab allocation library for fixed-capacity collections.
//!
//! This crate provides `MemorySlab`, a fixed-capacity heap-allocated collection that works
//! with opaque memory blocks. It offers stable memory addresses and efficient index-based
//! lookup for memory management scenarios.

mod pool;
mod slab;

pub use slab::*;

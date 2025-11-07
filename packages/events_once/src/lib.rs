//! Core implementation of oneshot events that deliver a value from sender to receiver at most once.
//!
//! This is further wrapped by more ergonomic types in the `events` package, adding more value.

mod backtrace;
mod core;
mod disconnected;
mod lake;
mod pool;
mod reflective_t;

pub use core::*;

#[cfg(debug_assertions)]
pub(crate) use backtrace::*;
pub use disconnected::*;
pub use lake::*;
pub use pool::*;
pub use reflective_t::*;

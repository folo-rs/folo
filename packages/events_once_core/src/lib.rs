//! Core implementation of oneshot events that deliver a value from sender to receiver at most once.
//!
//! This is further wrapped by more ergonomic types in the `events` package, adding more value.

mod backtrace;
mod disconnected;
mod local;
mod reflective_t;
mod state;
mod sync;

#[cfg(debug_assertions)]
pub(crate) use backtrace::*;
pub use disconnected::*;
pub use local::*;
pub use reflective_t::*;
pub(crate) use state::*;
pub use sync::*;

trait Sealed {}

const ERR_POISONED_LOCK: &str = "encountered poisoned lock - program validity cannot be guaranteed";
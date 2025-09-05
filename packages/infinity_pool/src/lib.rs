//! Infinity Pool: Advanced object pool implementations with flexible memory management.
//!
//! This crate provides several types of object pools designed for different use cases,
//! from basic pooling to advanced memory layouts with custom drop policies.

mod blind;
mod builders;
mod cast;
mod constants;
mod drop_policy;
mod handles;
mod opaque;
mod pinned;

pub use blind::*;
pub use builders::*;
pub(crate) use constants::*;
pub use drop_policy::*;
pub use handles::*;
pub use opaque::*;
// Re-export so we can use it without the consumer needing a reference.
#[doc(hidden)]
pub use pastey::paste as __private_paste;
pub use pinned::*;

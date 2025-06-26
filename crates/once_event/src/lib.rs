//! A one-shot event with support for async and object pooling for allocation-free behavior.
//!
//! This crate provides `OnceEvent<T>`, which is an asynchronous event that can be triggered at
//! most once to deliver a value of type `T` to at most one listener awaiting that value.
//!
//! The event can use multiple storage models, enabling object pooling for allocation-free
//! behavior. Alternatively, the event data storage can be embedded inline into another data
//! structure owned by the caller. Other storage models are also supported, focusing on
//! rapid development and easier integration.

mod event;
mod pinned_rc;
mod with_ref_count;

pub use event::*;
pub use pinned_rc::*;
pub(crate) use with_ref_count::*;

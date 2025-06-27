//! A one-shot event with support for async and object pooling for allocation-free behavior.
//!
//! This crate provides asynchronous events that can be triggered at most once to deliver a value
//! of type `T` to at most one listener awaiting that value.
//!
//! The events use multiple storage models, enabling object pooling for allocation-free behavior.
//! Alternatively, the event data storage can be embedded inline into another data structure owned
//! by the caller. Other storage models are also supported, focusing on rapid development and easier
//! integration.
//!
//! # New API Overview
//!
//! The new API is built around storage types that can be activated to create sender/receiver pairs:
//!
//! * [`OnceEventEmbedded`] - Single event embedded directly into a data structure
//! * [`OnceEventPoolByRef`] - Pool of events accessed via shared references
//! * [`OnceEventPoolByRc`] - Pool of events accessed via `Rc` references
//! * [`OnceEventPoolUnsafe`] - Pool of events accessed via unsafe raw pointers
//!
//! Each storage type provides an `activate()` method that returns a sender/receiver pair for a new event.

mod event;
mod pinned_rc;
mod with_ref_count;

pub use event::*;
pub use pinned_rc::*;
pub(crate) use with_ref_count::*;

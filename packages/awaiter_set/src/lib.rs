#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Awaiter tracking for async synchronization primitives.
//!
//! Async synchronization primitives (such as events) need a way to
//! track which futures are waiting and wake them when a resource
//! becomes available. This crate provides that mechanism.
//!
//! # Overview
//!
//! A synchronization primitive owns an [`AwaiterSet`]. Each future
//! that cannot complete immediately embeds an [`Awaiter`] and
//! registers it with the set. When the primitive wants to wake one
//! future (for example when a signal is raised), it calls
//! [`AwaiterSet::notify_one()`], which removes an awaiter from the
//! set and returns its waker.
#![doc = simple_mermaid::mermaid!("../docs/diagrams/list_structure.mermaid")]
//!
//! # Waker invocation
//!
//! Some primitives call [`Waker::wake()`][std::task::Waker::wake]
//! while holding their internal lock. This is safe for all
//! mainstream async runtimes, which only enqueue the task for later
//! polling rather than re-polling synchronously inside `wake()`.
//! However, a user-defined waker that re-enters the same primitive
//! from inside `wake()` will deadlock. Releasing the lock before
//! `wake()` is preferred when practical.

pub(crate) mod awaiter;
mod set;

pub use awaiter::Awaiter;
pub use set::AwaiterSet;

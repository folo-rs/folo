#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Awaiter tracking for async synchronization primitives.
//!
//! Async synchronization primitives (mutexes, semaphores, events)
//! need a way to track which futures are waiting and wake them when
//! a resource becomes available. This crate provides that mechanism.
//!
//! # Overview
//!
//! A synchronization primitive owns an [`AwaiterSet`]. Each future
//! that cannot complete immediately embeds an [`Awaiter`] and
//! registers it with the set. When the primitive wants to wake one
//! future (e.g. on unlock or permit release), it calls
//! [`AwaiterSet::notify_one()`], which removes an awaiter from the
//! set and returns its waker.
//!
#![doc = simple_mermaid::mermaid!("../docs/diagrams/list_structure.mermaid")]
//!
//! # Synchronization
//!
//! Neither `AwaiterSet` nor `Awaiter` has internal synchronization.
//! The synchronization primitive must protect all access to both the
//! set and its registered awaiters with a single lock (or confine
//! them to a single thread). This is because the primitive typically
//! needs to atomically update its own state (e.g. a `locked` flag)
//! together with the set.
//!
//! # Re-entrancy
//!
//! Calling [`Waker::wake()`][std::task::Waker::wake] may cause the
//! executor to immediately re-poll the woken future, which may try
//! to register a new awaiter in the same set. To prevent this from
//! racing with the current operation, the primitive must release its
//! lock before calling `wake()`.

pub(crate) mod awaiter;
mod set;

pub use awaiter::Awaiter;
pub use set::AwaiterSet;

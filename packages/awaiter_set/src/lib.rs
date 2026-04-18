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
//! The caller must ensure that all access to the set and its awaiters are
//! properly synchronized. It is up to the owner of these objects to
//! protect all access to both the set and its registered awaiters with
//! a single lock (or by confining them to a single thread).
//!
//! The synchronization must be external to the awaiter set because the
//! primitive typically needs to atomically update its own state
//! (e.g. a `locked` flag) together with the set.
//!
//! # Waker invocation
//!
//! No mainstream async runtime re-polls a future synchronously
//! inside [`Waker::wake()`][std::task::Waker::wake] — they all
//! enqueue the task for later polling. Primitives may therefore
//! call `wake()` while holding their lock without risking
//! reentrancy deadlocks. However, releasing the lock before
//! `wake()` is still preferred when practical, as it reduces
//! contention.

pub(crate) mod awaiter;
mod set;

pub use awaiter::Awaiter;
pub use set::AwaiterSet;

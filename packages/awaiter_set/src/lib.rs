#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Zero-allocation awaiter tracking for async synchronization
//! primitives.
//!
//! When building async locks, semaphores, or events, futures that
//! cannot complete immediately need to be parked and later woken.
//! This crate provides the underlying set for that purpose: each
//! node lives inside the awaiting future itself, so no heap
//! allocation is needed to register or remove awaiters.
//!
//! # When to use this crate
//!
//! Use `awaiter_set` when you are implementing a new synchronization
//! primitive and need an efficient, allocation-free mechanism to track
//! pending futures. The [`asynchroniz`] and [`events`] crates are
//! built on top of this crate.
//!
//! [`asynchroniz`]: https://docs.rs/asynchroniz
//! [`events`]: https://docs.rs/events
//!
//! # Core types
//!
//! * [`AwaiterSet`] — the set of registered awaiters.
//! * [`AwaiterNode`] — a single entry, embedded inside a future.
//! * [`AwaiterNodeStorage`] — a convenience wrapper that bundles a
//!   node with its registration state and a pinning marker, reducing
//!   boilerplate in future implementations.
#![doc = simple_mermaid::mermaid!("../docs/diagrams/list_structure.mermaid")]
//!
//! # Synchronization
//!
//! The set has no internal synchronization. Callers must ensure
//! exclusive access for all operations — for example, by protecting
//! all node and set access with a [`Mutex`][std::sync::Mutex] or
//! equivalent, or by confining the containing type to a single thread.
//!
//! # Re-entrancy
//!
//! [`Waker::wake()`][std::task::Waker::wake] may be re-entrant: the
//! waker's executor can immediately poll the woken future, which may
//! attempt to access the same set. Callers must release locks before
//! calling `wake()` and rescan afterward, because the set may have
//! changed during the unlock window.

mod awaiter_node_storage;
mod list;
mod node;

pub use awaiter_node_storage::*;
pub use list::*;
pub use node::*;

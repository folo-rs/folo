#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Zero-allocation waiter queue for async synchronization primitives.
//!
//! When building async locks, semaphores, or events, futures that cannot
//! complete immediately need to be parked and later woken. This crate
//! provides the underlying queue for that purpose: a doubly-linked list
//! where each node lives inside the waiting future itself, so no heap
//! allocation is needed to enqueue or dequeue waiters.
//!
//! # When to use this crate
//!
//! Use `waiter_list` when you are implementing a new synchronization
//! primitive and need an efficient, allocation-free mechanism to track
//! pending futures. The [`asynchroniz`] and [`events`] crates are built
//! on top of this crate.
//!
//! [`asynchroniz`]: https://docs.rs/asynchroniz
//! [`events`]: https://docs.rs/events
//!
//! # Core types
//!
//! * [`WaiterList`] — the queue itself.
//! * [`WaiterNode`] — a single entry in the queue, embedded inside a
//!   future.
//! * [`WaiterNodeStorage`] — a convenience wrapper that bundles a node with
//!   its registration state and a pinning marker, reducing boilerplate
//!   in future implementations.
//!
#![doc = simple_mermaid::mermaid!("../doc/list_structure.mermaid")]
//!
//! # Synchronization
//!
//! The list has no internal synchronization. Callers must ensure
//! exclusive access for all operations — for example, by protecting all
//! node and list access with a [`Mutex`][std::sync::Mutex] or equivalent,
//! or by confining the containing type to a single thread.
//!
//! # Re-entrancy
//!
//! [`Waker::wake()`][std::task::Waker::wake] may be re-entrant: the
//! waker's executor can immediately poll the woken future, which may
//! attempt to access the same list. Callers must release locks before
//! calling `wake()` and rescan from the list head afterward, because
//! the list may have changed during the unlock window.

mod list;
mod node;
mod node_storage;

pub use list::*;
pub use node::*;
pub use node_storage::*;

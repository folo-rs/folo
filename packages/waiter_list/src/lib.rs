#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Intrusive doubly-linked waiter queue for async synchronization primitives.
//!
//! This crate provides [`WaiterList`] and [`WaiterNode`], a FIFO queue designed
//! for parking and waking async futures that wait on synchronization events.
//! The list is *intrusive*: each node is embedded directly inside the wait
//! future (typically behind an [`UnsafeCell`][std::cell::UnsafeCell]) rather
//! than being heap-allocated, making registration and removal zero-allocation.
//!
//! # Ownership model
//!
//! The [`WaiterList`] does not own its nodes. Each [`WaiterNode`] is a field of
//! the future that created it. The node is linked into the list when the future
//! is first polled and unlinked either by the future's [`Drop`] implementation
//! or by the synchronization primitive's notification logic.
//!
//! Nodes contain [`PhantomPinned`][std::marker::PhantomPinned] because the list
//! holds raw pointers to them. The containing future must be pinned before
//! polling, ensuring the node address remains stable for its lifetime in the
//! list.
#![doc = simple_mermaid::mermaid!("../doc/list_structure.mermaid")]
//!
//! # Synchronization
//!
//! The list has no internal synchronization. Callers must ensure exclusive
//! access for all operations:
//!
//! * **Thread-safe primitives** — protect all node and list access with a
//!   [`Mutex`][std::sync::Mutex].
//! * **Single-threaded primitives** — constrain the containing type to `!Send`
//!   so all access is confined to a single thread.
//!
//! # Re-entrancy
//!
//! [`Waker::wake()`][std::task::Waker::wake] may be re-entrant: the waker's
//! executor can immediately poll the woken future, which may attempt to access
//! the same list. Callers must release locks before calling `wake()` and
//! rescan from the list head afterward, because the list may have changed
//! during the unlock window.

mod list;
mod node;
mod slot;

pub use list::*;
pub use node::*;
pub use slot::*;

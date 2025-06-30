//! Events that can happen at most once (send/receive consumes the sender/receiver).
//!
//! This module provides one-time use events in both single-threaded and thread-safe variants.
//! Each event can only be used once - after sending and receiving a value, the sender and
//! receiver are consumed.
//!
//! ## Event Types
//!
//! - [`OnceEvent`] - Thread-safe events that can be shared across threads
//! - [`LocalOnceEvent`] - Single-threaded events with lower overhead
//! - [`OnceEventPool`] - Thread-safe pooled events with automatic resource management
//! - [`LocalOnceEventPool`] - Single-threaded pooled events with automatic resource management
//!
//! ## Single-threaded Usage Pattern
//!
//! 1. Create an instance of [`LocalOnceEvent<T>`] (potentially inside an [`std::rc::Rc`])
//! 2. Call [`LocalOnceEvent::bind_by_ref()`] or another binding method to get a tuple with both sender
//!    and receiver instances
//! 3. You can only do this once (panic on 2nd call; [`LocalOnceEvent::bind_by_ref_checked()`] is also
//!    supported, returning [`None`] on 2nd call instead)
//! 4. Use [`ByRefLocalOnceSender`]/[`ByRefLocalOnceReceiver`] as desired, either dropping them or
//!    consuming them via self-taking methods
//!
//! ## Thread-safe Usage Pattern
//!
//! 1. Create an instance of [`OnceEvent<T>`] (potentially inside an [`std::sync::Arc`] or [`std::rc::Rc`])
//! 2. Call [`OnceEvent::bind_by_ref()`] or another binding method to get a tuple with both sender
//!    and receiver instances
//! 3. You can only do this once (panic on 2nd call; [`OnceEvent::bind_by_ref_checked()`] is also
//!    supported, returning [`None`] on 2nd call instead)
//! 4. Use [`ByRefOnceSender`]/[`ByRefOnceReceiver`] as desired, either dropping them or
//!    consuming them via self-taking methods
//!
//! ## Pooled Event Usage
//!
//! For automatic resource management, use [`OnceEventPool`] (thread-safe) or [`LocalOnceEventPool`]
//! (single-threaded). These pools automatically create and clean up events without memory
//! allocation overhead, making them suitable for high-frequency scenarios.
//!
//! # Example (Thread-safe)
//!
//! ```rust
//! use events::OnceEvent;
//! # use futures::executor::block_on;
//!
//! # block_on(async {
//! let event = OnceEvent::<i32>::new();
//! let (sender, receiver) = event.bind_by_ref();
//!
//! sender.send(42);
//! let value = receiver.await;
//! assert_eq!(value, 42);
//! # });
//! ```
//!
//! # Example (Single-threaded)
//!
//! ```rust
//! use events::LocalOnceEvent;
//! # use futures::executor::block_on;
//!
//! # block_on(async {
//! let event = LocalOnceEvent::<i32>::new();
//! let (sender, receiver) = event.bind_by_ref();
//!
//! sender.send(42);
//! let value = receiver.await;
//! assert_eq!(value, 42);
//! # });
//! ```
//!
//! # Example (Pooled Local Events)
//!
//! ```rust
//! use events::LocalOnceEventPool;
//! # use futures::executor::block_on;
//!
//! # block_on(async {
//! let pool = LocalOnceEventPool::<i32>::new();
//!
//! // First usage - creates new event  
//! let (sender1, receiver1) = pool.bind_by_ref();
//! sender1.send(42);
//! let value1 = receiver1.await;
//! assert_eq!(value1, 42);
//! // Event automatically returned to pool when endpoints are dropped
//!
//! // Second usage - reuses the same event instance efficiently
//! let (sender2, receiver2) = pool.bind_by_ref();
//! sender2.send(100);
//! let value2 = receiver2.await;
//! assert_eq!(value2, 100);
//! // Same event reused - no additional allocation overhead
//! # });
//! ```
//!
//! # Example (Pooled Thread-safe Events)
//!
//! ```rust
//! use events::OnceEventPool;
//! # use futures::executor::block_on;
//!
//! # block_on(async {
//! let pool = OnceEventPool::<i32>::new();
//!
//! // First usage - creates new event
//! let (sender1, receiver1) = pool.bind_by_ref();
//! sender1.send(42);
//! let value1 = receiver1.recv_async().await;
//! assert_eq!(value1, 42);
//! 
//! // Second usage - efficiently reuses the same underlying event
//! let (sender2, receiver2) = pool.bind_by_ref();
//! sender2.send(200);
//! let value2 = receiver2.recv_async().await;
//! assert_eq!(value2, 200);
//! // Pool automatically manages event lifecycle and reuse
//! # });
//! ```

mod local;
mod pinned_with_ref_count;
mod pooled_local;
mod pooled_sync;
mod sync;

// Re-export all public types from all modules
pub use local::{
    ByPtrLocalOnceReceiver, ByPtrLocalOnceSender, ByRcLocalOnceReceiver, ByRcLocalOnceSender,
    ByRefLocalOnceReceiver, ByRefLocalOnceSender, LocalOnceEvent,
};
pub use pooled_local::{
    ByPtrPooledLocalOnceReceiver, ByPtrPooledLocalOnceSender, ByRcPooledLocalOnceReceiver,
    ByRcPooledLocalOnceSender, ByRefPooledLocalOnceReceiver, ByRefPooledLocalOnceSender,
    LocalOnceEventPool,
};
#[allow(
    clippy::module_name_repetitions,
    reason = "OnceEventPool is the correct name for this type"
)]
pub use pooled_sync::{
    ByArcPooledOnceReceiver, ByArcPooledOnceSender, ByPtrPooledOnceReceiver, ByPtrPooledOnceSender,
    ByRcPooledOnceReceiver, ByRcPooledOnceSender, ByRefPooledOnceReceiver, ByRefPooledOnceSender,
    OnceEventPool,
};
#[allow(
    clippy::module_name_repetitions,
    reason = "OnceEvent is the correct name for this type"
)]
pub use sync::{
    ByArcOnceReceiver, ByArcOnceSender, ByPtrOnceReceiver, ByPtrOnceSender, ByRcOnceReceiver,
    ByRcOnceSender, ByRefOnceReceiver, ByRefOnceSender, OnceEvent,
};

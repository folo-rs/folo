//! Events that can happen at most once (send/receive consumes the sender/receiver).
//!
//! This module provides one-time use events in both single-threaded and thread-safe variants.
//! Each event can only be used once - after sending and receiving a value, the sender and
//! receiver are consumed.
//!
//! ## Event Types
//!
//! - [`Event`] - Thread-safe events that can be shared across threads
//! - [`LocalEvent`] - Single-threaded events with lower overhead
//! - [`EventPool`] - Thread-safe pooled events with automatic resource management
//! - [`LocalEventPool`] - Single-threaded pooled events with automatic resource management
//!
//! ## Single-threaded Usage Pattern
//!
//! 1. Create an instance of [`LocalEvent<T>`] (potentially inside an [`std::rc::Rc`])
//! 2. Call [`LocalEvent::by_ref()`] or another activation method to get a tuple with both sender
//!    and receiver instances
//! 3. You can only do this once (panic on 2nd call; [`LocalEvent::by_ref_checked()`] is also
//!    supported, returning [`None`] on 2nd call instead)
//! 4. Use [`ByRefLocalEventSender`]/[`ByRefLocalEventReceiver`] as desired, either dropping them or
//!    consuming them via self-taking methods
//!
//! ## Thread-safe Usage Pattern
//!
//! 1. Create an instance of [`Event<T>`] (potentially inside an [`std::sync::Arc`] or [`std::rc::Rc`])
//! 2. Call [`Event::by_ref()`] or another activation method to get a tuple with both sender
//!    and receiver instances
//! 3. You can only do this once (panic on 2nd call; [`Event::by_ref_checked()`] is also
//!    supported, returning [`None`] on 2nd call instead)
//! 4. Use [`ByRefEventSender`]/[`ByRefEventReceiver`] as desired, either dropping them or
//!    consuming them via self-taking methods
//!
//! ## Pooled Event Usage
//!
//! For automatic resource management, use [`EventPool`] (thread-safe) or [`LocalEventPool`]
//! (single-threaded). These pools automatically create and clean up events without memory
//! allocation overhead, making them suitable for high-frequency scenarios.
//!
//! # Example (Thread-safe)
//!
//! ```rust
//! use events::once::Event;
//! use futures::executor::block_on;
//!
//! block_on(async {
//!     let event = Event::<i32>::new();
//!     let (sender, receiver) = event.by_ref();
//!
//!     sender.send(42);
//!     let value = receiver.await;
//!     assert_eq!(value, 42);
//! });
//! ```
//!
//! # Example (Single-threaded)
//!
//! ```rust
//! use events::once::LocalEvent;
//! use futures::executor::block_on;
//!
//! block_on(async {
//!     let event = LocalEvent::<i32>::new();
//!     let (sender, receiver) = event.by_ref();
//!
//!     sender.send(42);
//!     let value = receiver.await;
//!     assert_eq!(value, 42);
//! });
//! ```
//!
//! # Example (Pooled Local Events)
//!
//! ```rust
//! use events::once::LocalEventPool;
//! use futures::executor::block_on;
//!
//! block_on(async {
//!     let mut pool = LocalEventPool::<i32>::new();
//!     let (sender, receiver) = pool.by_ref();
//!
//!     sender.send(42);
//!     let value = receiver.await;
//!     assert_eq!(value, 42);
//!     // Event automatically returned to pool when endpoints are dropped
//! });
//! ```

mod local;
mod pooled_local;
mod pooled_sync;
mod sync;

// Re-export all public types from all modules
pub use local::{
    ByPtrLocalEventReceiver, ByPtrLocalEventSender, ByRcLocalEventReceiver, ByRcLocalEventSender,
    ByRefLocalEventReceiver, ByRefLocalEventSender, LocalEvent,
};
pub use pooled_local::{
    ByPtrPooledLocalEventReceiver, ByPtrPooledLocalEventSender, ByRcPooledLocalEventReceiver,
    ByRcPooledLocalEventSender, ByRefPooledLocalEventReceiver, ByRefPooledLocalEventSender,
    LocalEventPool, WithRefCountLocal,
};
pub use pooled_sync::{
    ByArcPooledEventReceiver, ByArcPooledEventSender, ByPtrPooledEventReceiver,
    ByPtrPooledEventSender, ByRcPooledEventReceiver, ByRcPooledEventSender,
    ByRefPooledEventReceiver, ByRefPooledEventSender, EventPool, WithRefCount,
};
pub use sync::{
    ByArcEventReceiver, ByArcEventSender, ByPtrEventReceiver, ByPtrEventSender, ByRcEventReceiver,
    ByRcEventSender, ByRefEventReceiver, ByRefEventSender, Event,
};

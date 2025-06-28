//! Events that can happen at most once (send/receive consumes the sender/receiver).
//!
//! This module provides one-time use events in both single-threaded and thread-safe variants.
//! Each event can only be used once - after sending and receiving a value, the sender and
//! receiver are consumed.
//!
//! # Single-threaded Usage Pattern
//!
//! 1. Create an instance of [`LocalEvent<T>`] (potentially inside an [`std::rc::Rc`])
//! 2. Call [`LocalEvent::by_ref()`] to get a tuple with both sender and receiver instances
//! 3. You can only do this once (panic on 2nd call; [`LocalEvent::by_ref_checked()`] is also
//!    supported, returning [`None`] on 2nd call instead)
//! 4. Use [`ByRefLocalEventSender`]/[`ByRefLocalEventReceiver`] as desired, either dropping them or
//!    consuming them via self-taking methods
//!
//! # Thread-safe Usage Pattern
//!
//! 1. Create an instance of [`Event<T>`] (potentially inside an [`std::sync::Arc`])
//! 2. Call [`Event::by_ref()`] to get a tuple with both sender and receiver instances
//! 3. You can only do this once (panic on 2nd call; [`Event::by_ref_checked()`] is also
//!    supported, returning [`None`] on 2nd call instead)
//! 4. Use [`ByRefEventSender`]/[`ByRefEventReceiver`] as desired, either dropping them or
//!    consuming them via self-taking methods
//!
//! # Example (Thread-safe)
//!
//! ```rust
//! use events::once::Event;
//!
//! let event = Event::<i32>::new();
//! let (sender, receiver) = event.by_ref();
//!
//! sender.send(42);
//! let value = receiver.recv();
//! assert_eq!(value, 42);
//! ```
//!
//! # Example (Single-threaded)
//!
//! ```rust
//! use events::once::LocalEvent;
//!
//! let event = LocalEvent::<i32>::new();
//! let (sender, receiver) = event.by_ref();
//!
//! sender.send(42);
//! let value = receiver.recv();
//! assert_eq!(value, 42);
//! ```

mod local;
mod sync;

// Re-export all public types from both modules
pub use local::{
    ByPtrLocalEventReceiver, ByPtrLocalEventSender, ByRcLocalEventReceiver, ByRcLocalEventSender,
    ByRefLocalEventReceiver, ByRefLocalEventSender, LocalEvent,
};
pub use sync::{
    ByArcEventReceiver, ByArcEventSender, ByPtrEventReceiver, ByPtrEventSender, ByRcEventReceiver,
    ByRcEventSender, ByRefEventReceiver, ByRefEventSender, Event,
};

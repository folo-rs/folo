//! Events that can happen at most once (send/receive consumes the sender/receiver).
//!
//! This module provides one-time use events in both single-threaded and thread-safe variants.
//! Each event can only be used once - after sending and receiving a value, the sender and
//! receiver are consumed.
//!
//! # Single-threaded Usage Pattern
//!
//! 1. Create an instance of [`LocalEvent<T>`] (potentially inside an [`std::rc::Rc`])
//! 2. Call [`LocalEvent::sender()`] and [`LocalEvent::receiver()`] to get instances of each, or
//!    [`LocalEvent::endpoints()`] to get a tuple with both
//! 3. You can only do this once (panic on 2nd call; [`LocalEvent::sender_checked()`] is also
//!    supported, returning [`None`] on 2nd call instead)
//! 4. Use [`ByRefLocalEventSender`]/[`ByRefLocalEventReceiver`] as desired, either dropping them or
//!    consuming them via self-taking methods
//!
//! # Thread-safe Usage Pattern
//!
//! 1. Create an instance of [`Event<T>`] (potentially inside an [`std::sync::Arc`])
//! 2. Call [`Event::sender()`] and [`Event::receiver()`] to get instances of each, or
//!    [`Event::endpoints()`] to get a tuple with both
//! 3. You can only do this once (panic on 2nd call; [`Event::sender_checked()`] is also
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
//! let (sender, receiver) = event.endpoints();
//!
//! sender.send(42);
//! let value = receiver.receive();
//! assert_eq!(value, 42);
//! ```
//!
//! # Example (Single-threaded)
//!
//! ```rust
//! use events::once::LocalEvent;
//!
//! let event = LocalEvent::<i32>::new();
//! let (sender, receiver) = event.endpoints();
//!
//! sender.send(42);
//! let value = receiver.receive();
//! assert_eq!(value, 42);
//! ```

mod local;
mod sync;

// Re-export all public types from both modules
pub use local::{ByRefLocalEventReceiver, ByRefLocalEventSender, LocalEvent};
pub use sync::{ByRefEventReceiver, ByRefEventSender, Event};

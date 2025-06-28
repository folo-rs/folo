//! High-performance event signaling primitives for concurrent environments.
//!
//! (DRAFT API WITH PLACEHOLDER IMPLEMENTATION - WORK IN PROGRESS)
//!
//! This crate provides lightweight, efficient signaling mechanisms for communicating between
//! different parts of an application. The API is designed to be simple to use while offering
//! high performance in concurrent scenarios.
//!
//! Both single-threaded and thread-safe variants are available:
//! - [`Event<T>`], [`ByRefEventSender<T>`], [`ByRefEventReceiver<T>`] - Thread-safe variants
//! - [`LocalEvent<T>`], [`ByRefLocalEventSender<T>`], [`ByRefLocalEventReceiver<T>`] - Single-threaded variants
//!
//! Each receiver type supports both synchronous and asynchronous receiving:
//! - [`ByRefEventReceiver::recv`] / [`ByRefLocalEventReceiver::recv`] - Synchronous
//! - [`ByRefEventReceiver::recv_async`] / [`ByRefLocalEventReceiver::recv_async`] - Asynchronous
//!
//! # Thread-safe Example
//!
//! ```rust
//! use events::once::Event;
//!
//! // Create a thread-safe event for passing a string message
//! let event = Event::<String>::new();
//! let (sender, receiver) = event.by_ref();
//!
//! // Send a message through the event
//! sender.send("Hello, World!".to_string());
//!
//! // Receive the message
//! let message = receiver.recv();
//! assert_eq!(message, "Hello, World!");
//! ```
//!
//! # Single-threaded Example
//!
//! ```rust
//! use events::once::LocalEvent;
//!
//! // Create a local event for passing a string message
//! let event = LocalEvent::<String>::new();
//! let (sender, receiver) = event.by_ref();
//!
//! // Send a message through the event
//! sender.send("Hello, World!".to_string());
//!
//! // Receive the message
//! let message = receiver.recv();
//! assert_eq!(message, "Hello, World!");
//! ```
//!
//! # Async Example
//!
//! ```rust
//! use events::once::Event;
//! use futures::executor::block_on;
//!
//! // Create a thread-safe event for async communication
//! let event = Event::<i32>::new();
//! let (sender, receiver) = event.by_ref();
//!
//! // Send a value through the event
//! sender.send(42);
//!
//! // Receive the value asynchronously
//! let value = block_on(receiver.recv_async());
//! assert_eq!(value, 42);
//! ```

pub mod once;

pub use once::*;

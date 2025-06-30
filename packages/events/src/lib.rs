//! High-performance event signaling primitives for concurrent environments.
//!
//! (DRAFT API WITH PLACEHOLDER IMPLEMENTATION - WORK IN PROGRESS)
//!
//! This crate provides lightweight, efficient signaling mechanisms for communicating between
//! different parts of an application. The API is designed to be simple to use while offering
//! high performance in concurrent scenarios.
//!
//! Both single-threaded and thread-safe variants are available:
//! - [`Event<T>`], [`ByRefEventSender<T>`], [`ByRefEventReceiver<T>`] - Thread-safe variants using references
//! - [`ByArcEventSender<T>`], [`ByArcEventReceiver<T>`] - Thread-safe variants using Arc ownership
//! - [`ByRcEventSender<T>`], [`ByRcEventReceiver<T>`] - Thread-safe variants using Rc ownership (single-threaded)
//! - [`LocalEvent<T>`], [`ByRefLocalEventSender<T>`], [`ByRefLocalEventReceiver<T>`] - Single-threaded variants using references
//! - [`ByRcLocalEventSender<T>`], [`ByRcLocalEventReceiver<T>`] - Single-threaded variants using Rc ownership
//!
//! Each receiver type implements [`Future`], allowing you to `.await` them directly.
//!
//! # Thread-safe Example
//!
//! ```rust
//! use events::once::Event;
//! use futures::executor::block_on;
//!
//! block_on(async {
//!     // Create a thread-safe event for passing a string message
//!     let event = Event::<String>::new();
//!     let (sender, receiver) = event.by_ref();
//!
//!     // Send a message through the event
//!     sender.send("Hello, World!".to_string());
//!
//!     // Receive the message
//!     let message = receiver.await;
//!     assert_eq!(message, "Hello, World!");
//! });
//! ```
//!
//! # Single-threaded Example
//!
//! ```rust
//! use events::once::LocalEvent;
//! use futures::executor::block_on;
//!
//! block_on(async {
//!     // Create a local event for passing a string message
//!     let event = LocalEvent::<String>::new();
//!     let (sender, receiver) = event.by_ref();
//!
//!     // Send a message through the event
//!     sender.send("Hello, World!".to_string());
//!
//!     // Receive the message
//!     let message = receiver.await;
//!     assert_eq!(message, "Hello, World!");
//! });
//! ```
//! 
//! # Arc-based Example
//!
//! ```rust
//! use std::sync::Arc;
//!
//! use events::once::Event;
//! use futures::executor::block_on;
//!
//! block_on(async {
//!     // Create an Arc-wrapped event for shared ownership
//!     let event = Arc::new(Event::<String>::new());
//!     let (sender, receiver) = event.by_arc();
//!
//!     // Send a message through the event
//!     sender.send("Hello, Arc!".to_string());
//!
//!     // Receive the message
//!     let message = receiver.await;
//!     assert_eq!(message, "Hello, Arc!");
//! });
//! ```
//!
//! # Rc-based Example
//!
//! ```rust
//! use std::rc::Rc;
//!
//! use events::once::LocalEvent;
//! use futures::executor::block_on;
//!
//! block_on(async {
//!     // Create an Rc-wrapped local event for shared ownership (single-threaded)
//!     let event = Rc::new(LocalEvent::<String>::new());
//!     let (sender, receiver) = event.by_rc();
//!
//!     // Send a message through the event
//!     sender.send("Hello, Rc!".to_string());
//!
//!     // Receive the message
//!     let message = receiver.await;
//!     assert_eq!(message, "Hello, Rc!");
//! });
//! ```

pub mod once;

mod futures;

pub use once::*;

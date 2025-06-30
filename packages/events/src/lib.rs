//! High-performance event signaling primitives for concurrent environments.
//!
//! (DRAFT API WITH PLACEHOLDER IMPLEMENTATION - WORK IN PROGRESS)
//!
//! This crate provides lightweight, efficient signaling mechanisms for communicating between
//! different parts of an application. The API is designed to be simple to use while offering
//! high performance in concurrent scenarios.
//!
//! Both single-threaded and thread-safe variants are available:
//! - [`OnceEvent<T>`], [`ByRefOnceSender<T>`], [`ByRefOnceReceiver<T>`] - Thread-safe variants using references
//! - [`ByArcOnceSender<T>`], [`ByArcOnceReceiver<T>`] - Thread-safe variants using Arc ownership
//! - [`ByRcOnceSender<T>`], [`ByRcOnceReceiver<T>`] - Thread-safe variants using Rc ownership (single-threaded)
//! - [`LocalOnceEvent<T>`], [`ByRefLocalOnceSender<T>`], [`ByRefLocalOnceReceiver<T>`] - Single-threaded variants using references
//! - [`ByRcLocalOnceSender<T>`], [`ByRcLocalOnceReceiver<T>`] - Single-threaded variants using Rc ownership
//!
//! Each receiver type implements [`Future`], allowing you to `.await` them directly.
//!
//! # Thread-safe Example
//!
//! ```rust
//! use events::OnceEvent;
//! # use futures::executor::block_on;
//!
//! # block_on(async {
//! // Create a thread-safe event for passing a string message
//! let event = OnceEvent::<String>::new();
//! let (sender, receiver) = event.by_ref();
//!
//! // Send a message through the event
//! sender.send("Hello, World!".to_string());
//!
//! // Receive the message
//! let message = receiver.await;
//! assert_eq!(message, "Hello, World!");
//! # });
//! ```
//!
//! # Single-threaded Example
//!
//! ```rust
//! use events::LocalOnceEvent;
//! # use futures::executor::block_on;
//!
//! # block_on(async {
//! // Create a local event for passing a string message
//! let event = LocalOnceEvent::<String>::new();
//! let (sender, receiver) = event.by_ref();
//!
//! // Send a message through the event
//! sender.send("Hello, World!".to_string());
//!
//! // Receive the message
//! let message = receiver.await;
//! assert_eq!(message, "Hello, World!");
//! # });
//! ```
//!
//! # Arc-based Example
//!
//! ```rust
//! use std::sync::Arc;
//!
//! use events::OnceEvent;
//! # use futures::executor::block_on;
//!
//! # block_on(async {
//! // Create an Arc-wrapped event for shared ownership
//! let event = Arc::new(OnceEvent::<String>::new());
//! let (sender, receiver) = event.by_arc();
//!
//! // Send a message through the event
//! sender.send("Hello, Arc!".to_string());
//!
//! // Receive the message
//! let message = receiver.await;
//! assert_eq!(message, "Hello, Arc!");
//! # });
//! ```
//!
//! # Rc-based Example
//!
//! ```rust
//! use std::rc::Rc;
//!
//! use events::LocalOnceEvent;
//! # use futures::executor::block_on;
//!
//! # block_on(async {
//! // Create an Rc-wrapped local event for shared ownership (single-threaded)
//! let event = Rc::new(LocalOnceEvent::<String>::new());
//! let (sender, receiver) = event.by_rc();
//!
//! // Send a message through the event
//! sender.send("Hello, Rc!".to_string());
//!
//! // Receive the message
//! let message = receiver.await;
//! assert_eq!(message, "Hello, Rc!");
//! # });
//! ```

pub mod once;

mod futures;

pub use once::*;

//! High-performance event signaling primitives for concurrent environments.
//!
//! This crate provides lightweight, efficient signaling mechanisms for communicating between
//! different parts of an application. The API is designed to be simple to use while offering
//! high performance in concurrent scenarios.
//!
//! # Example
//!
//! ```rust
//! use events::once::Event;
//!
//! // Create an event for passing a string message
//! let event = Event::<String>::new();
//! let (sender, receiver) = event.endpoints();
//!
//! // Send a message through the event
//! sender.send("Hello, World!".to_string());
//!
//! // Receive the message
//! let message = receiver.receive();
//! assert_eq!(message, "Hello, World!");
//! ```

pub mod once;

pub use once::*;

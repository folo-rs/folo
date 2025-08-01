//! High-performance event signaling primitives for concurrent environments.
//!
//! This package provides lightweight, efficient signaling mechanisms for communicating between
//! different parts of an application. The API is designed to be simple to use while offering
//! high performance in concurrent scenarios.
//!
//! ## Event Types
//!
//! This module provides one-time use events in both single-threaded and thread-safe variants.
//! Each event can only be used once - after sending and receiving a value, the sender and
//! receiver are consumed.
//!
//! - [`OnceEvent`] - Thread-safe events that can be shared across threads
//! - [`LocalOnceEvent`] - Single-threaded events with lower overhead
//! - [`OnceEventPool`] - Thread-safe pooled events with automatic resource management
//! - [`LocalOnceEventPool`] - Single-threaded pooled events with automatic resource management
//!
//! # Design Philosophy: Explicit Event Management
//!
//! Unlike traditional communication primitives (such as `oneshot` channels) where the
//! synchronization object is hidden behind dynamic allocation, this package brings the
//! **event** object front and center. This fundamental design decision enables several
//! key optimizations:
//!
//! ## Memory Management Efficiency
//!
//! In typical `oneshot` implementations, the synchronization state is allocated on the heap
//! and managed internally. By exposing the event object explicitly, callers can:
//!
//! - **Embed events in other structures**: Store the event directly within larger data
//!   structures, eliminating separate allocations
//! - **Use stack allocation**: For short-lived events, avoid heap allocation entirely
//! - **Leverage custom allocators**: Use specialized allocation strategies appropriate
//!   for their use case
//! - **Enable zero-allocation patterns**: Through pooling and reuse strategies
//!
//! ## Resource Pooling
//!
//! The explicit event structure makes pooling natural and efficient. The package provides
//! built-in pooling implementations ([`OnceEventPool`], [`LocalOnceEventPool`]) that
//! automatically manage event lifecycle without any dynamic allocation overhead after
//! the initial pool setup.
//!
//! ## Flexible Ownership Models
//!
//! The separation of the event from its endpoints (sender/receiver) allows for multiple
//! ownership patterns for both individual events and event pools:
//!
//! Event Ownership Models
//! - **Reference-based**: Minimal overhead when the event lifetime is managed externally
//! - **Arc/Rc-based**: Shared ownership when needed for complex scenarios
//! - **Pointer-based**: Direct pointer management for maximum performance
//!
//! Pool Ownership Models
//! - **Reference-based**: Pool accessed via shared reference (`&OnceEventPool`, `&LocalOnceEventPool`)
//! - **Arc/Rc-based**: Pool shared across multiple contexts (`Arc<OnceEventPool>`, `Rc<LocalOnceEventPool>`)
//! - **Pointer-based**: Direct pointer access to pinned pools for maximum performance
//!
//! This flexibility allows applications to choose the ownership model that best fits
//! their performance and safety requirements, rather than being forced into a single
//! dynamic allocation pattern.
//!
//! Both single-threaded and thread-safe variants are available for both individual events and pools:
//! - [`OnceEvent<T>`], [`OnceSender<E>`], [`OnceReceiver<E>`] - Thread-safe event variants
//! - [`LocalOnceEvent<T>`], [`LocalOnceSender<E>`], [`LocalOnceReceiver<E>`] - Single-threaded event variants
//! - [`OnceEventPool<T>`], [`PooledOnceSender<P>`], [`PooledOnceReceiver<P>`] - Thread-safe pool variants
//! - [`LocalOnceEventPool<T>`], [`PooledLocalOnceSender<P>`], [`PooledLocalOnceReceiver<P>`] - Single-threaded pool variants
//!
//! Each receiver type implements [`Future`], allowing you to `.await` them directly.
//! In debug builds, if backtraces are enabled, the owner of the event/pool can use
//! `inspect_awaiter()` or `inspect_awaiters()` to see the backtraces indicating where
//! the `.await` was issued from.
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
//! let (sender, receiver) = event.bind_by_ref();
//!
//! // Send a message through the event
//! sender.send("Hello, World!".to_string());
//!
//! // Receive the message
//! let message = receiver.await.unwrap();
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
//! let (sender, receiver) = event.bind_by_ref();
//!
//! // Send a message through the event
//! sender.send("Hello, World!".to_string());
//!
//! // Receive the message
//! let message = receiver.await.unwrap();
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
//! let (sender, receiver) = event.bind_by_arc();
//!
//! // Send a message through the event
//! sender.send("Hello, Arc!".to_string());
//!
//! // Receive the message
//! let message = receiver.await.unwrap();
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
//! let (sender, receiver) = event.bind_by_rc();
//!
//! // Send a message through the event
//! sender.send("Hello, Rc!".to_string());
//!
//! // Receive the message
//! let message = receiver.await.unwrap();
//! assert_eq!(message, "Hello, Rc!");
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
//! let value1 = receiver1.await.unwrap();
//! assert_eq!(value1, 42);
//! // Event automatically returned to pool when endpoints are dropped
//!
//! // Second usage - reuses the same event instance efficiently
//! let (sender2, receiver2) = pool.bind_by_ref();
//! sender2.send(100);
//! let value2 = receiver2.await.unwrap();
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
//! let value1 = receiver1.await.unwrap();
//! assert_eq!(value1, 42);
//!
//! // Second usage - efficiently reuses the same underlying event
//! let (sender2, receiver2) = pool.bind_by_ref();
//! sender2.send(200);
//! let value2 = receiver2.await.unwrap();
//! assert_eq!(value2, 200);
//! // Pool automatically manages event lifecycle and reuse
//! # });
//! ```
//!
//! # Pool Ownership Examples
//!
//! ## Arc-based Pool Sharing
//!
//! ```rust
//! use std::sync::Arc;
//!
//! use events::OnceEventPool;
//! # use futures::executor::block_on;
//!
//! # block_on(async {
//! // Create an Arc-wrapped pool for shared ownership across threads
//! let pool = Arc::new(OnceEventPool::<String>::new());
//! let (sender, receiver) = pool.bind_by_arc();
//!
//! sender.send("Hello from Arc pool!".to_string());
//! let message = receiver.await.unwrap();
//! assert_eq!(message, "Hello from Arc pool!");
//! # });
//! ```
//!
//! ## Rc-based Local Pool Sharing
//!
//! ```rust
//! use std::rc::Rc;
//!
//! use events::LocalOnceEventPool;
//! # use futures::executor::block_on;
//!
//! # block_on(async {
//! // Create an Rc-wrapped local pool for shared ownership (single-threaded)
//! let pool = Rc::new(LocalOnceEventPool::<String>::new());
//! let (sender, receiver) = pool.bind_by_rc();
//!
//! sender.send("Hello from Rc pool!".to_string());
//! let message = receiver.await.unwrap();
//! assert_eq!(message, "Hello from Rc pool!");
//! # });
//! ```

mod constants;
mod disconnected;
mod once;

pub(crate) use constants::*;
pub use disconnected::*;
pub use once::*;

trait Sealed {}

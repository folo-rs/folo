#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! Efficient oneshot events (channels) with support for single-threaded events,
//! object embedding, event pools and event lakes
//!
//! An event is a pair of a sender and receiver, where the sender can be used at most once. When
//! the event occurs, the sender submits a payload `T` to the receiver. Meanwhile, the receiver can
//! await the arrival of the payload.
//!
//! Inspired by [`oneshot`][1] which offers this functionality for basic use cases. The types in
//! this package expand on that further and provide additional features while maintaining the high
//! performance and low overhead characteristics of the original design:
//!
//! - **Single-threaded events**: Events that can only be used from a single thread, allowing for
//!   lower overhead and better performance where thread-safety is not required.
//! - **Object embedding**: Events can be embedded directly within other objects instead of
//!   being allocated on the heap. This reduces allocation overhead and improves cache locality.
//! - **Event pools**: Reusable pools of events that can be recycled to reduce heap memory
//!   allocation overhead and improve performance in high-throughput scenarios.
//! - **Event lakes**: Event pools for heterogeneous event types, allowing for efficient
//!   management and processing of diverse events that carry different types of payloads whose
//!   types are not known in advance.
//!
//! The events support both asynchronous awaiting and ad-hoc completion polling. Synchronous
//! waiting for event completion is not supported.
//!
//! # Basic usage
//!
//! The simplest way to create an event is to use [`Event::boxed()`] which allocates
//! an event on the heap and returns the sender/receiver pair.
//!
//! ```rust
//! use events_once::Event;
//!
//! #[tokio::main]
//! async fn main() {
//!     let (sender, receiver) = Event::<String>::boxed();
//!
//!     sender.send("Hello, world!".to_string());
//!
//!     // Events are thread-safe by default and their endpoints
//!     // may be freely moved to other threads or tasks.
//!     tokio::spawn(async move {
//!         let message = receiver.await.unwrap();
//!         println!("{message}");
//!     })
//!     .await
//!     .unwrap();
//! }
//! ```
//!
//! # Reusing event resources
//!
//! One-time events have high churn by design - they can only be used once, so new events must
//! constantly be created and old ones destroyed in a busy program. If the events live on the
//! heap, this can be quite expensive as memory allocation is costly.
//!
//! It can often be more efficient to reuse the resources used by the events, keeping around
//! a pool of events that can be reused over and over again, without allocating them anew
//! each time. To do this, create an [`EventPool<T>`] for the desired payload type `T`
//! and rent events from it on-demand.
//!
//! ```rust
//! use events_once::EventPool;
//!
//! #[tokio::main]
//! async fn main() {
//!     const CUSTOMER_COUNT: usize = 5;
//!
//!     let pool = EventPool::<String>::new();
//!
//!     for customer_index in 0..CUSTOMER_COUNT {
//!         let (tx, rx) = pool.rent();
//!
//!         tx.send(format!(
//!             "Customer {customer_index} has entered the building"
//!         ));
//!
//!         let message = rx.await.unwrap();
//!         println!("{message}");
//!
//!         // Both endpoints are dropped now and the event is returned to the pool.
//!         // The next iteration will reuse the resources associated with the first event.
//!     }
//! }
//! ```
//!
//! The `EventPool<T>` itself acts as a handle to a resource pool. You can cheaply clone it;
//! each clone from the same family will share the same pool of resources. It does not need
//! to outlive the rented events.
//!
//! # Reusing events with unknown payload types
//!
//! Similarly to pooling of events with a known payload type, it is also possible to pool events
//! when you do not know the payload types in advance (e.g. because they are defined via generic
//! type parameters).
//!
//! This is facilitated by [`EventLake`], which acts similar to an [`EventPool<T>`] but without
//! the `T` type parameter.
//!
//! ```rust
//! use std::fmt::Debug;
//!
//! use events_once::EventLake;
//!
//! #[tokio::main]
//! async fn main() {
//!     let lake = EventLake::new();
//!
//!     deliver_payload("Hello from the lake!", &lake).await;
//!     deliver_payload(42, &lake).await;
//! }
//!
//! async fn deliver_payload<T>(payload: T, lake: &EventLake)
//! where
//!     T: Send + Debug + 'static,
//! {
//!     let (tx, rx) = lake.rent::<T>();
//!
//!     tx.send(payload);
//!     let payload = rx.await.unwrap();
//!     println!("Received payload: {payload:?}");
//! }
//! ```
//!
//! The `EventLake` itself acts as a handle to a resource pool. You can cheaply clone it;
//! each clone from the same family will share the same pool of resources. It does not need
//! to outlive the rented events.
//!
//! # Manual event or lake lifetime management
//!
//! In high-performance scenarios, it can be beneficial to reduce the lifetime management overhead
//! associated with [`EventPool<T>`] and [`EventLake`] by providing guarantees about their lifetime
//! via unsafe code.
//!
//! If you are willing to guarantee that the pool/lake outlives all rented events, you can use the
//! [`RawEventPool<T>`] and [`RawEventLake`] types instead. These types offer an equivalent API as
//! their safe counterparts but come with lower overhead, as well as requiring unsafe code to rent
//! events.
//!
//! ```rust
//! use events_once::RawEventPool;
//!
//! #[tokio::main]
//! async fn main() {
//!     const CUSTOMER_COUNT: usize = 5;
//!
//!     let pool = Box::pin(RawEventPool::<String>::new());
//!
//!     for customer_index in 0..CUSTOMER_COUNT {
//!         // SAFETY: We promise the pool outlives both the returned endpoints.
//!         let (tx, rx) = unsafe { pool.as_ref().rent() };
//!
//!         tx.send(format!(
//!             "Customer {customer_index} has entered the building"
//!         ));
//!
//!         let message = rx.await.unwrap();
//!         println!("{message}");
//!
//!         // Both endpoints are dropped now and the event is returned to the pool.
//!         // The next iteration will reuse the resources associated with the first event.
//!     }
//! }
//! ```
//!
//! Unlike the regular [`EventPool`] and [`EventLake`], the raw variants do not implement `Clone`
//! and have unique ownership over the resources contained within.
//!
//! # Single-threaded events
//!
//! If you do not need thread-safety, you can use single-threaded events for additional efficiency.
//! The single-threaded types have `Local` in their names. All primitives offered by this package
//! come with single-threaded variants.
//!
//! ```rust
//! use events_once::LocalEvent;
//!
//! #[tokio::main(flavor = "current_thread")]
//! async fn main() {
//!     let (sender, receiver) = LocalEvent::<String>::boxed();
//!
//!     sender.send("Hello, world!".to_string());
//!
//!     let message = receiver.await.unwrap();
//!     println!("{message}");
//! }
//! ```
//!
//! # Embedding events in objects
//!
//! If the lifetime of an event is constrained to the lifetime of another object, it can be
//! valuable to merge both objects into a single allocation because heap memory allocation is a
//! relatively expensive operation that is best avoided.
//!
//! This requires unsafe code because you - the author of the code that will be using the events -
//! must provide the guarantee that the event will not outlive the object. The compiler cannot
//! guarantee this.
//!
//! To embed an event, define a field of type [`EmbeddedEvent<T>`] within your object. Then, use
//! [`Event::placed()`][Event::placed] to create the sender/receiver pair that will be connected
//! via the event in this container. The [`EmbeddedEvent<T>`] and its parent object must be pinned
//! for the lifetime of the endpoints.
//!
//! ```
//! use std::time::Duration;
//!
//! use events_once::{EmbeddedEvent, Event};
//! use pin_project::pin_project;
//!
//! #[pin_project]
//! struct Account {
//!     id: u64,
//!
//!     // Event triggered when the account has been prepared
//!     // and is ready for use by the customer's user agent.
//!     #[pin]
//!     ready_to_use: EmbeddedEvent<()>,
//! }
//!
//! #[tokio::main]
//! async fn main() {
//!     let mut account = Box::pin(Account {
//!         id: 42,
//!         ready_to_use: EmbeddedEvent::new(),
//!     });
//!
//!     // SAFETY: We promise that `account` lives longer than any of the endpoints returned.
//!     let (ready_tx, ready_rx) =
//!         unsafe { Event::placed(account.as_mut().project().ready_to_use) };
//!
//!     let prepare_account_task = tokio::spawn(async move {
//!         // Simulate some asynchronous work to prepare the account.
//!         tokio::time::sleep(Duration::from_millis(10)).await;
//!
//!         // Signal that the account is ready to use.
//!         ready_tx.send(());
//!     });
//!
//!     let use_account_task = tokio::spawn(async move {
//!         // Wait until the account is ready to use.
//!         ready_rx.await.unwrap();
//!
//!         println!("Account {} is ready to use!", account.id);
//!     });
//!
//!     // The safety promise we made requires that we keep the account alive for
//!     // at least as long as the events endpoints are alive. As we are now dropping
//!     // the account, we must also ensure that the two tasks using the endpoints
//!     // have completed first. We do not care about the result here, we just want
//!     // to ensure that they are done, so they could not possibly be referencing the
//!     // embedded event once we drop the account.
//!     drop(prepare_account_task.await);
//!     drop(use_account_task.await);
//! }
//! ```
//!
//! # Synchronous polling
//!
//! While the primary API is intended for `receiver.await` usage, there are scenarios where
//! synchronous polling is desirable. For these cases, the receiver provides the `into_value()`
//! method, which allows you to attempt to retrieve the value without awaiting.
//!
//! This method consumes the receiver. If there is no value available, it returns the original
//! receiver back to you for later use.
//!
//! ```rust
//! use events_once::{Event, IntoValueError};
//!
//! #[tokio::main]
//! async fn main() {
//!     let (sender, receiver) = Event::<String>::boxed();
//!
//!     // into_value() is designed for synchronous scenarios where you do not want to wait but
//!     // simply want to either obtain the received value or do nothing. First, we do nothing.
//!     //
//!     // If no value has been sent yet, into_value() returns Err(IntoValueError::Pending(self)).
//!     let IntoValueError::Pending(receiver) = receiver.into_value().unwrap_err() else {
//!         panic!("Received unexpected error instead of IntoValueError::Pending");
//!     };
//!
//!     sender.send("Hello, world!".to_string());
//!
//!     let message = receiver.into_value().unwrap();
//!
//!     println!("Received message: {message}");
//! }
//! ```
//!
//! [1]: https://crates.io/crates/oneshot

mod backtrace;
mod core;
mod disconnected;
mod lake;
mod pool;

pub use core::*;

#[cfg(debug_assertions)]
pub(crate) use backtrace::*;
pub use disconnected::*;
pub use lake::*;
pub use pool::*;

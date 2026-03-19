#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Async manual-reset and auto-reset events for multi-use signaling.
//!
//! This crate provides two families of event primitives:
//!
//! * **Manual-reset events** ([`ManualResetEvent`], [`LocalManualResetEvent`]) — a gate
//!   that, once set, releases all current and future awaiters until explicitly
//!   reset via [`reset()`][ManualResetEvent::reset].
//! * **Auto-reset events** ([`AutoResetEvent`], [`LocalAutoResetEvent`]) — a token
//!   dispenser that releases exactly one awaiter per
//!   [`set()`][AutoResetEvent::set] call.
//!
//! Each family comes in a thread-safe variant (`Send + Sync`) and a
//! single-threaded `Local` variant for improved efficiency when thread safety
//! is not required.
//!
//! Events are lightweight cloneable handles. All clones from the same family
//! share the same underlying state.
//!
//! # Manual-reset example
//!
//! ```
//! use events::ManualResetEvent;
//!
//! # futures::executor::block_on(async {
//! let event = ManualResetEvent::boxed();
//! let waiter = event.clone();
//!
//! event.set();
//! waiter.wait().await;
//! # });
//! ```
//!
//! # Auto-reset example
//!
//! ```
//! use events::AutoResetEvent;
//!
//! # futures::executor::block_on(async {
//! let event = AutoResetEvent::boxed();
//! event.set();
//! event.wait().await;
//!
//! // Signal was consumed.
//! assert!(!event.try_acquire());
//! # });
//! ```

mod auto_reset_event;
mod local_auto_reset_event;
mod local_manual_reset_event;
mod manual_reset_event;
mod waiter_list;

#[cfg(test)]
mod test_helpers;

pub use auto_reset_event::*;
pub use local_auto_reset_event::*;
pub use local_manual_reset_event::*;
pub use manual_reset_event::*;

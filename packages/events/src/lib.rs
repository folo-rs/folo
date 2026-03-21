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
//! #[tokio::main]
//! async fn main() {
//!     let event = ManualResetEvent::boxed();
//!     let setter = event.clone();
//!
//!     // Producer opens the gate from a background task.
//!     tokio::spawn(async move {
//!         setter.set();
//!     });
//!
//!     // Consumer waits for the gate to open.
//!     event.wait().await;
//!
//!     // The gate stays open — it must be explicitly closed.
//!     assert!(event.is_set());
//! }
//! ```
//!
//! # Auto-reset example
//!
//! ```
//! use events::AutoResetEvent;
//!
//! #[tokio::main]
//! async fn main() {
//!     let event = AutoResetEvent::boxed();
//!     let setter = event.clone();
//!
//!     // Producer signals from a background task.
//!     tokio::spawn(async move {
//!         setter.set();
//!     });
//!
//!     // Consumer waits for the signal.
//!     event.wait().await;
//!
//!     // Signal was consumed.
//!     assert!(!event.try_acquire());
//! }
//! ```

mod auto_reset_event;
mod local_auto_reset_event;
mod local_manual_reset_event;
mod manual_reset_event;
mod waiter_list;

#[cfg(test)]
mod test_helpers;

pub(crate) const NEVER_POISONED: &str = "we never panic while holding this lock";

pub use auto_reset_event::{AutoResetEvent, EmbeddedAutoResetEvent, RawAutoResetEvent};
pub use local_auto_reset_event::{
    EmbeddedLocalAutoResetEvent, LocalAutoResetEvent, RawLocalAutoResetEvent,
};
pub use local_manual_reset_event::{
    EmbeddedLocalManualResetEvent, LocalManualResetEvent, RawLocalManualResetEvent,
};
pub use manual_reset_event::{EmbeddedManualResetEvent, ManualResetEvent, RawManualResetEvent};

/// Future types returned by event `wait()` methods.
///
/// These futures are `!Unpin` and must be pinned before polling.
pub mod futures {
    pub use crate::auto_reset_event::{AutoResetWaitFuture, RawAutoResetWaitFuture};
    pub use crate::local_auto_reset_event::{
        LocalAutoResetWaitFuture, RawLocalAutoResetWaitFuture,
    };
    pub use crate::local_manual_reset_event::{
        LocalManualResetWaitFuture, RawLocalManualResetWaitFuture,
    };
    pub use crate::manual_reset_event::{ManualResetWaitFuture, RawManualResetWaitFuture};
}

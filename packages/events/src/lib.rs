#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Async manual-reset and auto-reset event primitives.
//!
//! This crate provides async event primitives:
//!
//! * **Manual-reset events** ([`ManualResetEvent`], [`LocalManualResetEvent`]) — a gate
//!   that, once set, releases all current and future awaiters until explicitly
//!   reset via [`reset()`][ManualResetEvent::reset].
//! * **Auto-reset events** ([`AutoResetEvent`], [`LocalAutoResetEvent`]) — a signal
//!   that releases at most one awaiter per
//!   [`set()`][AutoResetEvent::set] call, automatically consuming the
//!   signal when an awaiter is released.
//!
//! Each family comes in a thread-safe variant (`Send + Sync`) and a
//! single-threaded `Local` variant for improved efficiency when thread safety
//! is not required.
//!
//! # Boxed vs embedded storage
//!
//! Every event type can be created in two ways:
//!
//! * **[`boxed()`][AutoResetEvent::boxed]** — the event manages its
//!   own storage. Simple to use: the returned handle is `Clone` and
//!   can be shared freely. Best for most use cases.
//! * **[`embedded()`][AutoResetEvent::embedded]** — borrows
//!   caller-provided storage (`Embedded*` types) instead of
//!   allocating. This eliminates one allocation per event, which
//!   matters when events are created on a hot path or embedded
//!   inside other data structures. The caller must ensure the
//!   storage outlives all handles and wait futures, and the
//!   `embedded()` call is `unsafe` to reflect this contract.
//!
//! Events are lightweight cloneable handles. All clones from the same
//! [`boxed()`][AutoResetEvent::boxed] or
//! [`embedded()`][AutoResetEvent::embedded] origin share the same
//! underlying state.
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
//!     assert!(event.try_wait());
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
//!     assert!(!event.try_wait());
//! }
//! ```

mod auto;
mod local_auto;
mod local_manual;
mod manual;

#[cfg(test)]
mod test_helpers;

pub(crate) const NEVER_POISONED: &str = "we never panic while holding this lock";

pub use auto::{AutoResetEvent, EmbeddedAutoResetEvent, EmbeddedAutoResetEventRef};
pub use local_auto::{
    EmbeddedLocalAutoResetEvent, EmbeddedLocalAutoResetEventRef, LocalAutoResetEvent,
};
pub use local_manual::{
    EmbeddedLocalManualResetEvent, EmbeddedLocalManualResetEventRef, LocalManualResetEvent,
};
pub use manual::{EmbeddedManualResetEvent, EmbeddedManualResetEventRef, ManualResetEvent};

/// Future types returned by event `wait()` methods.
///
/// These futures must be pinned before polling.
pub mod futures {
    pub use crate::auto::{AutoResetWaitFuture, EmbeddedAutoResetWaitFuture};
    pub use crate::local_auto::{EmbeddedLocalAutoResetWaitFuture, LocalAutoResetWaitFuture};
    pub use crate::local_manual::{EmbeddedLocalManualResetWaitFuture, LocalManualResetWaitFuture};
    pub use crate::manual::{EmbeddedManualResetWaitFuture, ManualResetWaitFuture};
}

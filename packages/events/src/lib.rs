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
//! # Comparison with other crates
//!
//! The numbers below come from the `events_bench` and `contended_bench`
//! benchmarks shipped with this crate, run single-process on a many-core
//! Windows `x86_64` machine. They are intended to convey orders of magnitude,
//! not exact figures — rerun the benchmarks in your own environment if you
//! need precise measurements.
//!
//! [`event_listener`][el] is the closest async-only competitor.
//! [`rsevents`][rs] is a synchronous (blocking) implementation included as a
//! reference point on the non-blocking fast paths.
//!
//! [el]: https://crates.io/crates/event-listener
//! [rs]: https://crates.io/crates/rsevents
//!
//! ## Sync fast path (`set` + `try_wait`, no contention)
//!
//! | Implementation                  | Manual-reset | Auto-reset |
//! |---------------------------------|--------------|------------|
//! | `events` (thread-safe, boxed)   |       1.1 ns |     4.2 ns |
//! | `events` (thread-safe, embedded)|       1.1 ns |     4.1 ns |
//! | `events` (local, boxed)         |     < 1.0 ns |     2.8 ns |
//! | `rsevents` (sync)               |       1.3 ns |     4.0 ns |
//! | `event_listener`                |        61 ns |        n/a |
//!
//! ## Async fast path (create, pin and poll a `wait` future on a pre-set event)
//!
//! | Implementation                  | Manual-reset | Auto-reset |
//! |---------------------------------|--------------|------------|
//! | `events` (thread-safe, boxed)   |       6.4 ns |     7.8 ns |
//! | `events` (thread-safe, embedded)|       6.2 ns |     7.2 ns |
//! | `events` (local, boxed)         |       7.4 ns |    10.0 ns |
//! | `event_listener` (`Event`)      |        66 ns |        n/a |
//! | `event_listener` (`listener!`)  |        32 ns |        n/a |
//!
//! ## 100-waiter release
//!
//! | Implementation                  | Manual-reset | Auto-reset |
//! |---------------------------------|--------------|------------|
//! | `events` (thread-safe)          |       4.1 µs |     4.3 µs |
//! | `events` (local)                |       2.1 µs |     2.0 µs |
//! | `event_listener`                |       6.9 µs |        n/a |
//!
//! ## Contention scaling (`set` + `try_wait` per thread, manual-reset)
//!
//! | Threads | `events` | `event_listener` |
//! |---------|----------|------------------|
//! |       1 |   4.6 ns |           9.1 ns |
//! |       2 |    75 ns |           157 ns |
//! |       4 |   108 ns |           570 ns |
//!
//! ## Allocations
//!
//! * `events` performs **zero allocations** on every benchmarked hot path,
//!   including [`set()`][AutoResetEvent::set], [`try_wait()`][AutoResetEvent::try_wait]
//!   and polling [`wait()`][AutoResetEvent::wait] futures, regardless of how many
//!   waiters are registered.
//! * `event_listener` allocates one heap node per registered listener
//!   (~56 bytes per listener) plus per-call bookkeeping.
//! * `rsevents` is allocation-free but offers only blocking APIs and cannot
//!   be polled from an async context.
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

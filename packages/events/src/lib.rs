#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Async synchronization primitives with intrusive waiter lists.
//!
//! This crate provides several families of synchronization primitives:
//!
//! * **Manual-reset events** ([`ManualResetEvent`], [`LocalManualResetEvent`]) — a gate
//!   that, once set, releases all current and future awaiters until explicitly
//!   reset via [`reset()`][ManualResetEvent::reset].
//! * **Auto-reset events** ([`AutoResetEvent`], [`LocalAutoResetEvent`]) — a token
//!   dispenser that releases exactly one awaiter per
//!   [`set()`][AutoResetEvent::set] call.
//! * **Mutexes** ([`Mutex`], [`LocalMutex`]) — async mutual exclusion with
//!   `Deref`/`DerefMut` guards.
//! * **Semaphores** ([`Semaphore`], [`LocalSemaphore`]) — permit-based
//!   concurrency control with single and multi-permit acquire.
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
//!
//! # Performance
//!
//! Measured with Criterion on an x86-64 development machine. Lower is better.
//!
//! ## Mutex
//!
//! | Benchmark | `Mutex` | `LocalMutex` | tokio | async-lock |
//! |---|---|---|---|---|
//! | Round trip | **29 ns** | **2.4 ns** | 33 ns | 37 ns |
//! | Async poll ready | **35 ns** | **6.1 ns** | 52 ns | 66 ns |
//! | Many waiters (×100) | **6.5 µs** | **2.1 µs** | 13.5 µs | 26.5 µs |
//!
//! ## Semaphore
//!
//! | Benchmark | `Semaphore` | `LocalSemaphore` | tokio | async-lock |
//! |---|---|---|---|---|
//! | Round trip | **31 ns** | **5.8 ns** | 66 ns | 64 ns |
//! | Async poll ready | **37 ns** | **11 ns** | 46 ns | 41 ns |
//! | Many waiters (×100) | **16 µs** | **3.0 µs** | 12.6 µs | 22 µs |
//!
//! Run `cargo bench -p events` to reproduce.

mod auto;
mod local_auto;
mod local_manual;
mod local_mutex;
mod local_semaphore;
mod manual;
mod mutex;
mod semaphore;
mod waiter_list;

#[cfg(test)]
mod test_helpers;

pub(crate) const NEVER_POISONED: &str = "we never panic while holding this lock";

pub use auto::{AutoResetEvent, EmbeddedAutoResetEvent, RawAutoResetEvent};
pub use local_auto::{EmbeddedLocalAutoResetEvent, LocalAutoResetEvent, RawLocalAutoResetEvent};
pub use local_manual::{
    EmbeddedLocalManualResetEvent, LocalManualResetEvent, RawLocalManualResetEvent,
};
pub use local_mutex::{
    EmbeddedLocalMutex, LocalMutex, LocalMutexGuard, RawLocalMutex, RawLocalMutexGuard,
};
pub use local_semaphore::{
    EmbeddedLocalSemaphore, LocalSemaphore, LocalSemaphorePermit, RawLocalSemaphore,
    RawLocalSemaphorePermit,
};
pub use manual::{EmbeddedManualResetEvent, ManualResetEvent, RawManualResetEvent};
pub use mutex::{EmbeddedMutex, Mutex, MutexGuard, RawMutex, RawMutexGuard};
pub use semaphore::{
    EmbeddedSemaphore, RawSemaphore, RawSemaphorePermit, Semaphore, SemaphorePermit,
};

/// Future types returned by event and synchronization primitive
/// `wait()` / `lock()` methods.
///
/// These futures are `!Unpin` and must be pinned before polling.
pub mod futures {
    pub use crate::auto::{AutoResetWaitFuture, RawAutoResetWaitFuture};
    pub use crate::local_auto::{LocalAutoResetWaitFuture, RawLocalAutoResetWaitFuture};
    pub use crate::local_manual::{LocalManualResetWaitFuture, RawLocalManualResetWaitFuture};
    pub use crate::local_mutex::{LocalMutexLockFuture, RawLocalMutexLockFuture};
    pub use crate::local_semaphore::{LocalSemaphoreAcquireFuture, RawLocalSemaphoreAcquireFuture};
    pub use crate::manual::{ManualResetWaitFuture, RawManualResetWaitFuture};
    pub use crate::mutex::{MutexLockFuture, RawMutexLockFuture};
    pub use crate::semaphore::{RawSemaphoreAcquireFuture, SemaphoreAcquireFuture};
}

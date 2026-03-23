#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Async mutex and semaphore primitives with intrusive waiter lists.
//!
//! This crate provides async synchronization primitives built on the
//! [`waiter_list`] crate's intrusive doubly-linked waiter queue:
//!
//! * **Mutexes** ([`Mutex`], [`LocalMutex`]) — async mutual exclusion with
//!   `Deref`/`DerefMut` guards and FIFO fairness.
//! * **Semaphores** ([`Semaphore`], [`LocalSemaphore`]) — permit-based
//!   concurrency control with single and multi-permit acquire.
//!
//! Each family comes in a thread-safe variant (`Send + Sync`) and a
//! single-threaded `Local` variant for improved efficiency when thread
//! safety is not required.
//!
//! All primitives support boxed, embedded, and raw construction variants,
//! as well as non-blocking `try_lock()` / `try_acquire()` methods.
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
//! Run `cargo bench -p asynchroniz` to reproduce.

mod local_mutex;
mod local_semaphore;
mod mutex;
mod semaphore;

#[cfg(test)]
#[allow(dead_code, reason = "test utilities prepared for future use")]
mod test_helpers;

pub(crate) const NEVER_POISONED: &str = "we never panic while holding this lock";

pub use local_mutex::{
    EmbeddedLocalMutex, LocalMutex, LocalMutexGuard, RawLocalMutex, RawLocalMutexGuard,
};
pub use local_semaphore::{
    EmbeddedLocalSemaphore, LocalSemaphore, LocalSemaphorePermit, RawLocalSemaphore,
    RawLocalSemaphorePermit,
};
pub use mutex::{EmbeddedMutex, Mutex, MutexGuard, RawMutex, RawMutexGuard};
pub use semaphore::{
    EmbeddedSemaphore, RawSemaphore, RawSemaphorePermit, Semaphore, SemaphorePermit,
};

/// Future types returned by `lock()` and `acquire()` methods.
///
/// These futures are `!Unpin` and must be pinned before polling.
pub mod futures {
    pub use crate::local_mutex::{LocalMutexLockFuture, RawLocalMutexLockFuture};
    pub use crate::local_semaphore::{LocalSemaphoreAcquireFuture, RawLocalSemaphoreAcquireFuture};
    pub use crate::mutex::{MutexLockFuture, RawMutexLockFuture};
    pub use crate::semaphore::{RawSemaphoreAcquireFuture, SemaphoreAcquireFuture};
}

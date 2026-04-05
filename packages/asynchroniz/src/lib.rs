#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Async mutex and semaphore primitives.
//!
//! This crate provides async synchronization primitives:
//!
//! * **Mutexes** ([`Mutex`], [`LocalMutex`]) — async mutual exclusion with
//!   `Deref`/`DerefMut` guards.
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
//! | Round trip | **25 ns** | **1.6 ns** | 26 ns | 45 ns |
//! | Async poll ready | **28 ns** | **4.5 ns** | 40 ns | 44 ns |
//! | Many waiters (×100) | **5.2 µs** | **1.6 µs** | 10.5 µs | 23.6 µs |
//!
//! ## Semaphore
//!
//! | Benchmark | `Semaphore` | `LocalSemaphore` | tokio | async-lock |
//! |---|---|---|---|---|
//! | Round trip | **27 ns** | **5.0 ns** | 32 ns | 44 ns |
//! | Async poll ready | **31 ns** | **9.0 ns** | 37 ns | 48 ns |
//! | Many waiters (×100) | **7.3 µs** | **2.4 µs** | 10.4 µs | 21.5 µs |
//!
//! Run `cargo bench -p asynchroniz` to reproduce.

mod constants;
mod local_mutex;
mod local_semaphore;
mod mutex;
mod semaphore;

pub use local_mutex::*;
pub use local_semaphore::*;
pub use mutex::*;
pub use semaphore::*;

/// Future types returned by `lock()` and `acquire()` methods.
pub mod futures {
    pub use crate::local_mutex::{EmbeddedLocalMutexLockFuture, LocalMutexLockFuture};
    pub use crate::local_semaphore::{
        EmbeddedLocalSemaphoreAcquireFuture, LocalSemaphoreAcquireFuture,
    };
    pub use crate::mutex::{EmbeddedMutexLockFuture, MutexLockFuture};
    pub use crate::semaphore::{EmbeddedSemaphoreAcquireFuture, SemaphoreAcquireFuture};
}

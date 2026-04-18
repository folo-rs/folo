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
//! | Round trip | **10 ns** | **2.2 ns** | 29 ns | 47 ns |
//! | Async poll ready | **26 ns** | **6.8 ns** | 45 ns | 46 ns |
//! | Many waiters (×100) | **8.6 µs** | **2.2 µs** | 11.0 µs | 25.0 µs |
//!
//! ## Semaphore
//!
//! | Benchmark | `Semaphore` | `LocalSemaphore` | tokio | async-lock |
//! |---|---|---|---|---|
//! | Round trip | **15 ns** | **5.2 ns** | 35 ns | 52 ns |
//! | Async poll ready | **33 ns** | **11 ns** | 40 ns | 52 ns |
//! | Many waiters (×100) | **13.6 µs** | **7.9 µs** | 10.3 µs | 22.9 µs |
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

pub mod futures;

//! Worker pool that schedules tasks on the same processor that spawned them.
//!
//! This crate provides a worker pool where each task is executed on the same processor that
//! spawned it, ensuring optimal cache locality and minimizing cross-processor data movement.
//!
//! # Quick start
//!
//! ```rust
//! use vicinal::Pool;
//! # use futures::executor::block_on;
//!
//! # block_on(async {
//! let pool = Pool::new();
//! let scheduler = pool.scheduler();
//!
//! let task1 = scheduler.spawn(|| 42);
//! let _task2 = scheduler.spawn(|| println!("doing some stuff"));
//!
//! assert_eq!(task1.await, 42);
//!
//! let scheduler2 = scheduler.clone();
//! let task3 = scheduler.spawn(move || scheduler2.spawn(|| 55));
//!
//! assert_eq!(task3.await.await, 55);
//! # });
//! ```
//!
//! # Tradeoffs
//!
//! - **Single task latency on an idle pool** is prioritized. The expectation is that tasks are
//!   short-lived so that the pool is often idle.
//!
//! # Shutdown behavior
//!
//! When the [`Pool`] is dropped, it signals all worker threads to shut down and waits for any
//! currently-executing tasks to complete. Queued tasks that have not started execution are
//! abandoned.
//!
//! If you need to ensure all spawned tasks complete before shutdown, await their [`JoinHandle`]s
//! before dropping the pool.
//!
//! # Panics
//!
//! If a task panics, the panic is captured and re-thrown when the [`JoinHandle`] is awaited.
//!
//! # Platform support
//!
//! The package is tested on the following operating systems:
//!
//! * Windows 11 x64
//! * Windows Server 2022 x64
//! * Ubuntu 24.04 x64
//!
//! On non-Windows non-Linux platforms (e.g. mac OS), the package will not uphold the processor
//! locality guarantees, but will otherwise function correctly as a worker pool.

mod join_handle;
mod metrics;
mod pool;
mod processor_registry;
mod processor_state;
mod scheduler;
mod spin_free_mutex;
mod task;
mod worker;

pub use join_handle::*;
pub(crate) use pool::PoolInner;
pub use pool::*;
pub(crate) use processor_registry::ProcessorRegistry;
pub(crate) use processor_state::ProcessorState;
pub use scheduler::*;
pub(crate) use spin_free_mutex::SpinFreeMutex;
pub(crate) use task::*;
pub(crate) use worker::*;

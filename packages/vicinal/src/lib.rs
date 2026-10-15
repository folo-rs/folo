#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Worker pool with configurable processor affinity.
//!
//! Processor affinity is the set of processors on which a worker is permitted to run.
//!
//! A memory region groups processors with their local physical memory, as reported by
//! [`many_cpus`]. Access to memory local to a processor can be faster than access across regions.
//!
//! A caller submits a task through one of a pool's [`Scheduler`] spawn methods. The pool samples
//! the calling thread's current logical processor; this is the task's submitting processor.
//! Tasks execute synchronously on that processor by default.
//! [`WorkerPolicy`] can instead share workers across the caller's memory region or across
//! the system. Each policy starts its fixed worker count on first use of a destination by default.
//! [`PoolBuilder::eager`] initializes selected destinations during construction instead.
//!
//! # Quick start
//!
//! ```rust
//! # use std::error::Error;
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
//! assert_eq!(task1.await?, 42);
//!
//! let scheduler2 = scheduler.clone();
//! let task3 = scheduler.spawn(move || scheduler2.spawn(|| 55));
//!
//! assert_eq!(task3.await?.await?, 55);
//! # Ok::<(), Box<dyn Error>>(())
//! # }).unwrap();
//! ```
//!
//! # Memory regions and worker counts
//!
//! Systems with non-uniform memory access (NUMA) have processors connected to different physical
//! memory modules. A memory region represents one such locality group; many systems have only one.
//! Regions are identified by [`many_cpus::MemoryRegionId`], not by a task's allocations or data.
//!
//! With a regional [`WorkerPolicy`], submissions from processors in the same region share one
//! configured worker count. Eager construction or the first submission activates those workers;
//! further submissions from that region do not add workers. Each pool accounts for its regions
//! independently, and scheduler clones share their pool's workers.
//!
//! With [`WorkerPolicy::per_system`], all submitting processors share one configured
//! worker count per pool, regardless of memory region. Workers may move across regions but
//! remain limited to eagerly selected processors and those that have submitted accepted work.
//!
//! Worker placement is tracked separately from concurrency. The permitted set starts with the eager
//! selection or the first submitting processor and grows when another processor in the destination
//! submits accepted work. Worker concurrency
//! is configured explicitly, rather than calculated from this set or from a processor-time quota.
//! This controls where tasks run, not where their memory is allocated.
//!
//! # Eager initialization
//!
//! [`PoolBuilder::eager`] takes ownership of a [`many_cpus::ProcessorSet`] from the pool's hardware
//! instance. Construction waits for the configured workers in each selected destination to apply
//! affinity and complete [`PoolBuilder::thread_init`]. Regional workers use only the selected
//! processors in their region; system workers use the entire selection.
//!
//! Use the same processor set for eager selection and the application's submitting threads.
//! The selection is not a permanent limit: submissions from other eligible processors still
//! start missing destinations lazily or expand shared-worker affinity.
//!
//! Initializers may overlap and must not depend on construction returning, including to publish
//! the pool's scheduler. Eager construction leaves the calling thread's affinity unchanged.
//! It prepares workers, not task-specific allocations or caches.
//!
//! # Tradeoffs
//!
//! - **Single task latency on an idle pool** is prioritized. The expectation is that tasks are
//!   short-lived so that the pool is often idle.
//! - Shared workers use eagerly selected processors and accepted submitters in their destination.
//!   Using one stable, quota-appropriate set for selection and submissions keeps workers in it.
//!   Membership grows if new processors submit work and never shrinks. Quotas do not adjust counts.
//!   Fractional quotas and other process work can still cause throttling: operating-system
//!   delays after the process exceeds its processor-time allowance. See
//!   [avoiding operating system quota penalties] for guidance on choosing the processor set.
//! - Shared worker affinity expands at task boundaries, without preempting running tasks.
//!   Workers may migrate within their permitted set, even during one task.
//!   Task code must preserve worker affinity. Affinity does not bind allocations, move data
//!   or guarantee cache hits. Topology and restriction refresh are not supported.
//!
//! # Shutdown behavior
//!
//! When the [`Pool`] is dropped, it stops accepting work and waits for any
//! currently-executing tasks to complete. Queued tasks that have not started execution are
//! cancelled. Joined tasks return errors that retain their bodies and captures for recovery
//! through [`TaskError::into_body()`]. Fire-and-forget tasks release their captures.
//!
//! If you need to ensure all spawned tasks complete before shutdown, await their [`JoinHandle`]s
//! before dropping the pool.
//! Infrastructure failures log the failure and stop the entire pool. Queued joined work is
//! cancelled recoverably; queued fire-and-forget tasks release their captures.
//! The pool then remains poisoned and all further submissions panic synchronously.
//! Already executing tasks may finish; task panics do not poison the pool.
//!
//! # Task outcomes
//!
//! Awaiting a [`JoinHandle`] returns `Result<R, TaskError<R>>`:
//!
//! - **Success:** `Ok(value)` contains the task's return value.
//! - **Cancellation:** [`TaskError::is_cancel()`] identifies cancellation before execution.
//!   [`TaskError::into_body()`] recovers the original task body for execution elsewhere.
//! - **Panic:** [`TaskError::is_panic()`] identifies a task panic.
//!   [`TaskError::into_panic()`] recovers the original panic payload.
//!
//! Calling a recovery method for the other outcome returns the unchanged error.
//!
//! For reporting without recovery, [`TaskError::into_diagnostic()`] discards recovery data and
//! retains a thread-safe error, including its source chain and backtrace.
//!
//! Fire-and-forget tasks log task panics instead.
//!
//! # Platform support
//!
//! The package is tested on the following operating systems:
//!
//! * Windows 11 x64
//! * Windows Server 2022 x64
//! * Ubuntu 24.04 x64
//!
//! On non-Windows non-Linux platforms (e.g. macOS), the package retains functional worker-pool
//! behavior without physical locality guarantees. The fallback topology has one memory region.
//!
//! [avoiding operating system quota penalties]:
//!   https://docs.rs/many_cpus/latest/many_cpus/#avoiding-operating-system-quota-penalties

mod constants;
mod group_id;
mod group_registry;
mod join_handle;
mod metrics;
mod pool;
mod pool_inner;
mod scheduler;
mod task;
mod task_error;
mod task_panic;
mod worker;
mod worker_group;
mod worker_policy;

pub(crate) use constants::NEVER_POISONED;
pub(crate) use group_id::*;
pub(crate) use group_registry::*;
pub use join_handle::*;
pub use pool::*;
pub(crate) use pool_inner::PoolInner;
pub use scheduler::*;
pub(crate) use task::*;
pub use task_error::*;
pub(crate) use task_panic::*;
pub(crate) use worker::*;
pub(crate) use worker_group::*;
pub use worker_policy::*;

#[cfg(test)]
::testing::set_allocator!();

#[cfg(test)]
::testing::init_tracing!();

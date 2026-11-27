#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Worker pool for running closures with configurable processor locality.
//!
//! [`Pool::builder`] selects a worker policy and constructs a pool. The pool's [`Scheduler`]
//! accepts ordinary closures, returns a [`JoinHandle`] immediately and runs each closure once on
//! a worker thread. Await that handle when the result or completion matters; callers that do not
//! need a result can use the fire-and-forget spawn methods.
//!
//! By default, the worker policy follows the submitting thread's logical processor. Applications
//! can instead use [`policies::WorkersPerMemoryRegion`] or [`policies::WorkersPerSystem`] to
//! share a fixed worker count across a larger locality. Processor affinity controls which
//! processors a worker may run on. A memory region groups processors by local physical memory,
//! as reported by [`many_cpus`].
//!
//! Workers start lazily on first use by default. [`PoolBuilder::eager`] starts selected
//! destinations during construction, after the configured processors have been pinned. Custom
//! policy implementors use the types in [`worker_policy`]; applications that use the built-in
//! policies normally only need [`Pool`], [`PoolBuilder`], and [`policies`].
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
//! With [`policies::WorkersPerMemoryRegion`], submissions from processors in the same region share
//! one configured worker count. Eager construction or the first submission activates those workers;
//! further submissions from that region do not add workers. Each pool accounts for its regions
//! independently, and scheduler clones share their pool's workers.
//!
//! With [`policies::WorkersPerSystem`], all submitting processors share one configured worker
//! count per pool, regardless of memory region. Workers may move across regions but remain limited
//! to eagerly selected processors and those included during policy selection.
//!
//! Worker placement is tracked separately from concurrency. The permitted set starts with the eager
//! selection or the first submitting processor and grows when another processor in the destination
//! reaches policy selection. Worker concurrency
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
//! - Shared workers use eagerly selected processors and submitters included during policy selection.
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

mod build_error;
mod built_in_worker_policy;
mod constants;
mod group_id;
mod join_handle;
mod metrics;
pub mod policies;
mod pool;
mod pool_inner;
mod pool_runtime;
mod scheduler;
mod task;
mod task_error;
mod task_panic;
mod worker;
mod worker_core;
mod worker_group;
mod worker_group_id;
mod worker_groups;
pub mod worker_policy;

pub use build_error::*;
#[cfg(test)]
pub(crate) use built_in_worker_policy::{BoundBuiltInPolicy, PolicyKind};
pub(crate) use built_in_worker_policy::{BuiltInWorkerPolicy, validate_eager_processors};
pub(crate) use constants::NEVER_POISONED;
pub(crate) use group_id::*;
pub use join_handle::*;
pub use pool::*;
pub(crate) use pool_inner::PoolInner;
pub(crate) use pool_runtime::*;
pub use scheduler::*;
pub(crate) use task::*;
pub use task_error::*;
pub(crate) use task_panic::*;
pub(crate) use worker::BindingReservation;
pub(crate) use worker_core::*;
#[cfg(test)]
pub(crate) use worker_group::WorkerEntry;
pub(crate) use worker_group::{GroupState, WorkerGroupState};

#[cfg(test)]
::testing::set_allocator!();

#[cfg(test)]
::testing::init_tracing!();

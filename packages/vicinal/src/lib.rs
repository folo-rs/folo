#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Worker pool for running closures with configurable processor locality.
//!
//! A [`Pool`] owns worker threads and the queues shared by its cloned [`Scheduler`] values.
//! Submitting a closure returns a [`JoinHandle`] immediately, while a worker runs the closure
//! independently on a worker thread. Await the handle when the result or completion matters.
//! Use a fire-and-forget spawn method when the result does not matter.
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
//! # Choosing where work runs
//!
//! Systems with non-uniform memory access (NUMA) have processors connected to different physical
//! memory modules. [`many_cpus`](https://docs.rs/many_cpus/latest/many_cpus/) represents these
//! locality groups as memory regions. A system with uniform memory access may contain only one
//! region.
//!
//! A worker policy defines how tasks are distributed between worker threads and on which
//! processors those threads may execute. The default processor-local policy creates worker
//! threads for the processor that submits the first task. Work submitted from another processor
//! uses another set of worker threads created for that processor. Implement custom policies with
//! the types in [`worker_policy`].
//!
//! [`policies::WorkersPerMemoryRegion`] creates one set of worker threads per submitting region.
//! Submissions from processors in that region share those workers.
//! [`policies::WorkersPerSystem`] creates one set of worker threads for the entire pool, so the
//! same workers handle submissions from every region.
//!
//! Placement and concurrency are separate choices. The worker count controls how many tasks may
//! execute at once. Processor affinity controls where those worker threads may execute. Regional
//! and system-wide workers begin with the eager selection or first submitting processor, then
//! become allowed to run on additional eligible processors as those processors submit work.
//! Expanding the allowed processors does not add worker threads. Affinity does not move task
//! allocations or guarantee cache hits.
//!
//! # Starting workers
//!
//! Worker threads start lazily when their processor, region or pool first receives work. This
//! avoids creating threads an application never uses. Scheduler clones share the same pool and
//! therefore share its workers.
//!
//! [`PoolBuilder::eager`] takes ownership of a
//! [`many_cpus::ProcessorSet`](https://docs.rs/many_cpus/latest/many_cpus/struct.ProcessorSet.html)
//! and starts every worker required by the selected processors, regions or system-wide workers.
//! It waits for each worker to apply its initial affinity and finish the configured
//! [`PoolBuilder::thread_init`] callback.
//!
//! For optimal task locality, use the same processor set for eager selection and the application's
//! submitting threads.
//! Later submissions from other eligible processors still start missing processor-local
//! workers or expand the processors on which shared workers may run.
//!
//! # Receiving task results
//!
//! Awaiting a [`JoinHandle`] returns `Result<R, TaskError<R>>`:
//!
//! - **Success:** `Ok(value)` contains the task's return value.
//! - **Cancellation:** [`TaskError::is_cancel()`] identifies cancellation before execution.
//!   [`TaskError::into_body()`] recovers the original task body for execution elsewhere.
//! - **Panic:** [`TaskError::is_panic()`] identifies a task panic.
//!   [`TaskError::into_panic()`] recovers the original panic payload.
//!
//! Calling a recovery method for the other outcome returns the unchanged error. For reporting
//! without recovery, [`TaskError::into_diagnostic()`] discards recovery data and retains a
//! thread-safe error with its source chain and backtrace. Fire-and-forget tasks log task panics
//! instead.
//!
//! # Shutting down
//!
//! Dropping a [`Pool`] stops admission, waits for executing tasks and cancels tasks that have not
//! started. Await joined tasks before dropping the pool if every submitted task must complete.
//! Retained schedulers do not keep worker threads alive, and submissions after clean shutdown
//! return cancelled tasks.
//!
//! # Handling errors
//!
//! Worker-infrastructure failures stop and permanently poison the entire pool. Queued joined
//! tasks return cancellation errors that retain their bodies; queued fire-and-forget tasks release
//! their captures. Executing tasks may finish. Further submissions panic synchronously. A task
//! panic remains a task result and does not poison the pool.
//!
//! # Choosing a processor set
//!
//! The pool favors low latency for one task on an idle pool and assumes that tasks are usually
//! short-lived. The worker count is explicit; processor-time quotas do not reduce or multiply it.
//!
//! Prefer one stable, quota-appropriate processor set for both eager selection and application
//! submission. Shared workers may grow their permitted set when a new processor submits work, but
//! the set never shrinks. Fractional quotas and unrelated process work can still cause operating
//! system throttling after the process exceeds its processor-time allowance. See
//! [avoiding operating system quota penalties] for guidance.
//!
//! Workers refresh shared affinity at task boundaries. A running task keeps its previous affinity
//! until it reaches the next boundary. Task code must preserve worker affinity. Affinity controls
//! execution placement only: it does not bind allocations, move data or guarantee cache hits.
//! The pool does not refresh topology or operating-system restrictions after construction.
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
mod worker_groups;
pub mod worker_policy;

pub(crate) use build_error::EagerProcessorHardwareMismatch;
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
pub(crate) use worker_group::WorkerGroupState;

#[cfg(test)]
::testing::set_allocator!();

#[cfg(test)]
::testing::init_tracing!();

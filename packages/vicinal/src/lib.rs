#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Worker pool with configurable processor affinity.
//!
//! A memory region groups processors with their local physical memory, as reported by
//! [`many_cpus`]. Access to memory local to a processor can be faster than access across regions.
//!
//! Tasks execute synchronously on the processor sampled during submission by default.
//! [`WorkerPolicy`] can instead share workers across the caller's memory region or across
//! the system. Each policy starts its fixed worker count lazily on first use of a destination.
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
//! # Memory regions and worker counts
//!
//! Systems with non-uniform memory access (NUMA) have processors connected to different physical
//! memory modules. A memory region represents one such locality group; many systems have only one.
//! Regions are identified by [`many_cpus::MemoryRegionId`], not by a task's allocations or data.
//!
//! With a regional [`WorkerPolicy`], submissions from processors in the same region share one
//! configured worker count. The first submission activates those workers; further submissions
//! from that region do not add workers. Each pool accounts for its regions independently,
//! and scheduler clones share their pool's workers.
//!
//! With [`WorkerPolicy::fixed_per_system`], all submitting processors share one configured
//! worker count per pool, regardless of memory region. Workers may move across regions but
//! remain limited to processors that have submitted accepted work to that pool.
//!
//! Worker placement is tracked separately from concurrency. Workers use only processors that have
//! submitted accepted work to that pool and destination. This set starts with the first submitter
//! and grows when another processor in the destination submits work. Worker concurrency is configured
//! explicitly, rather than calculated from this set or from a processor-time quota.
//! This controls where tasks run, not where their memory is allocated.
//!
//! # Tradeoffs
//!
//! - **Single task latency on an idle pool** is prioritized. The expectation is that tasks are
//!   short-lived so that the pool is often idle.
//! - Shared workers use only processors that submitted accepted work to their pool and destination.
//!   A stable, quota-appropriate submitter set keeps worker placement within that set; membership
//!   grows if new processors submit work and never shrinks. Quotas do not adjust worker counts.
//!   Fractional quotas and other process work can still cause throttling: operating-system
//!   delays after the process exceeds its processor-time allowance. See
//!   [avoiding operating system quota penalties] for guidance on choosing the submitter set.
//! - Shared worker affinity expands at task boundaries, without preempting running tasks.
//!   Workers may migrate within their permitted set, even during one task.
//!   Task code must preserve worker affinity. Affinity does not bind allocations, move data
//!   or guarantee cache hits. Topology and restriction refresh are not supported.
//!
//! # Shutdown behavior
//!
//! When the [`Pool`] is dropped, it stops accepting work and waits for any
//! currently-executing tasks to complete. Queued tasks that have not started execution are
//! abandoned.
//!
//! If you need to ensure all spawned tasks complete before shutdown, await their [`JoinHandle`]s
//! before dropping the pool.
//! Infrastructure failures log the failure and stop the entire pool, abandoning queued work.
//! The pool then remains poisoned and all further submissions panic synchronously.
//! Already executing tasks may finish; task panics do not poison the pool.
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
pub(crate) use worker::*;
pub(crate) use worker_group::*;
pub use worker_policy::*;

#[cfg(test)]
::testing::set_allocator!();

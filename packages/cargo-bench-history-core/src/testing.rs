//! In-workspace test helpers exposed to the `cargo-bench-history` shell crate.
//!
//! Gated behind the `private-test-util` feature, so end-user builds never see it.

#![cfg_attr(coverage_nightly, coverage(off))]

use anyspawn::{BoxedBlockingTask, BoxedFuture, SpawnCustom, Spawner, ThreadAwareAsyncFnOnce};
use thread_aware::ThreadAware;

/// A [`Spawner`] that runs every dispatched blocking task inline on the calling
/// thread rather than on a runtime's blocking pool.
///
/// The analysis distributes its detection work through [`Spawner::spawn_blocking`];
/// production injects a Tokio spawner, but tests and Miri inject this one so they
/// need no async runtime and stay deterministic. Each task runs to completion the
/// moment it is spawned, so the returned join handle resolves immediately and a
/// panic in a task propagates straight to the caller.
#[derive(Clone, ThreadAware)]
struct SynchronousSpawner;

impl SpawnCustom for SynchronousSpawner {
    #[cfg_attr(test, mutants::skip)] // Unreachable: the analysis distributes only blocking work.
    fn spawn(&self, _task: BoxedFuture) {
        unreachable!("the analysis distributes only blocking work")
    }

    #[cfg_attr(test, mutants::skip)] // Unreachable: the analysis distributes only blocking work.
    fn spawn_anywhere(&self, _task: Box<dyn ThreadAwareAsyncFnOnce<()>>) {
        unreachable!("the analysis distributes only blocking work")
    }

    fn spawn_blocking(&self, task: BoxedBlockingTask) {
        task();
    }
}

/// Builds a [`Spawner`] that runs blocking tasks inline on the calling thread.
///
/// Tests and Miri inject this where production injects a Tokio-backed
/// `Spawner::new_tokio`, so the spawner-distributed analysis runs without a Tokio
/// runtime and stays deterministic.
pub fn synchronous_spawner() -> Spawner {
    Spawner::new_custom("synchronous", SynchronousSpawner)
}

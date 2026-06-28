//! In-workspace test helpers exposed to the `cargo-bench-history` shell crate.
//!
//! Gated behind the `private-test-util` feature, so end-user builds never see it.

#![cfg_attr(coverage_nightly, coverage(off))]

use std::task::{Context, Poll, Waker};

use anyspawn::{BoxedBlockingTask, BoxedFuture, SpawnCustom, Spawner, ThreadAwareAsyncFnOnce};
use thread_aware::ThreadAware;

/// A [`Spawner`] that runs every dispatched task inline on the calling thread
/// rather than on a runtime's worker or blocking pool.
///
/// The analysis distributes its detection work through [`Spawner::spawn_blocking`]
/// and its object-load work through [`Spawner::spawn`]; production injects a Tokio
/// spawner, but tests and Miri inject this one so they need no async runtime and
/// stay deterministic. Each task runs to completion the moment it is spawned, so
/// the returned join handle resolves immediately and a panic in a task propagates
/// straight to the caller.
#[derive(Clone, ThreadAware)]
struct SynchronousSpawner;

impl SpawnCustom for SynchronousSpawner {
    fn spawn(&self, mut task: BoxedFuture) {
        // The analysis's spawned load tasks await only the injected in-memory
        // storage, which is always immediately ready, so a single poll with a
        // no-op waker drives the future to completion. Running it inline keeps the
        // tests free of an async runtime (and Miri-safe), exactly like
        // `spawn_blocking`, and avoids nesting an executor inside the test's own
        // `block_on`.
        let mut context = Context::from_waker(Waker::noop());
        match task.as_mut().poll(&mut context) {
            Poll::Ready(()) => {}
            Poll::Pending => unreachable!(
                "the synchronous spawner drives only immediately-ready in-memory \
                 futures, which never yield Pending"
            ),
        }
    }

    #[cfg_attr(test, mutants::skip)] // Unreachable: the analysis never spawns relocatable async tasks.
    fn spawn_anywhere(&self, _task: Box<dyn ThreadAwareAsyncFnOnce<()>>) {
        unreachable!("the analysis never spawns relocatable async tasks")
    }

    fn spawn_blocking(&self, task: BoxedBlockingTask) {
        task();
    }
}

/// Builds a [`Spawner`] that runs tasks inline on the calling thread.
///
/// Tests and Miri inject this where production injects a Tokio-backed
/// `Spawner::new_tokio`, so the spawner-distributed analysis runs without a Tokio
/// runtime and stays deterministic.
pub fn synchronous_spawner() -> Spawner {
    Spawner::new_custom("synchronous", SynchronousSpawner)
}

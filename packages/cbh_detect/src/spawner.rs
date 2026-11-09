use std::future::Future;
#[cfg(any(test, feature = "private-test-util"))]
use std::task::{Context, Poll, Waker};

/// Dispatches analysis work through a runtime or an in-process test executor.
///
/// The analysis owns this boundary so its deterministic tests need no runtime.
/// Runtime task failures propagate as panics rather than incomplete analysis results.
#[derive(Clone, Debug)]
pub struct Spawner {
    kind: SpawnerKind,
}

impl Spawner {
    /// Connects analysis dispatch to the command's selected runtime.
    #[must_use]
    pub fn new(runtime: anyspawn::Spawner) -> Self {
        Self {
            kind: SpawnerKind::Runtime(runtime),
        }
    }

    /// Executes immediately-ready test work without a runtime.
    #[cfg(any(test, feature = "private-test-util"))]
    pub(crate) fn synchronous() -> Self {
        Self {
            kind: SpawnerKind::Synchronous,
        }
    }

    /// Starts an object-loading task and returns its completion future.
    pub fn spawn<F>(&self, work: F) -> impl Future<Output = F::Output> + use<F>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let task = match &self.kind {
            SpawnerKind::Runtime(runtime) => Task::Runtime(runtime.spawn(work)),
            #[cfg(any(test, feature = "private-test-util"))]
            SpawnerKind::Synchronous => {
                let mut work = Box::pin(work);
                let mut context = Context::from_waker(Waker::noop());
                match work.as_mut().poll(&mut context) {
                    Poll::Ready(value) => Task::Synchronous(value),
                    // Fake storage completes inline; accepting Pending would hide a
                    // runtime dependency in tests. Ref: docs/implementation.md.
                    Poll::Pending => {
                        panic!("the synchronous spawner requires immediately-ready futures")
                    }
                }
            }
        };
        task.join()
    }

    /// Starts a detection chunk and returns its completion future.
    pub fn spawn_blocking<T, F>(&self, work: F) -> impl Future<Output = T> + use<T, F>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let task = match &self.kind {
            SpawnerKind::Runtime(runtime) => Task::Runtime(runtime.spawn_blocking(work)),
            #[cfg(any(test, feature = "private-test-util"))]
            SpawnerKind::Synchronous => Task::Synchronous(work()),
        };
        task.join()
    }
}

/// Keeps the test executor independent of runtime-specific construction.
#[derive(Clone, Debug)]
enum SpawnerKind {
    Runtime(anyspawn::Spawner),
    #[cfg(any(test, feature = "private-test-util"))]
    Synchronous,
}

/// Retains eagerly dispatched work until analysis recombines it in input order.
enum Task<T: Send + 'static> {
    Runtime(anyspawn::JoinHandle<T>),
    #[cfg(any(test, feature = "private-test-util"))]
    Synchronous(T),
}

impl<T: Send + 'static> Task<T> {
    /// Preserves the analysis contract that every dispatched chunk completes.
    async fn join(self) -> T {
        match self {
            Self::Runtime(handle) => handle
                .await
                .unwrap_or_else(|error| panic!("analysis task failed: {error:?}")),
            #[cfg(any(test, feature = "private-test-util"))]
            Self::Synchronous(value) => value,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use futures::executor::block_on;

    use super::*;

    #[test]
    fn synchronous_async_dispatch_completes_before_joining() {
        let spawner = Spawner::synchronous();
        let completed = Arc::new(AtomicBool::new(false));
        let task = spawner.spawn({
            let completed = Arc::clone(&completed);
            async move {
                // No other data is published; only this completion flag is observed.
                completed.store(true, Ordering::Relaxed);
            }
        });
        assert!(completed.load(Ordering::Relaxed));
        block_on(task);
    }

    #[test]
    fn synchronous_blocking_dispatch_completes_before_joining() {
        let spawner = Spawner::synchronous();
        let completed = Arc::new(AtomicBool::new(false));
        let task = spawner.spawn_blocking({
            let completed = Arc::clone(&completed);
            move || {
                // No other data is published; only this completion flag is observed.
                completed.store(true, Ordering::Relaxed);
            }
        });
        assert!(completed.load(Ordering::Relaxed));
        block_on(task);
    }

    #[test]
    fn synchronous_dispatch_returns_task_values() {
        let spawner = Spawner::synchronous();
        assert_eq!(block_on(spawner.spawn(async { "loaded" })), "loaded");
        assert_eq!(block_on(spawner.spawn_blocking(|| "detected")), "detected");
    }

    #[test]
    fn synchronous_dispatch_rejects_pending_work() {
        testing::assert_panics(|| {
            drop(Spawner::synchronous().spawn(std::future::pending::<()>()));
        });
    }

    #[test]
    fn synchronous_dispatch_propagates_panics() {
        testing::assert_panics(|| {
            drop(Spawner::synchronous().spawn(async { panic!("task panic") }));
        });
        testing::assert_panics(|| {
            drop(Spawner::synchronous().spawn_blocking(|| panic!("task panic")));
        });
    }
}

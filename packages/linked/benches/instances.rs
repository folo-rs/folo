//! Basic operations on the `instances!` macro and underlying type.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, LazyLock};

use criterion::{Criterion, criterion_group, criterion_main};
use many_cpus::ProcessorSet;
use new_zealand::nz;
use par_bench::{Run, ThreadPool};
use seq_macro::seq;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

#[expect(
    dead_code,
    reason = "We do not care about using all the fields but we want to pay the price of initializing them"
)]
#[linked::object]
struct TestSubject {
    local_state: AtomicUsize,
    shared_state: Arc<AtomicUsize>,
}

impl TestSubject {
    fn new() -> Self {
        let shared_state = Arc::new(AtomicUsize::new(0));

        linked::new!(Self {
            local_state: AtomicUsize::new(0),
            shared_state: Arc::clone(&shared_state),
        })
    }
}

linked::instances!(static TARGET: TestSubject = TestSubject::new());

static ONE_PROCESSOR: LazyLock<ProcessorSet> =
    LazyLock::new(|| ProcessorSet::builder().take(nz!(1)).unwrap());
static TWO_PROCESSORS: LazyLock<Option<ProcessorSet>> =
    LazyLock::new(|| ProcessorSet::builder().take(nz!(2)));

static ONE_THREAD: LazyLock<ThreadPool> = LazyLock::new(|| ThreadPool::new(&ONE_PROCESSOR));
static TWO_THREADS: LazyLock<Option<ThreadPool>> =
    LazyLock::new(|| TWO_PROCESSORS.as_ref().map(ThreadPool::new));

fn entrypoint(c: &mut Criterion) {
    let mut g = c.benchmark_group("instances::get");

    g.bench_function("one-threaded", |b| {
        b.iter_custom(|iters| {
            Run::builder()
                .iter_fn(|()| Arc::weak_count(&TARGET.get().shared_state))
                .build()
                .execute_on(&ONE_THREAD, iters)
                .mean_duration()
        });
    });

    if let Some(two_threads) = &*TWO_THREADS {
        g.bench_function("two-threaded", |b| {
            b.iter_custom(|iters| {
                Run::builder()
                    .iter_fn(|()| Arc::weak_count(&TARGET.get().shared_state))
                    .build()
                    .execute_on(two_threads, iters)
                    .mean_duration()
            });
        });
    }

    g.finish();

    let mut g = c.benchmark_group("instances::get_1000");

    g.bench_function("one-threaded", |b| {
        b.iter_custom(|iters| {
            Run::builder()
                // We have to clean up the worker thread before/after the run.
                .prepare_thread_fn(|_| LinkedVariableClearGuard::default())
                .iter_fn(|()| {
                    seq!(N in 0..1000 {
                        black_box(Arc::weak_count(&TARGET_MANY_~N.get().shared_state));
                    });
                })
                .build()
                .execute_on(&ONE_THREAD, iters)
                .mean_duration()
        });
    });

    if let Some(two_threads) = &*TWO_THREADS {
        g.bench_function("two-threaded", |b| {
            b.iter_custom(|iters| {
                Run::builder()
                    // We have to clean up the worker thread before/after the run.
                    .prepare_thread_fn(|_| LinkedVariableClearGuard::default())
                    .iter_fn(|()| {
                        seq!(N in 0..1000 {
                            black_box(Arc::weak_count(&TARGET_MANY_~N.get().shared_state));
                        });
                    })
                    .build()
                    .execute_on(two_threads, iters)
                    .mean_duration()
            });
        });
    }

    g.finish();
}

// We manually expand the macro here just because macro-in-macro goes crazy and fails to operate.
seq!(N in 0..1000 {
    #[expect(non_camel_case_types, reason = "manually replicating uglified macro internals for benchmark")]
    struct __lookup_key_~N;

    const TARGET_MANY_~N : ::linked::StaticInstances<TestSubject> =
        ::linked::StaticInstances::new(
            ::std::any::TypeId::of::<__lookup_key_~N>,
            TestSubject::new
        );
});

/// Clears all data stored in the shared variable system when created and dropped. Just for testing.
#[derive(Debug)]
struct LinkedVariableClearGuard {}

impl Default for LinkedVariableClearGuard {
    fn default() -> Self {
        ::linked::__private_clear_linked_variables();
        Self {}
    }
}

impl Drop for LinkedVariableClearGuard {
    fn drop(&mut self) {
        ::linked::__private_clear_linked_variables();
    }
}

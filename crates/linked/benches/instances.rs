//! Basic operations on the `instances!` macro and underlying type.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use benchmark_utils::{ThreadPool, bench_on_threadpool};
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
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

fn entrypoint(c: &mut Criterion) {
    let thread_pool = ThreadPool::default();

    let mut g = c.benchmark_group("instances::get");

    g.bench_function("single-threaded", |b| {
        b.iter(|| black_box(Arc::weak_count(&TARGET.get().shared_state)));
    });

    g.bench_function("multi-threaded", |b| {
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &thread_pool,
                iters,
                || (),
                |()| {
                    black_box(Arc::weak_count(&TARGET.get().shared_state));
                },
            )
        });
    });

    g.finish();

    let mut g = c.benchmark_group("instances::get_1000");

    g.bench_function("single-threaded", |b| {
        b.iter_batched_ref(
            LinkedVariableClearGuard::default,
            |_| {
                seq!(N in 0..1000 {
                    black_box(Arc::weak_count(&TARGET_MANY_~N.get().shared_state));
                });
            },
            BatchSize::SmallInput,
        );
    });

    g.bench_function("multi-threaded", |b| {
        b.iter_custom(|iters| {
            let duration = bench_on_threadpool(
                &thread_pool,
                iters,
                || (),
                |()| {
                    seq!(N in 0..1000 {
                        black_box(Arc::weak_count(&TARGET_MANY_~N.get().shared_state));
                    });
                },
            );

            // The other threads were all temporary and have already gone away, so all we care about
            // is destroying the remains in the global registry, which is fine from this thread.
            linked::__private_clear_linked_variables();

            duration
        });
    });

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

//! Basic operations on the `instance_per_access!` macro and underlying type.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::{
    cell::Cell,
    hint::black_box,
    sync::{Arc, atomic::AtomicUsize},
};

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
    local_state: Cell<usize>,
    shared_state: Arc<AtomicUsize>,
}

impl TestSubject {
    fn new() -> Self {
        let shared_state = Arc::new(AtomicUsize::new(0));

        linked::new!(Self {
            local_state: Cell::new(0),
            shared_state: Arc::clone(&shared_state),
        })
    }
}

linked::instance_per_access!(static TARGET: TestSubject = TestSubject::new());

fn entrypoint(c: &mut Criterion) {
    let thread_pool = ThreadPool::all();

    let mut g = c.benchmark_group("per_access_static::get");

    g.bench_function("single-threaded", |b| {
        b.iter(|| black_box(TARGET.get().local_state.get()));
    });

    g.bench_function("multi-threaded", |b| {
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &thread_pool,
                iters,
                || (),
                |()| {
                    black_box(TARGET.get().local_state.get());
                },
            )
        });
    });

    g.finish();

    let mut g = c.benchmark_group("per_access_static::get_1000");

    g.bench_function("single-threaded", |b| {
        b.iter_batched_ref(
            LinkedVariableClearGuard::default,
            |_| {
                seq!(N in 0..1000 {
                    black_box(TARGET_MANY_~N.get().local_state.get());
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
                        black_box(TARGET_MANY_~N.get().local_state.get());
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

    const TARGET_MANY_~N : ::linked::PerAccessStatic<TestSubject> =
        ::linked::PerAccessStatic::new(
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

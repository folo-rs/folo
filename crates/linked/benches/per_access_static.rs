use std::{
    cell::Cell,
    hint::black_box,
    sync::{Arc, atomic::AtomicUsize},
};

use benchmark_utils::bench_on_every_processor;
use criterion::{Criterion, criterion_group, criterion_main};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

// We do not necessarily care about using all the fields, but we want to pay the price of initializing them.
#[allow(dead_code)]
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
    let mut g = c.benchmark_group("per_access_static::access_single_threaded");

    g.bench_function("get", |b| {
        b.iter(|| black_box(TARGET.get().local_state.get()));
    });

    g.finish();

    let mut g = c.benchmark_group("per_access_static::access_multi_threaded");

    g.bench_function("get", |b| {
        b.iter_custom(|iters| {
            bench_on_every_processor(
                iters,
                || (),
                |_| {
                    black_box(TARGET.get().local_state.get());
                },
            )
        });
    });

    g.finish();
}

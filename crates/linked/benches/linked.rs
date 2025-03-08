// Copyright (c) Microsoft Corporation.
// Copyright (c) Folo authors.

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use seq_macro::seq;

criterion_group!(benches, linked_block_mutation, linked_block_allocation);
criterion_main!(benches);

#[linked::object]
struct Payload {
    local_value: usize,
}

impl Payload {
    fn new() -> Payload {
        linked::new!(Self { local_value: 0 })
    }

    fn mutate(&mut self) {
        self.local_value = self.local_value.saturating_add(1);
    }
}

linked::instance_per_access! {
    static PAYLOAD: Payload = Payload::new();
}

fn linked_block_mutation(c: &mut Criterion) {
    let mut group = c.benchmark_group("linked_block_mutation");

    group.bench_function("get_reused_instance", |b| {
        b.iter_batched_ref(
            || PAYLOAD.get(),
            |instance| {
                instance.mutate();
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("get_fresh_instance", |b| {
        b.iter(|| PAYLOAD.get().mutate());
    });

    group.finish();
}

// We manually expand the link! macro here just because macro-in-macro goes crazy here.
seq!(N in 0..1000 {
    #[allow(non_camel_case_types)]
    struct __lookup_key_~N;

    const PAYLOAD_MANY_~N : ::linked::PerAccessProvider<Payload> =
        ::linked::PerAccessProvider::new(
            ::std::any::TypeId::of::<__lookup_key_~N>,
            Payload::new
        );
});

fn linked_block_allocation(c: &mut Criterion) {
    let mut group = c.benchmark_group("linked_block_allocation");

    group.bench_function("1000_variables", |b| {
        b.iter_batched_ref(
            LinkedVariableClearGuard::default,
            |_| {
                seq!(N in 0..1000 {
                    PAYLOAD_MANY_~N.get().mutate();
                });
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

/// Clears all data stored in the shared variable system when created and dropped. Just for testing.
pub struct LinkedVariableClearGuard {}

impl Default for LinkedVariableClearGuard {
    fn default() -> Self {
        linked::__private_clear_linked_variables();
        Self {}
    }
}

impl Drop for LinkedVariableClearGuard {
    fn drop(&mut self) {
        linked::__private_clear_linked_variables();
    }
}

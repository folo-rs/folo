//! Benchmarking hardware information operations exposed by `SystemHardware`.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use criterion::{Criterion, criterion_group, criterion_main};
use many_cpus::SystemHardware;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    let hw = SystemHardware::current();

    let mut group = c.benchmark_group("SystemHardware_HardwareInfo");

    // Mostly pointless since all the accessors just load from cached values.
    // Just here to detect anomalies if we do something strange and it gets slow.
    group.bench_function("max_processor_id", |b| {
        b.iter(|| hw.max_processor_id());
    });

    group.finish();
}

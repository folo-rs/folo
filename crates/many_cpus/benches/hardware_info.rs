use criterion::{Criterion, criterion_group, criterion_main};
use many_cpus::HardwareInfo;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("HardwareInfo");

    // Mostly pointless since all the accessors just load from a static lazy-initialize
    // variable. Just here to detect anomalies if we do something strange and it gets slow.
    group.bench_function("max_processor_id", |b| {
        b.iter(|| {
            let _ = HardwareInfo::current().max_processor_id();
        })
    });

    group.finish();
}

//! Benchmarks comparing the cost of `MetricsPusher::push()` across different scenarios
//! involving large numbers of registered events.
//!
//! Specifically investigates whether pushing idle pairs (where the local bag has not
//! received any new observations since the last push) is a measurable overhead, and
//! how the cost of operating push-mode events compares to pull-mode events.
//!
//! Each scenario is run twice:
//!
//! * `counter` — bare counter events with no histogram. Each push of a dirty pair
//!   stores only the `count` and `sum` fields, so the per-pair push cost is small.
//! * `histogram` — events with a large (32-bucket) histogram. Each push of a dirty
//!   pair stores `count`, `sum`, and all 32 bucket counts, so the per-pair push cost
//!   is much higher, magnifying the effect of skipping idle pairs.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use nm::{Event, Magnitude, MetricsPusher, Pull, Push};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

const BULK_EVENT_COUNT: usize = 1000;
const SPARSE_UPDATE_COUNT: usize = 10;

const HISTOGRAM_BUCKETS: &[Magnitude] = &[
    0, 1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536, 131072,
    262144, 524288, 1048576, 2097152, 4194304, 8388608, 16777216, 33554432, 67108864, 134217728,
    268435456, 536870912, 1073741824,
];

struct PushBulk {
    pusher: MetricsPusher,
    events: Vec<Event<Push>>,
}

impl PushBulk {
    fn new(name_prefix: &str, buckets: &'static [Magnitude]) -> Self {
        let pusher = MetricsPusher::new();

        let events = (0..BULK_EVENT_COUNT)
            .map(|i| {
                Event::builder()
                    .name(format!("{name_prefix}_{i}"))
                    .histogram(buckets)
                    .pusher(&pusher)
                    .build()
            })
            .collect();

        Self { pusher, events }
    }
}

fn make_pull_bulk(name_prefix: &str, buckets: &'static [Magnitude]) -> Vec<Event<Pull>> {
    (0..BULK_EVENT_COUNT)
        .map(|i| {
            Event::builder()
                .name(format!("{name_prefix}_{i}"))
                .histogram(buckets)
                .build()
        })
        .collect()
}

thread_local! {
    // Counter variants: events without a histogram. Push of a dirty pair only
    // stores `count` and `sum`.
    static PUSH_IDLE_COUNTER: PushBulk =
        PushBulk::new("bench_bulk_push_idle_counter", &[]);
    static PUSH_UPDATED_COUNTER: PushBulk =
        PushBulk::new("bench_bulk_push_updated_counter", &[]);
    static PULL_UPDATED_COUNTER: Vec<Event<Pull>> =
        make_pull_bulk("bench_bulk_pull_updated_counter", &[]);

    // Histogram variants: events with 32 histogram buckets. Push of a dirty pair
    // stores `count`, `sum`, and all 32 bucket counts (34 atomic stores per pair).
    static PUSH_IDLE_HISTOGRAM: PushBulk =
        PushBulk::new("bench_bulk_push_idle_histogram", HISTOGRAM_BUCKETS);
    static PUSH_UPDATED_HISTOGRAM: PushBulk =
        PushBulk::new("bench_bulk_push_updated_histogram", HISTOGRAM_BUCKETS);
    static PULL_UPDATED_HISTOGRAM: Vec<Event<Pull>> =
        make_pull_bulk("bench_bulk_pull_updated_histogram", HISTOGRAM_BUCKETS);
}

fn entrypoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("nm_push_bulk/push_bulk");

    // Warm up the thread-locals so that the lazy initialization (registering 1000
    // events with the local registry) does not contaminate the first measured iteration.
    PUSH_IDLE_COUNTER.with(|bulk| bulk.pusher.push());
    PUSH_UPDATED_COUNTER.with(|bulk| bulk.pusher.push());
    PULL_UPDATED_COUNTER.with(|events| _ = black_box(events.len()));
    PUSH_IDLE_HISTOGRAM.with(|bulk| bulk.pusher.push());
    PUSH_UPDATED_HISTOGRAM.with(|bulk| bulk.pusher.push());
    PULL_UPDATED_HISTOGRAM.with(|events| _ = black_box(events.len()));

    // Counter variants (no histogram).

    group.bench_function("push_idle_counter", |b| {
        b.iter(|| {
            PUSH_IDLE_COUNTER.with(|bulk| bulk.pusher.push());
        });
    });

    group.bench_function("observe_then_push_counter", |b| {
        b.iter(|| {
            PUSH_UPDATED_COUNTER.with(|bulk| {
                for event in &bulk.events {
                    event.observe_once();
                }
                bulk.pusher.push();
            });
        });
    });

    // Sparse-update scenario: many events registered, few touched per cycle.
    // This is the primary target workload for the dirty-skip optimization.
    group.bench_function("observe_sparse_then_push_counter", |b| {
        b.iter(|| {
            PUSH_UPDATED_COUNTER.with(|bulk| {
                for event in bulk.events.iter().take(SPARSE_UPDATE_COUNT) {
                    event.observe_once();
                }
                bulk.pusher.push();
            });
        });
    });

    group.bench_function("observe_push_only_counter", |b| {
        b.iter(|| {
            PUSH_UPDATED_COUNTER.with(|bulk| {
                for event in &bulk.events {
                    event.observe_once();
                }
            });
        });
    });

    group.bench_function("observe_pull_counter", |b| {
        b.iter(|| {
            PULL_UPDATED_COUNTER.with(|events| {
                for event in events {
                    event.observe_once();
                }
            });
        });
    });

    // Histogram variants (32 buckets).

    group.bench_function("push_idle_histogram", |b| {
        b.iter(|| {
            PUSH_IDLE_HISTOGRAM.with(|bulk| bulk.pusher.push());
        });
    });

    group.bench_function("observe_then_push_histogram", |b| {
        b.iter(|| {
            PUSH_UPDATED_HISTOGRAM.with(|bulk| {
                for event in &bulk.events {
                    event.observe_once();
                }
                bulk.pusher.push();
            });
        });
    });

    group.bench_function("observe_sparse_then_push_histogram", |b| {
        b.iter(|| {
            PUSH_UPDATED_HISTOGRAM.with(|bulk| {
                for event in bulk.events.iter().take(SPARSE_UPDATE_COUNT) {
                    event.observe_once();
                }
                bulk.pusher.push();
            });
        });
    });

    group.bench_function("observe_push_only_histogram", |b| {
        b.iter(|| {
            PUSH_UPDATED_HISTOGRAM.with(|bulk| {
                for event in &bulk.events {
                    event.observe_once();
                }
            });
        });
    });

    group.bench_function("observe_pull_histogram", |b| {
        b.iter(|| {
            PULL_UPDATED_HISTOGRAM.with(|events| {
                for event in events {
                    event.observe_once();
                }
            });
        });
    });

    group.finish();
}

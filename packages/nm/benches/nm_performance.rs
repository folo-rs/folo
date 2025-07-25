//! Benchmarking the observation of events.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;

use benchmark_utils::{ThreadPool, bench_on_every_processor, bench_on_threadpool};
use criterion::{Criterion, criterion_group, criterion_main};
use many_cpus::ProcessorSet;
use new_zealand::nz;
use nm::{Event, Magnitude, MetricsPusher, Push, Report};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    let one_processor = ThreadPool::new(
        &ProcessorSet::builder()
            .performance_processors_only()
            .take(nz!(1))
            .unwrap(),
    );

    let mut group = c.benchmark_group("nm_observation");

    group.bench_function("counter_st_pull", |b| {
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &one_processor,
                iters,
                || (),
                |()| PULL_COUNTER.with(Event::observe_once),
            )
        });
    });

    group.bench_function("counter_mt_pull", |b| {
        b.iter_custom(|iters| {
            bench_on_every_processor(iters, || (), |()| PULL_COUNTER.with(Event::observe_once))
        });
    });

    group.bench_function("plain_st_pull", |b| {
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &one_processor,
                iters,
                || (),
                |()| PULL_COUNTER.with(|x| x.observe(2)),
            )
        });
    });

    group.bench_function("plain_mt_pull", |b| {
        b.iter_custom(|iters| {
            bench_on_every_processor(iters, || (), |()| PULL_COUNTER.with(|x| x.observe(2)))
        });
    });

    group.bench_function("small_histogram_zero_st_pull", |b| {
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &one_processor,
                iters,
                || (),
                |()| PULL_SMALL_HISTOGRAM.with(|x| x.observe(0)),
            )
        });
    });

    group.bench_function("small_histogram_zero_mt_pull", |b| {
        b.iter_custom(|iters| {
            bench_on_every_processor(
                iters,
                || (),
                |()| PULL_SMALL_HISTOGRAM.with(|x| x.observe(0)),
            )
        });
    });

    group.bench_function("large_histogram_zero_st_pull", |b| {
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &one_processor,
                iters,
                || (),
                |()| PULL_LARGE_HISTOGRAM.with(|x| x.observe(0)),
            )
        });
    });

    group.bench_function("large_histogram_zero_mt_pull", |b| {
        b.iter_custom(|iters| {
            bench_on_every_processor(
                iters,
                || (),
                |()| PULL_LARGE_HISTOGRAM.with(|x| x.observe(0)),
            )
        });
    });

    group.bench_function("small_histogram_max_st_pull", |b| {
        // We use Magnitude::MAX to intentionally go out of range (naively implemented, searching
        // through all the buckets for a matching one and finding none).
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &one_processor,
                iters,
                || (),
                |()| PULL_SMALL_HISTOGRAM.with(|x| x.observe(Magnitude::MAX)),
            )
        });
    });

    group.bench_function("small_histogram_max_mt_pull", |b| {
        // We use Magnitude::MAX to intentionally go out of range (naively implemented, searching
        // through all the buckets for a matching one and finding none).
        b.iter_custom(|iters| {
            bench_on_every_processor(
                iters,
                || (),
                |()| PULL_SMALL_HISTOGRAM.with(|x| x.observe(Magnitude::MAX)),
            )
        });
    });

    group.bench_function("large_histogram_max_st_pull", |b| {
        // We use Magnitude::MAX to intentionally go out of range (naively implemented, searching
        // through all the buckets for a matching one and finding none).
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &one_processor,
                iters,
                || (),
                |()| PULL_LARGE_HISTOGRAM.with(|x| x.observe(Magnitude::MAX)),
            )
        });
    });

    group.bench_function("large_histogram_max_mt_pull", |b| {
        // We use Magnitude::MAX to intentionally go out of range (naively implemented, searching
        // through all the buckets for a matching one and finding none).
        b.iter_custom(|iters| {
            bench_on_every_processor(
                iters,
                || (),
                |()| PULL_LARGE_HISTOGRAM.with(|x| x.observe(Magnitude::MAX)),
            )
        });
    });

    group.bench_function("counter_st_push", |b| {
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &one_processor,
                iters,
                || (),
                |()| PUSH_COUNTER.with(Event::observe_once),
            )
        });
    });

    group.bench_function("counter_mt_push", |b| {
        b.iter_custom(|iters| {
            bench_on_every_processor(iters, || (), |()| PUSH_COUNTER.with(Event::observe_once))
        });
    });

    group.bench_function("plain_st_push", |b| {
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &one_processor,
                iters,
                || (),
                |()| PUSH_COUNTER.with(|x| x.observe(2)),
            )
        });
    });

    group.bench_function("plain_mt_push", |b| {
        b.iter_custom(|iters| {
            bench_on_every_processor(iters, || (), |()| PUSH_COUNTER.with(|x| x.observe(2)))
        });
    });

    group.bench_function("small_histogram_zero_st_push", |b| {
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &one_processor,
                iters,
                || (),
                |()| PUSH_SMALL_HISTOGRAM.with(|x| x.observe(0)),
            )
        });
    });

    group.bench_function("small_histogram_zero_mt_push", |b| {
        b.iter_custom(|iters| {
            bench_on_every_processor(
                iters,
                || (),
                |()| PUSH_SMALL_HISTOGRAM.with(|x| x.observe(0)),
            )
        });
    });

    group.bench_function("large_histogram_zero_st_push", |b| {
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &one_processor,
                iters,
                || (),
                |()| PUSH_LARGE_HISTOGRAM.with(|x| x.observe(0)),
            )
        });
    });

    group.bench_function("large_histogram_zero_mt_push", |b| {
        b.iter_custom(|iters| {
            bench_on_every_processor(
                iters,
                || (),
                |()| PUSH_LARGE_HISTOGRAM.with(|x| x.observe(0)),
            )
        });
    });

    group.bench_function("small_histogram_max_st_push", |b| {
        // We use Magnitude::MAX to intentionally go out of range (naively implemented, searching
        // through all the buckets for a matching one and finding none).
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &one_processor,
                iters,
                || (),
                |()| PUSH_SMALL_HISTOGRAM.with(|x| x.observe(Magnitude::MAX)),
            )
        });
    });

    group.bench_function("small_histogram_max_mt_push", |b| {
        // We use Magnitude::MAX to intentionally go out of range (naively implemented, searching
        // through all the buckets for a matching one and finding none).
        b.iter_custom(|iters| {
            bench_on_every_processor(
                iters,
                || (),
                |()| PUSH_SMALL_HISTOGRAM.with(|x| x.observe(Magnitude::MAX)),
            )
        });
    });

    group.bench_function("large_histogram_max_st_push", |b| {
        // We use Magnitude::MAX to intentionally go out of range (naively implemented, searching
        // through all the buckets for a matching one and finding none).
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &one_processor,
                iters,
                || (),
                |()| PUSH_LARGE_HISTOGRAM.with(|x| x.observe(Magnitude::MAX)),
            )
        });
    });

    group.bench_function("large_histogram_max_mt_push", |b| {
        // We use Magnitude::MAX to intentionally go out of range (naively implemented, searching
        // through all the buckets for a matching one and finding none).
        b.iter_custom(|iters| {
            bench_on_every_processor(
                iters,
                || (),
                |()| PUSH_LARGE_HISTOGRAM.with(|x| x.observe(Magnitude::MAX)),
            )
        });
    });

    group.finish();

    let mut group = c.benchmark_group("nm_collection");

    group.bench_function("collect_st", |b| {
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &one_processor,
                iters,
                || (),
                |()| drop(black_box(Report::collect())),
            )
        });
    });

    group.bench_function("collect_mt", |b| {
        b.iter_custom(|iters| {
            bench_on_every_processor(iters, || (), |()| drop(black_box(Report::collect())))
        });
    });

    group.finish();

    let mut group = c.benchmark_group("nm_push");

    group.bench_function("push_st", |b| {
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &one_processor,
                iters,
                || (),
                |()| PUSHER.with(MetricsPusher::push),
            )
        });
    });

    group.bench_function("push_mt", |b| {
        b.iter_custom(|iters| {
            bench_on_every_processor(iters, || (), |()| PUSHER.with(MetricsPusher::push))
        });
    });

    group.finish();

    let mut group = c.benchmark_group("nm_timing");

    group.bench_function("timing_st_pull", |b| {
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &one_processor,
                iters,
                || (),
                |()| PULL_COUNTER.with(|x| x.observe_duration_millis(|| black_box(()))),
            )
        });
    });

    group.bench_function("timing_mt_pull", |b| {
        b.iter_custom(|iters| {
            bench_on_every_processor(
                iters,
                || (),
                |()| PULL_COUNTER.with(|x| x.observe_duration_millis(|| black_box(()))),
            )
        });
    });

    group.bench_function("timing_st_push", |b| {
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &one_processor,
                iters,
                || (),
                |()| PUSH_COUNTER.with(|x| x.observe_duration_millis(|| black_box(()))),
            )
        });
    });

    group.bench_function("timing_mt_push", |b| {
        b.iter_custom(|iters| {
            bench_on_every_processor(
                iters,
                || (),
                |()| PUSH_COUNTER.with(|x| x.observe_duration_millis(|| black_box(()))),
            )
        });
    });

    group.finish();
}

const SMALL_HISTOGRAM_BUCKETS: &[Magnitude] = &[0, 10, 100, 1000, 10000];
const LARGE_HISTOGRAM_BUCKETS: &[Magnitude] = &[
    0, 1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536, 131072,
    262144, 524288, 1048576, 2097152, 4194304, 8388608, 16777216, 33554432, 67108864, 134217728,
    268435456, 536870912, 1073741824,
];

thread_local! {
    static PULL_COUNTER: Event = Event::builder()
        .name("pull_counter")
        .build();

    static PULL_PLAIN: Event = Event::builder()
        .name("pull_plain")
        .build();

    static PULL_SMALL_HISTOGRAM: Event = Event::builder()
        .name("pull_small_histogram")
        .histogram(SMALL_HISTOGRAM_BUCKETS)
        .build();

    static PULL_LARGE_HISTOGRAM: Event = Event::builder()
        .name("pull_large_histogram")
        .histogram(LARGE_HISTOGRAM_BUCKETS)
        .build();

    static PUSHER: MetricsPusher = MetricsPusher::new();

    static PUSH_COUNTER: Event<Push> = Event::builder()
        .name("push_counter")
        .pusher_local(&PUSHER)
        .build();

    static PUSH_PLAIN: Event<Push> = Event::builder()
        .name("push_plain")
        .pusher_local(&PUSHER)
        .build();

    static PUSH_SMALL_HISTOGRAM: Event<Push> = Event::builder()
        .name("push_small_histogram")
        .histogram(SMALL_HISTOGRAM_BUCKETS)
        .pusher_local(&PUSHER)
        .build();

    static PUSH_LARGE_HISTOGRAM: Event<Push> = Event::builder()
        .name("push_large_histogram")
        .histogram(LARGE_HISTOGRAM_BUCKETS)
        .pusher_local(&PUSHER)
        .build();
}

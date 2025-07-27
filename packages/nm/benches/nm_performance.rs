//! Benchmarking the observation of events.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use many_cpus::ProcessorSet;
use new_zealand::nz;
use nm::{Event, Magnitude, MetricsPusher, Push, Report};
use par_bench::{Run, ThreadPool};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    let mut one_thread = ThreadPool::new(&ProcessorSet::single());
    let mut two_threads = ProcessorSet::builder().take(nz!(2)).map(|x| ThreadPool::new(&x));

    let mut group = c.benchmark_group("nm_observation");

    Run::new()
        .iter(|_| PULL_COUNTER.with(Event::observe_once))
        .execute_criterion_on(&mut one_thread, &mut group, "counter_st_pull");

    Run::new()
        .iter(|_| PULL_COUNTER.with(|x| x.observe(2)))
        .execute_criterion_on(&mut one_thread, &mut group, "plain_st_pull");

    Run::new()
        .iter(|_| PULL_SMALL_HISTOGRAM.with(|x| x.observe(0)))
        .execute_criterion_on(&mut one_thread, &mut group, "small_histogram_zero_st_pull");

    Run::new()
        .iter(|_| PULL_LARGE_HISTOGRAM.with(|x| x.observe(0)))
        .execute_criterion_on(&mut one_thread, &mut group, "large_histogram_zero_st_pull");

    // We use Magnitude::MAX to intentionally go out of range (naively implemented, searching
    // through all the buckets for a matching one and finding none).
    Run::new()
        .iter(|_| PULL_SMALL_HISTOGRAM.with(|x| x.observe(Magnitude::MAX)))
        .execute_criterion_on(&mut one_thread, &mut group, "small_histogram_max_st_pull");

    // We use Magnitude::MAX to intentionally go out of range (naively implemented, searching
    // through all the buckets for a matching one and finding none).
    Run::new()
        .iter(|_| PULL_LARGE_HISTOGRAM.with(|x| x.observe(Magnitude::MAX)))
        .execute_criterion_on(&mut one_thread, &mut group, "large_histogram_max_st_pull");

    Run::new()
        .iter(|_| PUSH_COUNTER.with(Event::observe_once))
        .execute_criterion_on(&mut one_thread, &mut group, "counter_st_push");

    Run::new()
        .iter(|_| PUSH_COUNTER.with(|x| x.observe(2)))
        .execute_criterion_on(&mut one_thread, &mut group, "plain_st_push");

    Run::new()
        .iter(|_| PUSH_SMALL_HISTOGRAM.with(|x| x.observe(0)))
        .execute_criterion_on(&mut one_thread, &mut group, "small_histogram_zero_st_push");

    Run::new()
        .iter(|_| PUSH_LARGE_HISTOGRAM.with(|x| x.observe(0)))
        .execute_criterion_on(&mut one_thread, &mut group, "large_histogram_zero_st_push");

    // We use Magnitude::MAX to intentionally go out of range (naively implemented, searching
    // through all the buckets for a matching one and finding none).
    Run::new()
        .iter(|_| PUSH_SMALL_HISTOGRAM.with(|x| x.observe(Magnitude::MAX)))
        .execute_criterion_on(&mut one_thread, &mut group, "small_histogram_max_st_push");

    // We use Magnitude::MAX to intentionally go out of range (naively implemented, searching
    // through all the buckets for a matching one and finding none).
    Run::new()
        .iter(|_| PUSH_LARGE_HISTOGRAM.with(|x| x.observe(Magnitude::MAX)))
        .execute_criterion_on(&mut one_thread, &mut group, "large_histogram_max_st_push");

    // Two-processor benchmarks for nm_observation group.
    if let Some(ref mut thread_pool) = two_threads {
        Run::new()
            .iter(|_| PULL_COUNTER.with(Event::observe_once))
            .execute_criterion_on(thread_pool, &mut group, "counter_mt_pull");

        Run::new()
            .iter(|_| PULL_COUNTER.with(|x| x.observe(2)))
            .execute_criterion_on(thread_pool, &mut group, "plain_mt_pull");

        Run::new()
            .iter(|_| PULL_SMALL_HISTOGRAM.with(|x| x.observe(0)))
            .execute_criterion_on(thread_pool, &mut group, "small_histogram_zero_mt_pull");

        Run::new()
            .iter(|_| PULL_LARGE_HISTOGRAM.with(|x| x.observe(0)))
            .execute_criterion_on(thread_pool, &mut group, "large_histogram_zero_mt_pull");

        // We use Magnitude::MAX to intentionally go out of range (naively implemented, searching
        // through all the buckets for a matching one and finding none).
        Run::new()
            .iter(|_| PULL_SMALL_HISTOGRAM.with(|x| x.observe(Magnitude::MAX)))
            .execute_criterion_on(thread_pool, &mut group, "small_histogram_max_mt_pull");

        // We use Magnitude::MAX to intentionally go out of range (naively implemented, searching
        // through all the buckets for a matching one and finding none).
        Run::new()
            .iter(|_| PULL_LARGE_HISTOGRAM.with(|x| x.observe(Magnitude::MAX)))
            .execute_criterion_on(thread_pool, &mut group, "large_histogram_max_mt_pull");

        Run::new()
            .iter(|_| PUSH_COUNTER.with(Event::observe_once))
            .execute_criterion_on(thread_pool, &mut group, "counter_mt_push");

        Run::new()
            .iter(|_| PUSH_COUNTER.with(|x| x.observe(2)))
            .execute_criterion_on(thread_pool, &mut group, "plain_mt_push");

        Run::new()
            .iter(|_| PUSH_SMALL_HISTOGRAM.with(|x| x.observe(0)))
            .execute_criterion_on(thread_pool, &mut group, "small_histogram_zero_mt_push");

        Run::new()
            .iter(|_| PUSH_LARGE_HISTOGRAM.with(|x| x.observe(0)))
            .execute_criterion_on(thread_pool, &mut group, "large_histogram_zero_mt_push");

        // We use Magnitude::MAX to intentionally go out of range (naively implemented, searching
        // through all the buckets for a matching one and finding none).
        Run::new()
            .iter(|_| PUSH_SMALL_HISTOGRAM.with(|x| x.observe(Magnitude::MAX)))
            .execute_criterion_on(thread_pool, &mut group, "small_histogram_max_mt_push");

        // We use Magnitude::MAX to intentionally go out of range (naively implemented, searching
        // through all the buckets for a matching one and finding none).
        Run::new()
            .iter(|_| PUSH_LARGE_HISTOGRAM.with(|x| x.observe(Magnitude::MAX)))
            .execute_criterion_on(thread_pool, &mut group, "large_histogram_max_mt_push");
    }

    group.finish();

    let mut group = c.benchmark_group("nm_collection");

    Run::new()
        .iter(|_| drop(black_box(Report::collect())))
        .execute_criterion_on(&mut one_thread, &mut group, "collect_st");

    // Two-processor benchmarks for nm_collection group.
    if let Some(ref mut thread_pool) = two_threads {
        Run::new()
            .iter(|_| drop(black_box(Report::collect())))
            .execute_criterion_on(thread_pool, &mut group, "collect_mt");
    }

    group.finish();

    let mut group = c.benchmark_group("nm_push");

    Run::new()
        .iter(|_| PUSHER.with(MetricsPusher::push))
        .execute_criterion_on(&mut one_thread, &mut group, "push_st");

    // Two-processor benchmarks for nm_push group.
    if let Some(ref mut thread_pool) = two_threads {
        Run::new()
            .iter(|_| PUSHER.with(MetricsPusher::push))
            .execute_criterion_on(thread_pool, &mut group, "push_mt");
    }

    group.finish();

    let mut group = c.benchmark_group("nm_timing");

    Run::new()
        .iter(|_| PULL_COUNTER.with(|x| x.observe_duration_millis(|| black_box(()))))
        .execute_criterion_on(&mut one_thread, &mut group, "timing_st_pull");

    Run::new()
        .iter(|_| PUSH_COUNTER.with(|x| x.observe_duration_millis(|| black_box(()))))
        .execute_criterion_on(&mut one_thread, &mut group, "timing_st_push");

    // Two-processor benchmarks for nm_timing group.
    if let Some(ref mut thread_pool) = two_threads {
        Run::new()
            .iter(|_| PULL_COUNTER.with(|x| x.observe_duration_millis(|| black_box(()))))
            .execute_criterion_on(thread_pool, &mut group, "timing_mt_pull");

        Run::new()
            .iter(|_| PUSH_COUNTER.with(|x| x.observe_duration_millis(|| black_box(()))))
            .execute_criterion_on(thread_pool, &mut group, "timing_mt_push");
    }

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

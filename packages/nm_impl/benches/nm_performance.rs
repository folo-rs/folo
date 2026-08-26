//! Benchmarks for the elementary operations of the `nm` package: observing events, publishing
//! the metrics of events that use the push model, and collecting reports.
//!
//! Paired with `nm_performance_cg.rs`, which measures the single-threaded scenarios of this file
//! under Callgrind.
//!
//! # Benchmark name vocabulary
//!
//! * `counter`: an event without a histogram, observed without an explicit magnitude.
//! * `plain`: an event without a histogram, observed with an explicit magnitude.
//! * `counter_batch_low` / `counter_batch_high`: a batch observation that records few / many
//!   occurrences in one operation.
//! * `small_histogram` / `large_histogram`: an event whose histogram has few / many buckets.
//! * `first_bucket` / `last_bucket` / `above_range`: where the observed magnitude lands: in the
//!   first bucket, in the last bucket, or in the implicit final range that captures magnitudes
//!   above every configured bucket boundary.
//! * `idle` / `dirty_events` / `dirty_buckets`: how much of a pusher's registry has been
//!   observed since the previous push: nothing, some events, or some buckets of every histogram
//!   event.
//! * `low` / `high`: the endpoints of a scenario's size axis.
//! * `st` / `mt`: whether the benchmark runs on one thread or two threads.
//! * `pull` / `push`: the publishing model of the event being observed.
//!
//! # Collection scope
//!
//! `Report::collect()` reads the process-global registry, so the collection group runs before the
//! observation groups can register unrelated state. Its low-cardinality case contains the event
//! shapes mirrored by the Callgrind collection case. The high-cardinality case adds the shared
//! push scenario and is intentionally Criterion-only because allocation and cache behavior
//! dominate at that scale.
//!
//! Collection is measured on one thread only, because a report is collected by a reporting thread
//! once per reporting interval rather than on any hot path.
//!
//! # Duration observation
//!
//! The `timing` benchmarks have no Callgrind counterpart: their cost is dominated by reading the
//! platform clock, which the Callgrind simulator models as a fixed cost, so an instruction count
//! would describe the wrapper rather than the operation.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{BatchSize, Bencher, Criterion, criterion_group, criterion_main};
use many_cpus::SystemHardware;
use new_zealand::nz;
use nm::{Event, Magnitude, MetricsPusher, Push, Report};
use par_bench::{Run, ThreadPool};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    let mut one_thread = ThreadPool::new(
        SystemHardware::current()
            .processors()
            .to_builder()
            .take(nz!(1))
            .unwrap(),
    );
    let mut two_threads = SystemHardware::current()
        .processors()
        .to_builder()
        .take(nz!(2))
        .map(|x| ThreadPool::new(&x));

    materialize_thread_events(&mut one_thread);
    collection_benchmarks(c);
    materialize_additional_observation_events(two_threads.as_mut());
    observation_benchmarks(c, &mut one_thread, two_threads.as_mut());
    push_benchmarks(c);
    timing_benchmarks(c, &mut one_thread, two_threads.as_mut());
}

fn observation_benchmarks(
    c: &mut Criterion,
    one_thread: &mut ThreadPool,
    two_threads: Option<&mut ThreadPool>,
) {
    let mut group = c.benchmark_group("nm_performance/observation");

    Run::new()
        .iter(|_| PULL_COUNTER.with(Event::observe_once))
        .execute_criterion_on(one_thread, &mut group, "counter_st_pull");

    Run::new()
        .iter(|_| PULL_PLAIN.with(|x| x.observe(black_box(PLAIN_MAGNITUDE))))
        .execute_criterion_on(one_thread, &mut group, "plain_st_pull");

    Run::new()
        .iter(|_| PULL_COUNTER.with(|x| x.batch(black_box(BATCH_LOW)).observe_once()))
        .execute_criterion_on(one_thread, &mut group, "counter_batch_low_st_pull");

    Run::new()
        .iter(|_| PULL_COUNTER.with(|x| x.batch(black_box(BATCH_HIGH)).observe_once()))
        .execute_criterion_on(one_thread, &mut group, "counter_batch_high_st_pull");

    Run::new()
        .iter(|_| PULL_SMALL_HISTOGRAM.with(|x| x.observe(black_box(FIRST_BUCKET_MAGNITUDE))))
        .execute_criterion_on(
            one_thread,
            &mut group,
            "small_histogram_first_bucket_st_pull",
        );

    Run::new()
        .iter(|_| {
            PULL_SMALL_HISTOGRAM
                .with(|x| x.observe(black_box(SMALL_HISTOGRAM_LAST_BUCKET_MAGNITUDE)));
        })
        .execute_criterion_on(
            one_thread,
            &mut group,
            "small_histogram_last_bucket_st_pull",
        );

    Run::new()
        .iter(|_| PULL_SMALL_HISTOGRAM.with(|x| x.observe(black_box(ABOVE_RANGE_MAGNITUDE))))
        .execute_criterion_on(
            one_thread,
            &mut group,
            "small_histogram_above_range_st_pull",
        );

    Run::new()
        .iter(|_| PULL_LARGE_HISTOGRAM.with(|x| x.observe(black_box(FIRST_BUCKET_MAGNITUDE))))
        .execute_criterion_on(
            one_thread,
            &mut group,
            "large_histogram_first_bucket_st_pull",
        );

    Run::new()
        .iter(|_| {
            PULL_LARGE_HISTOGRAM
                .with(|x| x.observe(black_box(LARGE_HISTOGRAM_LAST_BUCKET_MAGNITUDE)));
        })
        .execute_criterion_on(
            one_thread,
            &mut group,
            "large_histogram_last_bucket_st_pull",
        );

    Run::new()
        .iter(|_| PULL_LARGE_HISTOGRAM.with(|x| x.observe(black_box(ABOVE_RANGE_MAGNITUDE))))
        .execute_criterion_on(
            one_thread,
            &mut group,
            "large_histogram_above_range_st_pull",
        );

    Run::new()
        .iter(|_| PUSH_COUNTER.with(Event::observe_once))
        .execute_criterion_on(one_thread, &mut group, "counter_st_push");

    Run::new()
        .iter(|_| PUSH_PLAIN.with(|x| x.observe(black_box(PLAIN_MAGNITUDE))))
        .execute_criterion_on(one_thread, &mut group, "plain_st_push");

    Run::new()
        .iter(|_| PUSH_COUNTER.with(|x| x.batch(black_box(BATCH_LOW)).observe_once()))
        .execute_criterion_on(one_thread, &mut group, "counter_batch_low_st_push");

    Run::new()
        .iter(|_| PUSH_COUNTER.with(|x| x.batch(black_box(BATCH_HIGH)).observe_once()))
        .execute_criterion_on(one_thread, &mut group, "counter_batch_high_st_push");

    Run::new()
        .iter(|_| PUSH_SMALL_HISTOGRAM.with(|x| x.observe(black_box(FIRST_BUCKET_MAGNITUDE))))
        .execute_criterion_on(
            one_thread,
            &mut group,
            "small_histogram_first_bucket_st_push",
        );

    Run::new()
        .iter(|_| {
            PUSH_SMALL_HISTOGRAM
                .with(|x| x.observe(black_box(SMALL_HISTOGRAM_LAST_BUCKET_MAGNITUDE)));
        })
        .execute_criterion_on(
            one_thread,
            &mut group,
            "small_histogram_last_bucket_st_push",
        );

    Run::new()
        .iter(|_| PUSH_SMALL_HISTOGRAM.with(|x| x.observe(black_box(ABOVE_RANGE_MAGNITUDE))))
        .execute_criterion_on(
            one_thread,
            &mut group,
            "small_histogram_above_range_st_push",
        );

    Run::new()
        .iter(|_| PUSH_LARGE_HISTOGRAM.with(|x| x.observe(black_box(FIRST_BUCKET_MAGNITUDE))))
        .execute_criterion_on(
            one_thread,
            &mut group,
            "large_histogram_first_bucket_st_push",
        );

    Run::new()
        .iter(|_| {
            PUSH_LARGE_HISTOGRAM
                .with(|x| x.observe(black_box(LARGE_HISTOGRAM_LAST_BUCKET_MAGNITUDE)));
        })
        .execute_criterion_on(
            one_thread,
            &mut group,
            "large_histogram_last_bucket_st_push",
        );

    Run::new()
        .iter(|_| PUSH_LARGE_HISTOGRAM.with(|x| x.observe(black_box(ABOVE_RANGE_MAGNITUDE))))
        .execute_criterion_on(
            one_thread,
            &mut group,
            "large_histogram_above_range_st_push",
        );

    if let Some(thread_pool) = two_threads {
        Run::new()
            .iter(|_| PULL_COUNTER.with(Event::observe_once))
            .execute_criterion_on(thread_pool, &mut group, "counter_mt_pull");

        Run::new()
            .iter(|_| PULL_PLAIN.with(|x| x.observe(black_box(PLAIN_MAGNITUDE))))
            .execute_criterion_on(thread_pool, &mut group, "plain_mt_pull");

        Run::new()
            .iter(|_| PULL_SMALL_HISTOGRAM.with(|x| x.observe(black_box(FIRST_BUCKET_MAGNITUDE))))
            .execute_criterion_on(
                thread_pool,
                &mut group,
                "small_histogram_first_bucket_mt_pull",
            );

        Run::new()
            .iter(|_| PULL_SMALL_HISTOGRAM.with(|x| x.observe(black_box(ABOVE_RANGE_MAGNITUDE))))
            .execute_criterion_on(
                thread_pool,
                &mut group,
                "small_histogram_above_range_mt_pull",
            );

        Run::new()
            .iter(|_| PULL_LARGE_HISTOGRAM.with(|x| x.observe(black_box(FIRST_BUCKET_MAGNITUDE))))
            .execute_criterion_on(
                thread_pool,
                &mut group,
                "large_histogram_first_bucket_mt_pull",
            );

        Run::new()
            .iter(|_| PULL_LARGE_HISTOGRAM.with(|x| x.observe(black_box(ABOVE_RANGE_MAGNITUDE))))
            .execute_criterion_on(
                thread_pool,
                &mut group,
                "large_histogram_above_range_mt_pull",
            );

        Run::new()
            .iter(|_| PUSH_COUNTER.with(Event::observe_once))
            .execute_criterion_on(thread_pool, &mut group, "counter_mt_push");

        Run::new()
            .iter(|_| PUSH_PLAIN.with(|x| x.observe(black_box(PLAIN_MAGNITUDE))))
            .execute_criterion_on(thread_pool, &mut group, "plain_mt_push");

        Run::new()
            .iter(|_| PUSH_SMALL_HISTOGRAM.with(|x| x.observe(black_box(FIRST_BUCKET_MAGNITUDE))))
            .execute_criterion_on(
                thread_pool,
                &mut group,
                "small_histogram_first_bucket_mt_push",
            );

        Run::new()
            .iter(|_| PUSH_SMALL_HISTOGRAM.with(|x| x.observe(black_box(ABOVE_RANGE_MAGNITUDE))))
            .execute_criterion_on(
                thread_pool,
                &mut group,
                "small_histogram_above_range_mt_push",
            );

        Run::new()
            .iter(|_| PUSH_LARGE_HISTOGRAM.with(|x| x.observe(black_box(FIRST_BUCKET_MAGNITUDE))))
            .execute_criterion_on(
                thread_pool,
                &mut group,
                "large_histogram_first_bucket_mt_push",
            );

        Run::new()
            .iter(|_| PUSH_LARGE_HISTOGRAM.with(|x| x.observe(black_box(ABOVE_RANGE_MAGNITUDE))))
            .execute_criterion_on(
                thread_pool,
                &mut group,
                "large_histogram_above_range_mt_push",
            );
    }

    group.finish();
}

fn collection_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("nm_performance/collection");

    // The collected report is handed back to Criterion, which drops it after the measurement,
    // so the measured region covers collection alone. The batch size bounds how many reports
    // are alive at once.
    group.bench_function("collect_low_cardinality_st", |b| {
        b.iter_batched(
            || (),
            |()| Report::collect(),
            BatchSize::NumIterations(COLLECT_BATCH_ITERATIONS),
        );
    });

    PUSH_SCENARIO.with(|_| ());

    group.bench_function("collect_high_cardinality_st", |b| {
        b.iter_batched(
            || (),
            |()| Report::collect(),
            BatchSize::NumIterations(COLLECT_BATCH_ITERATIONS),
        );
    });

    group.finish();
}

fn push_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("nm_performance/push");

    group.bench_function("idle_st", |b| bench_push(b, |_| ()));

    group.bench_function("dirty_events_low_st", |b| {
        bench_push(b, |scenario| {
            scenario.observe_counters(SPARSE_DIRTY_EVENTS);
        });
    });

    group.bench_function("dirty_events_high_st", |b| {
        bench_push(b, |scenario| {
            scenario.observe_counters(PUSH_COUNTER_EVENTS);
        });
    });

    group.bench_function("dirty_buckets_low_st", |b| {
        bench_push(b, |scenario| {
            scenario.observe_buckets(ONE_BUCKET_MAGNITUDES);
        });
    });

    group.bench_function("dirty_buckets_high_st", |b| {
        bench_push(b, |scenario| {
            scenario.observe_buckets(LARGE_HISTOGRAM_BUCKETS);
        });
    });

    group.finish();
}

fn timing_benchmarks(
    c: &mut Criterion,
    one_thread: &mut ThreadPool,
    two_threads: Option<&mut ThreadPool>,
) {
    let mut group = c.benchmark_group("nm_performance/timing");

    Run::new()
        .iter(|_| PULL_COUNTER.with(|x| x.observe_duration_millis(|| black_box(()))))
        .execute_criterion_on(one_thread, &mut group, "timing_st_pull");

    Run::new()
        .iter(|_| PUSH_COUNTER.with(|x| x.observe_duration_millis(|| black_box(()))))
        .execute_criterion_on(one_thread, &mut group, "timing_st_push");

    if let Some(thread_pool) = two_threads {
        Run::new()
            .iter(|_| PULL_COUNTER.with(|x| x.observe_duration_millis(|| black_box(()))))
            .execute_criterion_on(thread_pool, &mut group, "timing_mt_pull");

        Run::new()
            .iter(|_| PUSH_COUNTER.with(|x| x.observe_duration_millis(|| black_box(()))))
            .execute_criterion_on(thread_pool, &mut group, "timing_mt_push");
    }

    group.finish();
}

/// Measures one push per iteration, with the state that the push operates on prepared outside
/// the measured region.
///
/// Criterion's `iter_batched()` with `BatchSize::PerIteration` is the usual way to keep
/// per-iteration setup out of a measurement, but its overhead is of the same order as a push over
/// this registry, so the push is timed directly instead. Preparing once per push is what keeps a
/// dirty pair dirty for exactly one push; preparing a whole batch up front would leave every
/// push but the first one idle.
///
/// Each reported figure therefore includes one clock read, which is a meaningful share of the
/// idle push and a negligible one of the scenarios that copy data. Compare the scenarios against
/// each other rather than reading any one of them as an absolute cost.
fn bench_push(b: &mut Bencher<'_>, prepare: impl Fn(&PushScenario)) {
    b.iter_custom(|iterations| {
        let mut elapsed = Duration::ZERO;

        for _ in 0..iterations {
            elapsed = elapsed.saturating_add(PUSH_SCENARIO.with(|scenario| {
                prepare(scenario);

                let started = Instant::now();
                scenario.pusher.push();
                started.elapsed()
            }));
        }

        elapsed
    });
}

/// Registers the observation benchmark events on any additional benchmark threads.
fn materialize_additional_observation_events(two_threads: Option<&mut ThreadPool>) {
    if let Some(thread_pool) = two_threads {
        materialize_thread_events(thread_pool);
    }
}

fn materialize_thread_events(pool: &mut ThreadPool) {
    _ = Run::new()
        .iter(|_| {
            PULL_COUNTER.with(Event::observe_once);
            PULL_PLAIN.with(|event| event.observe(PLAIN_MAGNITUDE));
            PULL_SMALL_HISTOGRAM.with(|event| event.observe(FIRST_BUCKET_MAGNITUDE));
            PULL_LARGE_HISTOGRAM.with(|event| event.observe(FIRST_BUCKET_MAGNITUDE));
            PUSH_COUNTER.with(Event::observe_once);
            PUSH_PLAIN.with(|event| event.observe(PLAIN_MAGNITUDE));
            PUSH_SMALL_HISTOGRAM.with(|event| event.observe(FIRST_BUCKET_MAGNITUDE));
            PUSH_LARGE_HISTOGRAM.with(|event| event.observe(FIRST_BUCKET_MAGNITUDE));
            PUSHER.with(MetricsPusher::push);
        })
        .execute_on(pool, 1);
}

/// The registry that the `push` benchmarks publish from: one pusher holding both counter events
/// and histogram events, in a steady state where every pair has already been published once.
///
/// A push walks every pair registered with the pusher, so all `push` benchmarks share one
/// scenario and differ only in how much of it is dirty when the push happens. The scan cost is
/// then identical across the scenarios and the cost of copying dirty data is the only variable.
struct PushScenario {
    pusher: MetricsPusher,
    counters: Vec<Event<Push>>,
    histograms: Vec<Event<Push>>,
}

impl PushScenario {
    fn new() -> Self {
        let pusher = MetricsPusher::new();

        let counters = (0..PUSH_COUNTER_EVENTS)
            .map(|index| {
                Event::builder()
                    .name(format!("bench_push_scenario_counter_{index}"))
                    .pusher(&pusher)
                    .build()
            })
            .collect();

        let histograms = (0..PUSH_HISTOGRAM_EVENTS)
            .map(|index| {
                Event::builder()
                    .name(format!("bench_push_scenario_histogram_{index}"))
                    .histogram(LARGE_HISTOGRAM_BUCKETS)
                    .pusher(&pusher)
                    .build()
            })
            .collect();

        let scenario = Self {
            pusher,
            counters,
            histograms,
        };

        // Observe and publish everything once, so that a benchmark which observes nothing
        // measures the idle path from its first iteration onward.
        scenario.observe_counters(PUSH_COUNTER_EVENTS);
        scenario.observe_buckets(LARGE_HISTOGRAM_BUCKETS);
        scenario.pusher.push();

        scenario
    }

    /// Observes one occurrence in each of the first `count` counter events, leaving those pairs
    /// dirty for the next push.
    fn observe_counters(&self, count: usize) {
        for event in self.counters.iter().take(count) {
            event.observe_once();
        }
    }

    /// Observes one occurrence of each magnitude in every histogram event, leaving those pairs
    /// dirty with one dirty bucket per magnitude.
    fn observe_buckets(&self, magnitudes: &[Magnitude]) {
        for event in &self.histograms {
            for magnitude in magnitudes {
                event.observe(*magnitude);
            }
        }
    }
}

/// Magnitude observed where the benchmark needs an explicit magnitude but its value does not
/// affect what is being measured.
const PLAIN_MAGNITUDE: Magnitude = 2;

/// The smallest batch that still records an occurrence.
const BATCH_LOW: usize = 1;

/// A batch large enough that any per-occurrence work in the implementation would dominate the
/// measurement.
const BATCH_HIGH: usize = 10_000;

/// Bucket boundaries of a histogram of the size that callers typically configure.
const SMALL_HISTOGRAM_BUCKETS: &[Magnitude] = &[0, 10, 100, 1000, 10000];

/// Bucket boundaries of a histogram at the upper end of what callers configure, chosen to expose
/// the per-bucket cost of both the bucket scan and publishing.
const LARGE_HISTOGRAM_BUCKETS: &[Magnitude] = &[
    0, 1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536, 131072,
    262144, 524288, 1048576, 2097152, 4194304, 8388608, 16777216, 33554432, 67108864, 134217728,
    268435456, 536870912, 1073741824,
];

/// Lands in the first bucket of either histogram, which is the shortest bucket scan.
const FIRST_BUCKET_MAGNITUDE: Magnitude = 0;

/// The largest boundary configured for the small histogram, so an observation of it lands in the
/// last bucket after scanning every other bucket.
const SMALL_HISTOGRAM_LAST_BUCKET_MAGNITUDE: Magnitude = 10_000;

/// The largest boundary configured for the large histogram, so an observation of it lands in the
/// last bucket after scanning every other bucket.
const LARGE_HISTOGRAM_LAST_BUCKET_MAGNITUDE: Magnitude = 1_073_741_824;

/// Above every configured boundary of either histogram, so the observation lands in the implicit
/// final range after a scan that matches no bucket.
const ABOVE_RANGE_MAGNITUDE: Magnitude = Magnitude::MAX;

/// Magnitudes that mark a single histogram bucket dirty, for the low end of the dirty-bucket axis.
const ONE_BUCKET_MAGNITUDES: &[Magnitude] = &[FIRST_BUCKET_MAGNITUDE];

/// Counter events registered with the push scenario pusher. Sized so that a push spans enough
/// pairs to be resolvable against the timer while staying within the range of event counts that
/// a single thread realistically registers.
const PUSH_COUNTER_EVENTS: usize = 64;

/// Histogram events registered with the push scenario pusher, matching the counter event count so
/// that both kinds contribute equally to the cost of scanning the registry.
const PUSH_HISTOGRAM_EVENTS: usize = 64;

/// Events observed between two pushes in the sparse workload, representing a push interval in
/// which only a small share of the registered events saw any traffic. This is the workload that
/// the pusher's skipping of unobserved pairs exists for.
const SPARSE_DIRTY_EVENTS: usize = 4;

/// How many reports may be alive at once in the collection benchmark. Large enough to amortize
/// the timer across iterations, small enough to bound the memory held by collected reports.
const COLLECT_BATCH_ITERATIONS: u64 = 64;

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

    static PUSH_SCENARIO: PushScenario = PushScenario::new();
}

//! Benchmarks for observing and publishing metrics at bulk scale: a registry holding as many
//! events as a busy thread accumulates over the lifetime of a process.
//!
//! `nm_performance.rs` measures the same operations against a registry of the size a typical
//! thread holds; this file scales the registry up so that per-event costs, which are invisible
//! at the smaller scale, dominate the measurement. The question it answers is what a push costs
//! when most of the registry has seen no traffic since the previous push, compared to a push that
//! has data to copy for every event.
//!
//! There is no Callgrind counterpart: at this scale the interesting effects are cache and memory
//! behavior, which instruction counts do not capture.
//!
//! # Benchmark name vocabulary
//!
//! The names follow the vocabulary documented in `nm_performance.rs`, with `counters` and
//! `histograms` identifying which of the two bulk registries the benchmark runs against.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;
use std::thread::LocalKey;
use std::time::{Duration, Instant};

use criterion::{Bencher, Criterion, criterion_group, criterion_main};
use nm::{Event, Magnitude, MetricsPusher, Pull, Push};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    // Materialize the registries before measuring, so that their lazy initialization does not
    // land in the first measured iteration of whichever benchmark the caller selected.
    PUSH_COUNTERS.with(|bulk| bulk.pusher.push());
    PUSH_HISTOGRAMS.with(|bulk| bulk.pusher.push());
    PULL_COUNTERS.with(|events| _ = black_box(events.len()));
    PULL_HISTOGRAMS.with(|events| _ = black_box(events.len()));

    push_benchmarks(c);
    observation_benchmarks(c);
}

fn push_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("nm_push_bulk/push");

    group.bench_function("idle_counters_st", |b| {
        bench_push(b, &PUSH_COUNTERS, |_| ());
    });

    group.bench_function("dirty_events_low_counters_st", |b| {
        bench_push(b, &PUSH_COUNTERS, |bulk| {
            bulk.observe_once_in_first(black_box(SPARSE_DIRTY_EVENTS));
        });
    });

    group.bench_function("dirty_events_high_counters_st", |b| {
        bench_push(b, &PUSH_COUNTERS, |bulk| {
            bulk.observe_once_in_first(black_box(BULK_EVENT_COUNT));
        });
    });

    group.bench_function("idle_histograms_st", |b| {
        bench_push(b, &PUSH_HISTOGRAMS, |_| ());
    });

    group.bench_function("dirty_buckets_low_histograms_st", |b| {
        bench_push(b, &PUSH_HISTOGRAMS, |bulk| {
            bulk.observe_magnitudes(black_box(ONE_BUCKET_MAGNITUDES));
        });
    });

    group.bench_function("dirty_buckets_high_histograms_st", |b| {
        bench_push(b, &PUSH_HISTOGRAMS, |bulk| {
            bulk.observe_magnitudes(black_box(HISTOGRAM_BUCKETS));
        });
    });

    group.finish();
}

fn observation_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("nm_push_bulk/observation");

    group.bench_function("counters_st_push", |b| {
        b.iter(|| {
            PUSH_COUNTERS.with(|bulk| bulk.observe_once_in_first(black_box(BULK_EVENT_COUNT)));
        });
    });

    group.bench_function("counters_st_pull", |b| {
        b.iter(|| {
            PULL_COUNTERS.with(|events| {
                for event in events {
                    event.observe_once();
                }
            });
        });
    });

    group.bench_function("histograms_st_push", |b| {
        b.iter(|| {
            PUSH_HISTOGRAMS.with(|bulk| bulk.observe_magnitudes(black_box(ONE_BUCKET_MAGNITUDES)));
        });
    });

    group.bench_function("histograms_st_pull", |b| {
        b.iter(|| {
            PULL_HISTOGRAMS.with(|events| {
                for event in events {
                    event.observe(black_box(FIRST_BUCKET_MAGNITUDE));
                }
            });
        });
    });

    group.finish();
}

/// Measures one push per iteration, with the observations that decide how much of the registry is
/// dirty made outside the measured region.
///
/// Criterion's `iter_batched()` with `BatchSize::PerIteration` is the usual way to keep
/// per-iteration setup out of a measurement, but its overhead is of the same order as a push over
/// a registry that has nothing to copy. Preparing once per push is also what keeps a dirty pair
/// dirty for exactly one push; preparing a whole batch up front would leave every push but the
/// first one idle.
///
/// Each reported figure therefore includes one clock read, which is negligible next to a push
/// over a registry of this size.
fn bench_push(b: &mut Bencher<'_>, bulk: &'static LocalKey<PushBulk>, prepare: impl Fn(&PushBulk)) {
    b.iter_custom(|iterations| {
        let mut elapsed = Duration::ZERO;

        for _ in 0..iterations {
            elapsed = elapsed.saturating_add(bulk.with(|bulk| {
                prepare(bulk);

                let started = Instant::now();
                bulk.pusher.push();
                started.elapsed()
            }));
        }

        elapsed
    });
}

/// A bulk registry of events that use the push model, together with the pusher that publishes
/// them.
///
/// A push walks every pair registered with the pusher, so this is the object under test in every
/// `push` benchmark of this file: the benchmarks differ only in how much of the registry they
/// observe between two pushes.
struct PushBulk {
    pusher: MetricsPusher,
    events: Vec<Event<Push>>,
}

impl PushBulk {
    fn new(name_prefix: &str, buckets: &'static [Magnitude]) -> Self {
        let pusher = MetricsPusher::new();

        let events = (0..BULK_EVENT_COUNT)
            .map(|index| {
                let mut builder = Event::builder().name(format!("{name_prefix}_{index}"));

                if !buckets.is_empty() {
                    builder = builder.histogram(buckets);
                }

                builder.pusher(&pusher).build()
            })
            .collect();

        Self { pusher, events }
    }

    /// Observes one occurrence in each of the first `count` events, leaving those pairs dirty for
    /// the next push.
    fn observe_once_in_first(&self, count: usize) {
        for event in self.events.iter().take(count) {
            event.observe_once();
        }
    }

    /// Observes one occurrence of each magnitude in every event, leaving every pair dirty with one
    /// dirty bucket per magnitude.
    fn observe_magnitudes(&self, magnitudes: &[Magnitude]) {
        for event in &self.events {
            for magnitude in magnitudes {
                event.observe(*magnitude);
            }
        }
    }
}

fn make_pull_bulk(name_prefix: &str, buckets: &'static [Magnitude]) -> Vec<Event<Pull>> {
    (0..BULK_EVENT_COUNT)
        .map(|index| {
            let mut builder = Event::builder().name(format!("{name_prefix}_{index}"));

            if !buckets.is_empty() {
                builder = builder.histogram(buckets);
            }

            builder.build()
        })
        .collect()
}

/// Events per bulk registry. Chosen to be far larger than the registry of a typical thread, so
/// that the per-event cost of a push is what the measurement reports.
const BULK_EVENT_COUNT: usize = 1000;

/// Events observed between two pushes in the sparse workload, representing a push interval in
/// which only a small share of a large registry saw any traffic. This is the workload that the
/// pusher's skipping of unobserved pairs exists for.
const SPARSE_DIRTY_EVENTS: usize = 10;

/// Bucket boundaries of a histogram at the upper end of what callers configure, chosen to expose
/// the per-bucket cost of publishing.
const HISTOGRAM_BUCKETS: &[Magnitude] = &[
    0, 1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536, 131072,
    262144, 524288, 1048576, 2097152, 4194304, 8388608, 16777216, 33554432, 67108864, 134217728,
    268435456, 536870912, 1073741824,
];

/// Lands in the first bucket of the histogram, which is the shortest bucket scan.
const FIRST_BUCKET_MAGNITUDE: Magnitude = 0;

/// Magnitudes that mark a single histogram bucket dirty, for the low end of the dirty-bucket axis.
const ONE_BUCKET_MAGNITUDES: &[Magnitude] = &[FIRST_BUCKET_MAGNITUDE];

/// Absence of bucket boundaries, which is what makes an event a counter.
const NO_BUCKETS: &[Magnitude] = &[];

thread_local! {
    static PUSH_COUNTERS: PushBulk = PushBulk::new("bench_bulk_push_counter", NO_BUCKETS);
    static PUSH_HISTOGRAMS: PushBulk =
        PushBulk::new("bench_bulk_push_histogram", HISTOGRAM_BUCKETS);

    static PULL_COUNTERS: Vec<Event<Pull>> =
        make_pull_bulk("bench_bulk_pull_counter", NO_BUCKETS);
    static PULL_HISTOGRAMS: Vec<Event<Pull>> =
        make_pull_bulk("bench_bulk_pull_histogram", HISTOGRAM_BUCKETS);
}

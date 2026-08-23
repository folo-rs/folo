//! Callgrind benchmarks for the elementary operations of the `nm` package: observing events,
//! publishing the metrics of events that use the push model, and collecting reports.
//!
//! Paired with `nm_performance.rs`, which covers the same scenarios under wall-clock measurement.
//! Each function here carries the name of its Criterion counterpart, so
//! `observation_counter_st_pull` is the instruction-count view of the Criterion benchmark
//! `nm_performance/observation/counter_st_pull`. See that file for the name vocabulary.
//!
//! Multi-threaded scenarios have no counterpart here: instruction counts do not capture the
//! contention that those benchmarks exist to measure. Neither do the duration observation
//! benchmarks, whose cost is dominated by a platform clock read that the simulator models at a
//! fixed cost.
//!
//! # Collection scope
//!
//! The collection case uses the same low-cardinality event set as its Criterion counterpart.
//! Collection allocates a report and one metrics entry per event, so its instruction count is a
//! composite of registry traversal and allocator behavior. The high-cardinality Criterion case
//! has no Callgrind counterpart because allocation and cache effects dominate at that scale.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]
#![cfg_attr(
    target_os = "linux",
    expect(
        clippy::exit,
        clippy::missing_docs_in_private_items,
        unused_qualifications,
        reason = "These lints originate in Gungraun macro expansion and cannot be addressed in \
          this benchmark."
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {
    // Gungraun requires Valgrind, which is Linux-only. On other platforms this bench target
    // compiles to a no-op so `cargo build --all-targets` still works.
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig, main};
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "linux")]
main!(
    config = LibraryBenchmarkConfig::default().tool(
        Callgrind::default()
            .args(["--branch-sim=yes"])
            .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
    ),
    library_benchmark_groups = [observation, collection, push]
);

#[cfg(target_os = "linux")]
mod linux {
    use std::hint::black_box;

    use gungraun::prelude::*;
    use nm::{Event, Magnitude, MetricsPusher, Push, Report};

    // Every benchmark function here measures exactly one scenario, which its name identifies, so
    // each carries a single case whose identifier names what the setup produces: the state that
    // the measured operation runs against.

    #[library_benchmark]
    #[bench::state(make_pull_counter("cg_pull_counter"))]
    fn observation_counter_st_pull(event: Event) -> Event {
        black_box(&event).observe_once();
        event
    }

    #[library_benchmark]
    #[bench::state(make_pull_counter("cg_pull_plain"))]
    fn observation_plain_st_pull(event: Event) -> Event {
        black_box(&event).observe(black_box(PLAIN_MAGNITUDE));
        event
    }

    #[library_benchmark]
    #[bench::state(make_pull_counter("cg_pull_counter_batch_low"))]
    fn observation_counter_batch_low_st_pull(event: Event) -> Event {
        black_box(&event).batch(black_box(BATCH_LOW)).observe_once();
        event
    }

    #[library_benchmark]
    #[bench::state(make_pull_counter("cg_pull_counter_batch_high"))]
    fn observation_counter_batch_high_st_pull(event: Event) -> Event {
        black_box(&event)
            .batch(black_box(BATCH_HIGH))
            .observe_once();
        event
    }

    #[library_benchmark]
    #[bench::state(make_pull_histogram(
        "cg_pull_small_histogram_first_bucket",
        SMALL_HISTOGRAM_BUCKETS,
        FIRST_BUCKET_MAGNITUDE,
    ))]
    fn observation_small_histogram_first_bucket_st_pull(input: (Event, Magnitude)) -> Event {
        let (event, magnitude) = input;
        black_box(&event).observe(black_box(magnitude));
        event
    }

    #[library_benchmark]
    #[bench::state(make_pull_histogram(
        "cg_pull_small_histogram_last_bucket",
        SMALL_HISTOGRAM_BUCKETS,
        SMALL_HISTOGRAM_LAST_BUCKET_MAGNITUDE,
    ))]
    fn observation_small_histogram_last_bucket_st_pull(input: (Event, Magnitude)) -> Event {
        let (event, magnitude) = input;
        black_box(&event).observe(black_box(magnitude));
        event
    }

    #[library_benchmark]
    #[bench::state(make_pull_histogram(
        "cg_pull_small_histogram_above_range",
        SMALL_HISTOGRAM_BUCKETS,
        ABOVE_RANGE_MAGNITUDE,
    ))]
    fn observation_small_histogram_above_range_st_pull(input: (Event, Magnitude)) -> Event {
        let (event, magnitude) = input;
        black_box(&event).observe(black_box(magnitude));
        event
    }

    #[library_benchmark]
    #[bench::state(make_pull_histogram(
        "cg_pull_large_histogram_first_bucket",
        LARGE_HISTOGRAM_BUCKETS,
        FIRST_BUCKET_MAGNITUDE,
    ))]
    fn observation_large_histogram_first_bucket_st_pull(input: (Event, Magnitude)) -> Event {
        let (event, magnitude) = input;
        black_box(&event).observe(black_box(magnitude));
        event
    }

    #[library_benchmark]
    #[bench::state(make_pull_histogram(
        "cg_pull_large_histogram_last_bucket",
        LARGE_HISTOGRAM_BUCKETS,
        LARGE_HISTOGRAM_LAST_BUCKET_MAGNITUDE,
    ))]
    fn observation_large_histogram_last_bucket_st_pull(input: (Event, Magnitude)) -> Event {
        let (event, magnitude) = input;
        black_box(&event).observe(black_box(magnitude));
        event
    }

    #[library_benchmark]
    #[bench::state(make_pull_histogram(
        "cg_pull_large_histogram_above_range",
        LARGE_HISTOGRAM_BUCKETS,
        ABOVE_RANGE_MAGNITUDE,
    ))]
    fn observation_large_histogram_above_range_st_pull(input: (Event, Magnitude)) -> Event {
        let (event, magnitude) = input;
        black_box(&event).observe(black_box(magnitude));
        event
    }

    #[library_benchmark]
    #[bench::state(make_push_counter("cg_push_counter"))]
    fn observation_counter_st_push(
        input: (Event<Push>, MetricsPusher),
    ) -> (Event<Push>, MetricsPusher) {
        let (event, pusher) = input;
        black_box(&event).observe_once();
        (event, pusher)
    }

    #[library_benchmark]
    #[bench::state(make_push_counter("cg_push_plain"))]
    fn observation_plain_st_push(
        input: (Event<Push>, MetricsPusher),
    ) -> (Event<Push>, MetricsPusher) {
        let (event, pusher) = input;
        black_box(&event).observe(black_box(PLAIN_MAGNITUDE));
        (event, pusher)
    }

    #[library_benchmark]
    #[bench::state(make_push_counter("cg_push_counter_batch_low"))]
    fn observation_counter_batch_low_st_push(
        input: (Event<Push>, MetricsPusher),
    ) -> (Event<Push>, MetricsPusher) {
        let (event, pusher) = input;
        black_box(&event).batch(black_box(BATCH_LOW)).observe_once();
        (event, pusher)
    }

    #[library_benchmark]
    #[bench::state(make_push_counter("cg_push_counter_batch_high"))]
    fn observation_counter_batch_high_st_push(
        input: (Event<Push>, MetricsPusher),
    ) -> (Event<Push>, MetricsPusher) {
        let (event, pusher) = input;
        black_box(&event)
            .batch(black_box(BATCH_HIGH))
            .observe_once();
        (event, pusher)
    }

    #[library_benchmark]
    #[bench::state(make_push_histogram(
        "cg_push_small_histogram_first_bucket",
        SMALL_HISTOGRAM_BUCKETS,
        FIRST_BUCKET_MAGNITUDE,
    ))]
    fn observation_small_histogram_first_bucket_st_push(
        input: (Event<Push>, MetricsPusher, Magnitude),
    ) -> (Event<Push>, MetricsPusher) {
        let (event, pusher, magnitude) = input;
        black_box(&event).observe(black_box(magnitude));
        (event, pusher)
    }

    #[library_benchmark]
    #[bench::state(make_push_histogram(
        "cg_push_small_histogram_last_bucket",
        SMALL_HISTOGRAM_BUCKETS,
        SMALL_HISTOGRAM_LAST_BUCKET_MAGNITUDE,
    ))]
    fn observation_small_histogram_last_bucket_st_push(
        input: (Event<Push>, MetricsPusher, Magnitude),
    ) -> (Event<Push>, MetricsPusher) {
        let (event, pusher, magnitude) = input;
        black_box(&event).observe(black_box(magnitude));
        (event, pusher)
    }

    #[library_benchmark]
    #[bench::state(make_push_histogram(
        "cg_push_small_histogram_above_range",
        SMALL_HISTOGRAM_BUCKETS,
        ABOVE_RANGE_MAGNITUDE,
    ))]
    fn observation_small_histogram_above_range_st_push(
        input: (Event<Push>, MetricsPusher, Magnitude),
    ) -> (Event<Push>, MetricsPusher) {
        let (event, pusher, magnitude) = input;
        black_box(&event).observe(black_box(magnitude));
        (event, pusher)
    }

    #[library_benchmark]
    #[bench::state(make_push_histogram(
        "cg_push_large_histogram_first_bucket",
        LARGE_HISTOGRAM_BUCKETS,
        FIRST_BUCKET_MAGNITUDE,
    ))]
    fn observation_large_histogram_first_bucket_st_push(
        input: (Event<Push>, MetricsPusher, Magnitude),
    ) -> (Event<Push>, MetricsPusher) {
        let (event, pusher, magnitude) = input;
        black_box(&event).observe(black_box(magnitude));
        (event, pusher)
    }

    #[library_benchmark]
    #[bench::state(make_push_histogram(
        "cg_push_large_histogram_last_bucket",
        LARGE_HISTOGRAM_BUCKETS,
        LARGE_HISTOGRAM_LAST_BUCKET_MAGNITUDE,
    ))]
    fn observation_large_histogram_last_bucket_st_push(
        input: (Event<Push>, MetricsPusher, Magnitude),
    ) -> (Event<Push>, MetricsPusher) {
        let (event, pusher, magnitude) = input;
        black_box(&event).observe(black_box(magnitude));
        (event, pusher)
    }

    #[library_benchmark]
    #[bench::state(make_push_histogram(
        "cg_push_large_histogram_above_range",
        LARGE_HISTOGRAM_BUCKETS,
        ABOVE_RANGE_MAGNITUDE,
    ))]
    fn observation_large_histogram_above_range_st_push(
        input: (Event<Push>, MetricsPusher, Magnitude),
    ) -> (Event<Push>, MetricsPusher) {
        let (event, pusher, magnitude) = input;
        black_box(&event).observe(black_box(magnitude));
        (event, pusher)
    }

    // The report is returned rather than dropped here, so that releasing it happens outside the
    // measured body. Gungraun drops the returned value after the measurement ends.
    #[library_benchmark]
    #[bench::state(make_collection_state())]
    fn collection_collect_low_cardinality_st(state: CollectionState) -> (CollectionState, Report) {
        let report = Report::collect();
        (state, report)
    }

    #[library_benchmark]
    #[bench::state(make_push_scenario("cg_push_idle", NOTHING_DIRTY, NO_BUCKETS))]
    fn push_idle_st(scenario: PushScenario) -> PushScenario {
        scenario.pusher.push();
        scenario
    }

    #[library_benchmark]
    #[bench::state(make_push_scenario(
        "cg_push_dirty_events_low",
        SPARSE_DIRTY_EVENTS,
        NO_BUCKETS
    ))]
    fn push_dirty_events_low_st(scenario: PushScenario) -> PushScenario {
        scenario.pusher.push();
        scenario
    }

    #[library_benchmark]
    #[bench::state(make_push_scenario(
        "cg_push_dirty_events_high",
        PUSH_COUNTER_EVENTS,
        NO_BUCKETS
    ))]
    fn push_dirty_events_high_st(scenario: PushScenario) -> PushScenario {
        scenario.pusher.push();
        scenario
    }

    #[library_benchmark]
    #[bench::state(make_push_scenario(
        "cg_push_dirty_buckets_low",
        NOTHING_DIRTY,
        ONE_BUCKET_MAGNITUDES
    ))]
    fn push_dirty_buckets_low_st(scenario: PushScenario) -> PushScenario {
        scenario.pusher.push();
        scenario
    }

    #[library_benchmark]
    #[bench::state(make_push_scenario(
        "cg_push_dirty_buckets_high",
        NOTHING_DIRTY,
        LARGE_HISTOGRAM_BUCKETS
    ))]
    fn push_dirty_buckets_high_st(scenario: PushScenario) -> PushScenario {
        scenario.pusher.push();
        scenario
    }

    library_benchmark_group!(
        name = observation,
        benchmarks = [
            observation_counter_st_pull,
            observation_plain_st_pull,
            observation_counter_batch_low_st_pull,
            observation_counter_batch_high_st_pull,
            observation_small_histogram_first_bucket_st_pull,
            observation_small_histogram_last_bucket_st_pull,
            observation_small_histogram_above_range_st_pull,
            observation_large_histogram_first_bucket_st_pull,
            observation_large_histogram_last_bucket_st_pull,
            observation_large_histogram_above_range_st_pull,
            observation_counter_st_push,
            observation_plain_st_push,
            observation_counter_batch_low_st_push,
            observation_counter_batch_high_st_push,
            observation_small_histogram_first_bucket_st_push,
            observation_small_histogram_last_bucket_st_push,
            observation_small_histogram_above_range_st_push,
            observation_large_histogram_first_bucket_st_push,
            observation_large_histogram_last_bucket_st_push,
            observation_large_histogram_above_range_st_push,
        ]
    );

    library_benchmark_group!(
        name = collection,
        benchmarks = [collection_collect_low_cardinality_st]
    );

    library_benchmark_group!(
        name = push,
        benchmarks = [
            push_idle_st,
            push_dirty_events_low_st,
            push_dirty_events_high_st,
            push_dirty_buckets_low_st,
            push_dirty_buckets_high_st,
        ]
    );

    /// The registry that the `push` benchmarks publish from: one pusher holding both counter
    /// events and histogram events, in a state where a chosen part of it has been observed since
    /// the previous push.
    ///
    /// A push walks every pair registered with the pusher, so all `push` benchmarks share one
    /// scenario shape and differ only in how much of it is dirty when the push happens. The scan
    /// cost is then identical across the scenarios and the cost of copying dirty data is the only
    /// variable. This mirrors the scenario that the Criterion `push` benchmarks measure.
    struct PushScenario {
        pusher: MetricsPusher,
        _counters: Vec<Event<Push>>,
        _histograms: Vec<Event<Push>>,
    }

    /// The registry that the `collection` benchmark reads: one event of each shape this file
    /// measures, under each publishing model, all carrying an observation so that the collected
    /// report contains data rather than empty entries.
    struct CollectionState {
        _pull_counter: Event,
        _pull_plain: Event,
        _pull_small_histogram: Event,
        _pull_large_histogram: Event,
        _push_counter: Event<Push>,
        _push_plain: Event<Push>,
        _push_small_histogram: Event<Push>,
        _push_large_histogram: Event<Push>,
    }

    // Distinct event names per case keep us safe in the unlikely scenario that Gungraun does not
    // fully isolate processes per case.

    fn make_pull_counter(name: &'static str) -> Event {
        Event::builder().name(name).build()
    }

    fn make_pull_histogram(
        name: &'static str,
        buckets: &'static [Magnitude],
        magnitude: Magnitude,
    ) -> (Event, Magnitude) {
        let event = Event::builder().name(name).histogram(buckets).build();
        (event, magnitude)
    }

    fn make_push_counter(name: &'static str) -> (Event<Push>, MetricsPusher) {
        let pusher = MetricsPusher::new();
        let event = Event::builder().name(name).pusher(&pusher).build();
        (event, pusher)
    }

    fn make_push_histogram(
        name: &'static str,
        buckets: &'static [Magnitude],
        magnitude: Magnitude,
    ) -> (Event<Push>, MetricsPusher, Magnitude) {
        let pusher = MetricsPusher::new();
        let event = Event::builder()
            .name(name)
            .histogram(buckets)
            .pusher(&pusher)
            .build();
        (event, pusher, magnitude)
    }

    /// Builds a push scenario and leaves `dirty_counters` counter events and one bucket per entry
    /// of `dirty_bucket_magnitudes` in every histogram event observed since the last push.
    fn make_push_scenario(
        name_prefix: &'static str,
        dirty_counters: usize,
        dirty_bucket_magnitudes: &'static [Magnitude],
    ) -> PushScenario {
        let pusher = MetricsPusher::new();

        let counters: Vec<Event<Push>> = (0..PUSH_COUNTER_EVENTS)
            .map(|index| {
                Event::builder()
                    .name(format!("{name_prefix}_counter_{index}"))
                    .pusher(&pusher)
                    .build()
            })
            .collect();

        let histograms: Vec<Event<Push>> = (0..PUSH_HISTOGRAM_EVENTS)
            .map(|index| {
                Event::builder()
                    .name(format!("{name_prefix}_histogram_{index}"))
                    .histogram(LARGE_HISTOGRAM_BUCKETS)
                    .pusher(&pusher)
                    .build()
            })
            .collect();

        // Observe and publish everything once, so that the scenario differs from the idle state
        // only in what the second round of observations below marks dirty.
        for event in &counters {
            event.observe_once();
        }

        for event in &histograms {
            for magnitude in LARGE_HISTOGRAM_BUCKETS {
                event.observe(*magnitude);
            }
        }

        pusher.push();

        for event in counters.iter().take(dirty_counters) {
            event.observe_once();
        }

        for event in &histograms {
            for magnitude in dirty_bucket_magnitudes {
                event.observe(*magnitude);
            }
        }

        PushScenario {
            pusher,
            _counters: counters,
            _histograms: histograms,
        }
    }

    fn make_collection_state() -> CollectionState {
        let pusher = MetricsPusher::new();

        let pull_counter = Event::builder().name("cg_collect_pull_counter").build();
        let pull_plain = Event::builder().name("cg_collect_pull_plain").build();
        let pull_small_histogram = Event::builder()
            .name("cg_collect_pull_small_histogram")
            .histogram(SMALL_HISTOGRAM_BUCKETS)
            .build();
        let pull_large_histogram = Event::builder()
            .name("cg_collect_pull_large_histogram")
            .histogram(LARGE_HISTOGRAM_BUCKETS)
            .build();

        let push_counter = Event::builder()
            .name("cg_collect_push_counter")
            .pusher(&pusher)
            .build();
        let push_plain = Event::builder()
            .name("cg_collect_push_plain")
            .pusher(&pusher)
            .build();
        let push_small_histogram = Event::builder()
            .name("cg_collect_push_small_histogram")
            .histogram(SMALL_HISTOGRAM_BUCKETS)
            .pusher(&pusher)
            .build();
        let push_large_histogram = Event::builder()
            .name("cg_collect_push_large_histogram")
            .histogram(LARGE_HISTOGRAM_BUCKETS)
            .pusher(&pusher)
            .build();

        pull_counter.observe_once();
        pull_plain.observe(PLAIN_MAGNITUDE);
        pull_small_histogram.observe(FIRST_BUCKET_MAGNITUDE);
        pull_large_histogram.observe(FIRST_BUCKET_MAGNITUDE);
        push_counter.observe_once();
        push_plain.observe(PLAIN_MAGNITUDE);
        push_small_histogram.observe(FIRST_BUCKET_MAGNITUDE);
        push_large_histogram.observe(FIRST_BUCKET_MAGNITUDE);

        // Collection reads the shared side of each push pair, so the push events only carry data
        // once they have been published.
        pusher.push();

        CollectionState {
            _pull_counter: pull_counter,
            _pull_plain: pull_plain,
            _pull_small_histogram: pull_small_histogram,
            _pull_large_histogram: pull_large_histogram,
            _push_counter: push_counter,
            _push_plain: push_plain,
            _push_small_histogram: push_small_histogram,
            _push_large_histogram: push_large_histogram,
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

    /// Bucket boundaries of a histogram at the upper end of what callers configure, chosen to
    /// expose the per-bucket cost of both the bucket scan and publishing.
    const LARGE_HISTOGRAM_BUCKETS: &[Magnitude] = &[
        0, 1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536,
        131072, 262144, 524288, 1048576, 2097152, 4194304, 8388608, 16777216, 33554432, 67108864,
        134217728, 268435456, 536870912, 1073741824,
    ];

    /// Lands in the first bucket of either histogram, which is the shortest bucket scan.
    const FIRST_BUCKET_MAGNITUDE: Magnitude = 0;

    /// The largest boundary configured for the small histogram, so an observation of it lands in
    /// the last bucket after scanning every other bucket.
    const SMALL_HISTOGRAM_LAST_BUCKET_MAGNITUDE: Magnitude = 10_000;

    /// The largest boundary configured for the large histogram, so an observation of it lands in
    /// the last bucket after scanning every other bucket.
    const LARGE_HISTOGRAM_LAST_BUCKET_MAGNITUDE: Magnitude = 1_073_741_824;

    /// Above every configured boundary of either histogram, so the observation lands in the
    /// implicit final range after a scan that matches no bucket.
    const ABOVE_RANGE_MAGNITUDE: Magnitude = Magnitude::MAX;

    /// Magnitudes that mark a single histogram bucket dirty, for the low end of the dirty-bucket
    /// axis.
    const ONE_BUCKET_MAGNITUDES: &[Magnitude] = &[FIRST_BUCKET_MAGNITUDE];

    /// Leaves every histogram event of a push scenario idle.
    const NO_BUCKETS: &[Magnitude] = &[];

    /// Leaves every counter event of a push scenario idle.
    const NOTHING_DIRTY: usize = 0;

    /// Counter events registered with the push scenario pusher, matching the Criterion push
    /// scenario so that both views describe a registry of the same size.
    const PUSH_COUNTER_EVENTS: usize = 64;

    /// Histogram events registered with the push scenario pusher, matching the counter event
    /// count so that both kinds contribute equally to the cost of scanning the registry.
    const PUSH_HISTOGRAM_EVENTS: usize = 64;

    /// Events observed between two pushes in the sparse workload, representing a push interval in
    /// which only a small share of the registered events saw any traffic. This is the workload
    /// that the pusher's skipping of unobserved pairs exists for.
    const SPARSE_DIRTY_EVENTS: usize = 4;
}

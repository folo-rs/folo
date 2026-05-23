//! Cycle-accurate benchmarks for the observation hot path of the `nm` package.
//!
//! Paired with `nm_performance.rs` which covers the same operations under
//! wall-clock measurement.
//!
//! Covers both publishing models (pull / push), counter and histogram events,
//! and the most-traveled and worst-case histogram bucket positions
//! (hit-first / hit-last / miss).
//!
//! Multi-threaded behavior is intentionally out of scope: see the existing
//! Criterion + `par_bench` suite in `packages/nm/benches/` for that.

#![allow(
    missing_docs,
    reason = "no need for API documentation on benchmark code"
)]
// Gungraun macro expansions trigger lints we cannot control from this file.
#![cfg_attr(
    target_os = "linux",
    allow(
        clippy::absolute_paths,
        clippy::allow_attributes_without_reason,
        clippy::exhaustive_structs,
        clippy::partial_pub_fields,
        clippy::pub_underscore_fields,
        clippy::cognitive_complexity,
        clippy::unnecessary_wraps,
        clippy::ignore_without_reason,
        clippy::default_trait_access,
        clippy::needless_pass_by_value,
        clippy::missing_assert_message,
        clippy::elidable_lifetime_names,
        clippy::needless_pass_by_ref_mut,
        clippy::doc_markdown,
        clippy::needless_for_each,
        clippy::redundant_clone,
        clippy::missing_docs_in_private_items,
        clippy::exit,
        unused_imports,
        unused_qualifications,
        unreachable_pub,
        missing_debug_implementations,
        unnameable_types,
        non_local_definitions,
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {
    // Gungraun requires Valgrind, which is Linux-only. On other platforms this
    // bench target compiles to a no-op so `cargo build --all-targets` still works.
}

#[cfg(target_os = "linux")]
extern crate gungraun;

#[cfg(target_os = "linux")]
use std::hint::black_box;

#[cfg(target_os = "linux")]
use gungraun::prelude::*;
#[cfg(target_os = "linux")]
use nm::{Event, Magnitude, MetricsPusher, Push, Report};

#[cfg(target_os = "linux")]
const SMALL_HISTOGRAM_BUCKETS: &[Magnitude] = &[0, 10, 100, 1000, 10000];

#[cfg(target_os = "linux")]
const LARGE_HISTOGRAM_BUCKETS: &[Magnitude] = &[
    0, 1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536, 131072,
    262144, 524288, 1048576, 2097152, 4194304, 8388608, 16777216, 33554432, 67108864, 134217728,
    268435456, 536870912, 1073741824,
];

// The largest bucket boundary of each histogram. We do not use `Magnitude::MAX`
// for the "last bucket" case because that would land in the implicit
// "overflow" range; we want to land in a defined bucket.
#[cfg(target_os = "linux")]
const SMALL_HISTOGRAM_LAST_BUCKET_VALUE: Magnitude = 10_000;
#[cfg(target_os = "linux")]
const LARGE_HISTOGRAM_LAST_BUCKET_VALUE: Magnitude = 1_073_741_824;

// Distinct event names per case keep us safe in the unlikely scenario that
// Gungraun does not fully isolate processes per case.

#[cfg(target_os = "linux")]
fn make_pull_counter(name: &'static str) -> Event {
    Event::builder().name(name).build()
}

#[cfg(target_os = "linux")]
fn make_pull_small_histogram(name: &'static str) -> Event {
    Event::builder()
        .name(name)
        .histogram(SMALL_HISTOGRAM_BUCKETS)
        .build()
}

#[cfg(target_os = "linux")]
fn make_pull_large_histogram(name: &'static str) -> Event {
    Event::builder()
        .name(name)
        .histogram(LARGE_HISTOGRAM_BUCKETS)
        .build()
}

#[cfg(target_os = "linux")]
fn make_push_counter(name: &'static str) -> (Event<Push>, MetricsPusher) {
    let pusher = MetricsPusher::new();
    let event = Event::builder().name(name).pusher(&pusher).build();
    (event, pusher)
}

#[cfg(target_os = "linux")]
fn make_push_small_histogram(name: &'static str) -> (Event<Push>, MetricsPusher) {
    let pusher = MetricsPusher::new();
    let event = Event::builder()
        .name(name)
        .histogram(SMALL_HISTOGRAM_BUCKETS)
        .pusher(&pusher)
        .build();
    (event, pusher)
}

#[cfg(target_os = "linux")]
fn make_push_large_histogram(name: &'static str) -> (Event<Push>, MetricsPusher) {
    let pusher = MetricsPusher::new();
    let event = Event::builder()
        .name(name)
        .histogram(LARGE_HISTOGRAM_BUCKETS)
        .pusher(&pusher)
        .build();
    (event, pusher)
}

#[cfg(target_os = "linux")]
fn make_pull_small_histogram_with(name: &'static str, magnitude: Magnitude) -> (Event, Magnitude) {
    (make_pull_small_histogram(name), magnitude)
}

#[cfg(target_os = "linux")]
fn make_pull_large_histogram_with(name: &'static str, magnitude: Magnitude) -> (Event, Magnitude) {
    (make_pull_large_histogram(name), magnitude)
}

#[cfg(target_os = "linux")]
fn make_push_small_histogram_with(
    name: &'static str,
    magnitude: Magnitude,
) -> (Event<Push>, MetricsPusher, Magnitude) {
    let (event, pusher) = make_push_small_histogram(name);
    (event, pusher, magnitude)
}

#[cfg(target_os = "linux")]
fn make_push_large_histogram_with(
    name: &'static str,
    magnitude: Magnitude,
) -> (Event<Push>, MetricsPusher, Magnitude) {
    let (event, pusher) = make_push_large_histogram(name);
    (event, pusher, magnitude)
}

// ---------- Pull model ----------

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::observe_once(make_pull_counter("cycles_pull_counter_once"))]
fn pull_counter_observe_once(event: Event) -> Event {
    black_box(&event).observe_once();
    event
}

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::observe_value(make_pull_counter("cycles_pull_counter_value"))]
fn pull_counter_observe_value(event: Event) -> Event {
    black_box(&event).observe(black_box(42));
    event
}

#[cfg(target_os = "linux")]
fn make_pull_counter_batch(n: usize) -> (Event, usize) {
    // Include `n` so each case gets its own registry entry even if the
    // Gungraun process isolation guarantees were ever to be weakened.
    let event = Event::builder()
        .name(format!("cycles_pull_counter_batch_{n}"))
        .build();
    (event, n)
}

// Batching short-circuits the per-observation work: one call records N occurrences.
// The instruction count should be nearly flat across `n`.
#[cfg(target_os = "linux")]
#[library_benchmark]
#[benches::sizes(
    args = [1_usize, 10_usize, 100_usize, 1_000_usize, 10_000_usize],
    setup = make_pull_counter_batch,
)]
fn pull_counter_observe_batch(input: (Event, usize)) -> Event {
    let (event, n) = input;
    black_box(&event).batch(black_box(n)).observe_once();
    event
}

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::hit_first(make_pull_small_histogram_with("cycles_pull_small_histo_first", 0))]
#[bench::hit_last(make_pull_small_histogram_with(
    "cycles_pull_small_histo_last",
    SMALL_HISTOGRAM_LAST_BUCKET_VALUE,
))]
#[bench::miss(make_pull_small_histogram_with("cycles_pull_small_histo_miss", Magnitude::MAX))]
fn pull_small_histogram_observe(input: (Event, Magnitude)) -> Event {
    let (event, magnitude) = input;
    black_box(&event).observe(black_box(magnitude));
    event
}

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::hit_first(make_pull_large_histogram_with("cycles_pull_large_histo_first", 0))]
#[bench::hit_last(make_pull_large_histogram_with(
    "cycles_pull_large_histo_last",
    LARGE_HISTOGRAM_LAST_BUCKET_VALUE,
))]
#[bench::miss(make_pull_large_histogram_with("cycles_pull_large_histo_miss", Magnitude::MAX))]
fn pull_large_histogram_observe(input: (Event, Magnitude)) -> Event {
    let (event, magnitude) = input;
    black_box(&event).observe(black_box(magnitude));
    event
}

// ---------- Push model ----------

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::observe_once(make_push_counter("cycles_push_counter_once"))]
fn push_counter_observe_once(input: (Event<Push>, MetricsPusher)) -> (Event<Push>, MetricsPusher) {
    let (event, pusher) = input;
    black_box(&event).observe_once();
    (event, pusher)
}

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::observe_value(make_push_counter("cycles_push_counter_value"))]
fn push_counter_observe_value(input: (Event<Push>, MetricsPusher)) -> (Event<Push>, MetricsPusher) {
    let (event, pusher) = input;
    black_box(&event).observe(black_box(42));
    (event, pusher)
}

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::hit_first(make_push_small_histogram_with("cycles_push_small_histo_first", 0))]
#[bench::hit_last(make_push_small_histogram_with(
    "cycles_push_small_histo_last",
    SMALL_HISTOGRAM_LAST_BUCKET_VALUE,
))]
#[bench::miss(make_push_small_histogram_with("cycles_push_small_histo_miss", Magnitude::MAX))]
fn push_small_histogram_observe(
    input: (Event<Push>, MetricsPusher, Magnitude),
) -> (Event<Push>, MetricsPusher) {
    let (event, pusher, magnitude) = input;
    black_box(&event).observe(black_box(magnitude));
    (event, pusher)
}

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::hit_first(make_push_large_histogram_with("cycles_push_large_histo_first", 0))]
#[bench::hit_last(make_push_large_histogram_with(
    "cycles_push_large_histo_last",
    LARGE_HISTOGRAM_LAST_BUCKET_VALUE,
))]
#[bench::miss(make_push_large_histogram_with("cycles_push_large_histo_miss", Magnitude::MAX))]
fn push_large_histogram_observe(
    input: (Event<Push>, MetricsPusher, Magnitude),
) -> (Event<Push>, MetricsPusher) {
    let (event, pusher, magnitude) = input;
    black_box(&event).observe(black_box(magnitude));
    (event, pusher)
}

// ---------- Collection & push (steady-state aggregate) ----------
//
// The scenarios below construct the same shape of registry as
// `nm_performance.rs`'s `collect_st` / `push_st` Criterion benches:
// 4 pull events + 4 push events (counter / plain / small histo / large
// histo) on a single `MetricsPusher`, each observed once during setup.
//
// `Report::collect()` and `MetricsPusher::push()` operate on the
// process-wide registry, so matching the registry shape is the way to
// keep these cycle scenarios comparable to their Criterion pairs.

#[cfg(target_os = "linux")]
struct AggregateState {
    _pull_counter: Event,
    _pull_plain: Event,
    _pull_small_histogram: Event,
    _pull_large_histogram: Event,
    _push_counter: Event<Push>,
    _push_plain: Event<Push>,
    _push_small_histogram: Event<Push>,
    _push_large_histogram: Event<Push>,
    pusher: MetricsPusher,
}

#[cfg(target_os = "linux")]
fn make_aggregate_state(name_prefix: &'static str) -> AggregateState {
    let pusher = MetricsPusher::new();

    let pull_counter = Event::builder()
        .name(format!("{name_prefix}_pull_counter"))
        .build();
    let pull_plain = Event::builder()
        .name(format!("{name_prefix}_pull_plain"))
        .build();
    let pull_small_histogram = Event::builder()
        .name(format!("{name_prefix}_pull_small_histo"))
        .histogram(SMALL_HISTOGRAM_BUCKETS)
        .build();
    let pull_large_histogram = Event::builder()
        .name(format!("{name_prefix}_pull_large_histo"))
        .histogram(LARGE_HISTOGRAM_BUCKETS)
        .build();

    let push_counter = Event::builder()
        .name(format!("{name_prefix}_push_counter"))
        .pusher(&pusher)
        .build();
    let push_plain = Event::builder()
        .name(format!("{name_prefix}_push_plain"))
        .pusher(&pusher)
        .build();
    let push_small_histogram = Event::builder()
        .name(format!("{name_prefix}_push_small_histo"))
        .histogram(SMALL_HISTOGRAM_BUCKETS)
        .pusher(&pusher)
        .build();
    let push_large_histogram = Event::builder()
        .name(format!("{name_prefix}_push_large_histo"))
        .histogram(LARGE_HISTOGRAM_BUCKETS)
        .pusher(&pusher)
        .build();

    pull_counter.observe_once();
    pull_plain.observe(2);
    pull_small_histogram.observe(0);
    pull_large_histogram.observe(0);
    push_counter.observe_once();
    push_plain.observe(2);
    push_small_histogram.observe(0);
    push_large_histogram.observe(0);

    AggregateState {
        _pull_counter: pull_counter,
        _pull_plain: pull_plain,
        _pull_small_histogram: pull_small_histogram,
        _pull_large_histogram: pull_large_histogram,
        _push_counter: push_counter,
        _push_plain: push_plain,
        _push_small_histogram: push_small_histogram,
        _push_large_histogram: push_large_histogram,
        pusher,
    }
}

#[cfg(target_os = "linux")]
fn make_collect_state() -> AggregateState {
    make_aggregate_state("cycles_collect")
}

// Dirty: every push pair carries an unflushed observation. The push call has
// to copy all 4 pairs through the channel.
#[cfg(target_os = "linux")]
fn make_push_dirty_state() -> AggregateState {
    make_aggregate_state("cycles_push_dirty")
}

// Idle: every push pair has already been flushed. The push call still has to
// scan each pair to discover there is nothing to do. This is the steady
// state that `nm_performance.rs`'s `push_st` Criterion bench measures after
// the first iteration.
#[cfg(target_os = "linux")]
fn make_push_idle_state() -> AggregateState {
    let state = make_aggregate_state("cycles_push_idle");
    state.pusher.push();
    state
}

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::eight_events(make_collect_state())]
fn collect(state: AggregateState) -> AggregateState {
    drop(black_box(Report::collect()));
    state
}

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::dirty(make_push_dirty_state())]
#[bench::idle(make_push_idle_state())]
fn push(state: AggregateState) -> AggregateState {
    state.pusher.push();
    state
}

#[cfg(target_os = "linux")]
library_benchmark_group!(
    name = pull_group,
    benchmarks = [
        pull_counter_observe_once,
        pull_counter_observe_value,
        pull_counter_observe_batch,
        pull_small_histogram_observe,
        pull_large_histogram_observe,
    ]
);

#[cfg(target_os = "linux")]
library_benchmark_group!(
    name = push_group,
    benchmarks = [
        push_counter_observe_once,
        push_counter_observe_value,
        push_small_histogram_observe,
        push_large_histogram_observe,
    ]
);

#[cfg(target_os = "linux")]
library_benchmark_group!(name = aggregate_group, benchmarks = [collect, push]);

#[cfg(target_os = "linux")]
main!(
    library_benchmark_groups = pull_group,
    push_group,
    aggregate_group
);

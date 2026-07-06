#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Processor time tracking utilities for benchmarks and performance analysis.
//!
//! This package provides utilities to track processor time during code execution,
//! enabling analysis of processor usage patterns in benchmarks and performance tests.
//!
//! The core functionality includes:
//! - [`Session`] - Configures processor time tracking and provides access to tracking data
//! - [`Report`] - Thread-safe processor time statistics that can be merged and processed independently
//! - [`Operation`] - Measures per-iteration processor time for a repeated operation
//! - [`ThreadMeasurement`] / [`ProcessMeasurement`] - In-progress measurements awaiting an iteration count
//!
//! This package is not meant for use in production, serving only as a development tool.
//!
//! # Primary metric
//!
//! Processor time is not deterministic: it jitters run to run with system load
//! and scheduling. On top of that, Criterion chooses how many iterations to time
//! and how many to discard as warm-up, so a raw pooled **mean** silently folds
//! warm-up and one-off costs into the per-iteration figure and drifts as that
//! sample mix changes between runs.
//!
//! The headline metric is therefore the **warmup-robust slope**: an
//! iterations-weighted, through-origin fit of each span's total time against its
//! iteration count, which recovers the marginal per-iteration cost even when the
//! blend of warm-up and steady-state spans shifts. The slope is paired with a
//! closed-form 95% confidence interval so its run-to-run uncertainty can be
//! inspected: it collapses onto the point estimate when every span records the
//! same per-iteration value, and is omitted entirely when a single span leaves it
//! unestimable. The confidence interval is always computed as part of finalizing
//! a report — there is no cheaper "slope-only" path that skips it — so the stdout
//! summary simply displays the slope alone for readability while the JSON output
//! records the slope together with that confidence interval. The pooled mean stays
//! available through [`ReportOperation::mean`] for callers that still want it.
//!
//! # Benchmarking
//!
//! The recommended pattern drives measurement from Criterion's `iter_custom`,
//! feeding its chosen iteration count into
//! [`iterations`](ThreadMeasurement::iterations) so each recorded span covers a
//! whole sample rather than a single iteration:
//!
//! ```no_run
//! use std::hint::black_box;
//! use std::time::Instant;
//!
//! use all_the_time::Session;
//! use criterion::Criterion;
//!
//! fn main() {
//!     let session = Session::new();
//!     let operation = session.operation("my_operation");
//!
//!     let mut criterion = Criterion::default();
//!     criterion.bench_function("my_operation", |b| {
//!         b.iter_custom(|iters| {
//!             let start = Instant::now();
//!             let _span = operation.measure_thread().iterations(iters);
//!             for _ in 0..iters {
//!                 black_box(42_u64.wrapping_mul(2));
//!             }
//!             start.elapsed()
//!         });
//!     });
//!
//!     // When `session` is dropped, the recorded statistics are printed to
//!     // stdout and written to the Cargo target directory as JSON.
//! }
//! ```
//!
//! # Machine-readable output
//!
//! Dropping a [`Session`] writes machine-readable JSON files (one per operation)
//! into the Cargo target directory at `target/all_the_time/<operation>.json`,
//! with operation names sanitized to be filesystem-safe. A human-readable
//! summary is also printed to stdout.
//!
//! Each JSON file records the total and mean per-iteration processor time
//! together with the warmup-robust slope point estimate and its 95% confidence
//! interval (when estimable). The interval is a deterministic function of the
//! measured spans, so identical measurements always yield identical bounds. The
//! stdout summary displays the same slope (see "Primary metric") without the
//! interval purely for readability — the interval is computed regardless of which
//! output consumes it.
//!
//! These outputs are produced automatically, so a typical benchmark only needs
//! to create a session and record work.
//!
//! # Measuring a variable amount of work
//!
//! When the number of iterations is only known after the measured work has run —
//! for example a loop that drains a queue or runs until a budget is exhausted —
//! capture the measurement first and finalize it with
//! [`complete`](ThreadMeasurement::complete) once the count is known:
//!
//! ```
//! use all_the_time::Session;
//!
//! fn main() {
//!     let session = Session::new();
//! #   let session = session.no_stdout().no_file();
//!     let operation = session.operation("drain_queue");
//!
//!     let measurement = operation.measure_thread();
//!     let mut processed = 0_u64;
//!     for item in 0..10 {
//!         std::hint::black_box(item * 2); // do work while draining
//!         processed += 1;
//!     }
//!     measurement.complete(processed);
//!
//!     // Statistics are emitted automatically when `session` is dropped.
//! }
//! ```
//!
//! # Thread vs process processor time
//!
//! You can choose between tracking thread processor time or process processor
//! time. Both begin with a `measure_*` call and finalize with an explicit
//! iteration count (shown here with a single iteration for brevity; real
//! benchmarks use the `iter_custom` pattern above):
//!
//! ```
//! use all_the_time::Session;
//!
//! # fn main() {
//! let session = Session::new();
//! # let session = session.no_stdout().no_file();
//!
//! // Track thread processor time
//! {
//!     let op = session.operation("thread_work");
//!     let _span = op.measure_thread().iterations(1);
//!     // Work done here is measured for the current thread only
//! }
//!
//! // Track process processor time (all threads)
//! {
//!     let op = session.operation("process_work");
//!     let _span = op.measure_process().iterations(1);
//!     // Work done here is measured for the entire process
//! }
//! # }
//! ```
//!
//! # Overhead
//!
//! Capturing a single measurement by calling `measure_xyz()` incurs an overhead of
//! approximately 500 nanoseconds on an arbitrary sample machine. You are recommended to batch
//! your measurements over a whole Criterion sample (via `.iterations(iters)` from
//! `iter_custom`) to amortize this overhead. Operating without batching, on individual
//! iterations, is only viable for macrobenchmarks for which a single iteration is a
//! large unit of work (e.g. an HTTP request).
//!
//! # Session management
//!
//! Multiple [`Session`] instances can be used concurrently as they track processor time
//! independently. Each session maintains its own set of operations and statistics.
//!
//! While [`Session`] itself is single-threaded, reports from sessions can be converted to
//! thread-safe [`Report`] instances and sent to other threads for processing:
//!
//! ```
//! use std::thread;
//! use std::time::Duration;
//!
//! use all_the_time::Session;
//!
//! # fn main() {
//! let session = Session::new();
//! # let session = session.no_stdout().no_file();
//! let operation = session.operation("work");
//! let _span = operation.measure_thread().iterations(1);
//! // Some work happens here
//!
//! let report = session.to_report();
//!
//! // Reports are `Send`, so they can be moved to another thread for processing.
//! let total_time = thread::spawn(move || {
//!     report
//!         .operations()
//!         .map(|(_, op)| op.total_processor_time())
//!         .sum::<Duration>()
//! })
//! .join()
//! .unwrap();
//!
//! println!("Total processor time: {total_time:?}");
//! # }
//! ```

#![doc(
    html_logo_url = "https://media.githubusercontent.com/media/folo-rs/folo/refs/heads/main/packages/all_the_time/icon.png"
)]
#![doc(
    html_favicon_url = "https://raw.githubusercontent.com/folo-rs/folo/refs/heads/main/packages/all_the_time/icon.ico"
)]

mod constants;
mod finalized;
mod operation;
mod operation_metrics;
mod pal;
mod process_span;
mod report;
mod session;
mod statistics;
mod target_output;
mod thread_span;

pub(crate) use constants::*;
pub use finalized::*;
pub use operation::*;
pub(crate) use operation_metrics::*;
pub use process_span::*;
pub use report::*;
pub use session::*;
pub use statistics::OperationStatistics;
pub use thread_span::*;

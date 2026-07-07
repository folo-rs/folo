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
//! - [`ThreadSpan`] / [`ProcessSpan`] - Active measurements that record when dropped
//!
//! This package is not meant for use in production, serving only as a development tool.
//!
//! # Primary metric
//!
//! The headline figure is the per-iteration processor time, reported as a
//! **warmup-robust slope** rather than a plain mean. Early iterations pay one-off
//! warm-up costs that a mean would fold into every iteration, making it drift
//! between runs; the slope isolates the marginal per-iteration cost so the number
//! stays stable. The JSON output pairs the slope with a 95% confidence interval;
//! the stdout summary shows the slope alone. The plain mean is available
//! through [`ReportOperation::mean`].
//!
//! # Benchmarking
//!
//! The recommended pattern drives measurement from Criterion's `iter_custom`,
//! feeding its chosen iteration count into
//! [`iterations`](ThreadSpan::iterations) so each recorded span covers a
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
//! with operation names sanitized to be filesystem-safe. Each file records the
//! operation's total and per-iteration processor time. A human-readable summary
//! is also printed to stdout.
//!
//! These outputs are produced automatically, so a typical benchmark only needs
//! to create a session and record work.
//!
//! # Measuring a variable amount of work
//!
//! When the number of iterations is only known after the measured work has run —
//! for example a loop that drains a queue or runs until a budget is exhausted —
//! set the count once the work is done and let the span record as it drops:
//!
//! ```
//! use all_the_time::Session;
//!
//! fn main() {
//!     let session = Session::new();
//! #   let session = session.no_stdout().no_file();
//!     let operation = session.operation("drain_queue");
//!
//!     let span = operation.measure_thread();
//!     let mut processed = 0_u64;
//!     for item in 0..10 {
//!         std::hint::black_box(item * 2); // do work while draining
//!         processed += 1;
//!     }
//!     drop(span.iterations(processed));
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

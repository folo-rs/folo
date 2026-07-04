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
//! - [`ThreadSpan`] - Tracks thread processor time over a time period
//! - [`ProcessSpan`] - Tracks process processor time over a time period
//! - [`Operation`] - Measures per-iteration processor time for a repeated operation
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
//! blend of warm-up and steady-state spans shifts. Each figure is paired with a
//! bootstrap 95% confidence interval so its run-to-run uncertainty can be
//! inspected. The stdout summary leads with the slope alone for readability; the
//! JSON output records the slope together with that confidence interval, and the
//! pooled mean stays available through [`ReportOperation::mean`] for callers that
//! still want it.
//!
//! # Example
//!
//! ```rust
//! use all_the_time::Session;
//!
//! # fn main() {
//! let session = Session::new();
//! # let session = session.no_stdout().no_file();
//!
//! {
//!     let batch_op = session.operation("batch_work");
//!     let _span = batch_op.measure_thread().iterations(1000);
//!     for _ in 0..1000 {
//!         // Fast operation - measured once and divided by 1000
//!         std::hint::black_box(42 * 2);
//!     }
//! }
//!
//! // When `session` is dropped, the recorded statistics are printed to stdout
//! // and written to the Cargo target directory. No explicit call is required.
//! # }
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
//! together with bootstrap dispersion statistics over the operation's measured
//! spans: a through-origin slope point estimate, the standard deviation, a 95%
//! bootstrap confidence interval, and the observed minimum and maximum. The
//! bootstrap uses a fixed seed, so the confidence interval is reproducible across
//! runs of identical measurements. The stdout summary leads with the same
//! warmup-robust slope (see "Primary metric"), without the interval, for
//! readability.
//!
//! These outputs are produced automatically, so a typical benchmark only needs
//! to create a session and record work.
//!
//! # Thread vs process processor time
//!
//! You can choose between tracking thread processor time or process processor time:
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
//!     let _span = op.measure_thread();
//!     // Work done here is measured for the current thread only
//! }
//!
//! // Track process processor time (all threads)
//! {
//!     let op = session.operation("process_work");
//!     let _span = op.measure_process();
//!     // Work done here is measured for the entire process
//! }
//! # }
//! ```
//!
//! # Overhead
//!
//! Capturing a single measurement by calling `measure_xyz()` incurs an overhead of
//! approximately 500 nanoseconds on an arbitrary sample machine. You are recommended to measure
//! multiple consecutive iterations to minimize the impact of overhead on your benchmarks:
//! `operation.measure_xyz().iterations(12345)`.
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
//! let _span = operation.measure_thread();
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
pub use operation::*;
pub(crate) use operation_metrics::*;
pub use process_span::*;
pub use report::*;
pub use session::*;
pub use statistics::OperationStatistics;
pub(crate) use statistics::{SpanRecord, compute_slope_nanos, compute_statistics};
pub use thread_span::*;

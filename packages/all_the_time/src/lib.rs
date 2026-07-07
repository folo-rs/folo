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
//! # Benchmarking
//!
//! The typical pattern is to drive measurement from Criterion's [`iter_custom()`] function,
//! feeding its chosen iteration count into [`iterations()`](ThreadSpan::iterations)
//! so each recorded span covers a whole sample rather than a single iteration:
//!
//! ```no_run
//! use std::hint::black_box;
//! use std::time::Instant;
//!
//! use all_the_time::Session;
//! use criterion::Criterion;
//!
//! fn bench(c: &mut Criterion) {
//!     let session = Session::new();
//!
//!     let operation = session.operation("my_operation");
//!     c.bench_function("my_operation", |b| {
//!         b.iter_custom(|iters| {
//!             let start = Instant::now();
//!             let _span = operation.measure_thread().iterations(iters);
//!
//!             for _ in 0..iters {
//!                 black_box(42_u64.wrapping_mul(2));
//!             }
//!
//!             start.elapsed()
//!         });
//!     });
//!
//!     // When `session` is dropped, the recorded statistics are printed to
//!     // stdout and written to the Cargo target directory as JSON.
//! }
//! ```
//!
//! You **must** call [`iterations()`](ThreadSpan::iterations) on the span before
//! it is dropped. Failure to do so will result in a panic.
//!
//! # Human-readable summary
//!
//! When a [`Session`] is dropped it prints a table of per-iteration figures to
//! stdout, one row per operation:
//!
//! ```text
//! Processor time statistics:
//!
//! | Operation    | Per iteration |
//! |--------------|---------------|
//! | decode_value |          84ns |
//! | encode_value |         120ns |
//! ```
//!
//! # Machine-readable output
//!
//! Dropping a [`Session`] also writes JSON files (one per operation) into the
//! Cargo target directory at `target/all_the_time/<operation>.json`, with
//! operation names sanitized to be filesystem-safe.
//!
//! Both outputs are produced automatically, so a typical benchmark only needs to
//! create a session and record work.
//!
//! # Measuring a variable amount of work
//!
//! You do not need to specify the iteration count up front, as long as it is
//! provided before the span is dropped.
//!
//! This allows you to measure work whose extent is not known at the start.
//!
//! ```
//! use std::hint::black_box;
//!
//! use all_the_time::Session;
//!
//! fn main() {
//!     let session = Session::new();
//! #   let session = session.no_stdout().no_file();
//!     let operation = session.operation("drain_queue");
//!
//!     let span = operation.measure_thread();
//!
//!     let mut processed = 0_u64;
//!
//!     while let Some(item) = get_next_item() {
//!         black_box(item.refresh());
//!         processed += 1;
//!     }
//!
//!     drop(span.iterations(processed));
//! }
//! # fn get_next_item() -> Option<Item> { None }
//! # struct Item;
//! # impl Item { fn refresh(&self) {} }
//! ```
//!
//! # Thread vs process measurement
//!
//! You can choose between tracking processor time spent by the current thread
//! or by the entire process. The latter is useful for measuring multithreaded
//! workloads.
//!
//! ```
//! use all_the_time::Session;
//!
//! let session = Session::new();
//! # let session = session.no_stdout().no_file();
//!
//! // Track thread processor time
//! {
//!     let op = session.operation("thread_work");
//!     let _span = op.measure_thread().iterations(1);
//!     do_some_work();
//! }
//!
//! // Track process processor time (all threads)
//! {
//!     let op = session.operation("process_work");
//!     let _span = op.measure_process().iterations(1);
//!     do_some_multithreaded_work();
//! }
//! # fn do_some_work() {}
//! # fn do_some_multithreaded_work() {}
//! ```
//!
//! # Overhead
//!
//! Capturing a single measurement by calling `measure_xyz()` incurs an overhead of
//! approximately 500 nanoseconds on an arbitrary sample machine.
//!
//! It is crucial that you measure multiple iterations in the same sample to amortize this
//! overhead. This is the purpose of the [`iter_custom()`] pattern described above.
//!
//! Operating without batching, by measuring individual iterations, is only viable for
//! macrobenchmarks for which a single iteration is a large unit of work (e.g. an HTTP request).
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
//!
//!
//! [`iter_custom()`]: https://docs.rs/criterion/latest/criterion/struct.Bencher.html#method.iter_custom

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
pub use thread_span::*;

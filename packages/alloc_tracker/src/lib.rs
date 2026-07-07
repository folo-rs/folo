#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Memory allocation tracking utilities for benchmarks and performance analysis.
//!
//! This package provides utilities to track memory allocations during code execution,
//! enabling analysis of allocation patterns in benchmarks and performance tests.
//! The tracker reports both the number of bytes allocated and the count of allocations.
//!
//! The core functionality includes:
//! - [`Allocator`] - A Rust memory allocator wrapper that enables allocation tracking
//! - [`Session`] - Configures allocation tracking and provides access to tracking data
//! - [`Report`] - Thread-safe memory allocation statistics that can be merged and processed independently
//! - [`ProcessSpan`] - Tracks process-wide memory allocation changes over a time period
//! - [`ThreadSpan`] - Tracks thread-local memory allocation changes over a time period
//! - [`Operation`] - Measures per-iteration memory allocation of a repeated operation
//!
//! Additionally, when the `panic_on_next_alloc` feature is enabled:
#![cfg_attr(
    feature = "panic_on_next_alloc",
    doc = "- [`panic_on_next_alloc`] - Function to enable panic-on-next-allocation for debugging"
)]
#![cfg_attr(
    not(feature = "panic_on_next_alloc"),
    doc = "- `panic_on_next_alloc` - Function to enable panic-on-next-allocation for debugging"
)]
//!
//! This package is not meant for use in production, serving only as a development tool.
//!
//! # Primary metric
//!
//! The headline figure for both bytes and allocation count is the per-iteration
//! cost, reported as a **warmup-robust slope** rather than a plain mean. The first
//! iterations pay one-off costs — lazily initialized caches, a buffer growing to
//! its steady-state capacity — that a mean would fold into every iteration, making
//! it drift between runs; the slope isolates the marginal per-iteration cost so the
//! number stays stable. The JSON output pairs each slope with a 95% confidence
//! interval; the stdout summary shows the slopes alone. The plain means are
//! available through [`Operation::mean`] and the report accessors.
//!
//! # Features
#![cfg_attr(
    feature = "panic_on_next_alloc",
    doc = "- `panic_on_next_alloc`: Enables the [`panic_on_next_alloc`] function for debugging"
)]
#![cfg_attr(
    not(feature = "panic_on_next_alloc"),
    doc = "- `panic_on_next_alloc`: Enables the `panic_on_next_alloc` function for debugging"
)]
//!   unexpected allocations. This feature adds some overhead to allocations, so it is optional.
//!  
//! # Benchmarking
//!
//! The recommended pattern drives measurement from Criterion's `iter_custom`,
//! feeding its chosen iteration count into [`iterations`](ProcessSpan::iterations)
//! so each recorded span covers a whole sample rather than a single iteration:
//!
//! ```no_run
//! use std::hint::black_box;
//! use std::time::Instant;
//!
//! use alloc_tracker::{Allocator, Session};
//! use criterion::Criterion;
//!
//! #[global_allocator]
//! static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
//!
//! fn main() {
//!     let session = Session::new();
//!     let operation = session.operation("my_operation");
//!
//!     let mut criterion = Criterion::default();
//!     criterion.bench_function("my_operation", |b| {
//!         b.iter_custom(|iters| {
//!             let start = Instant::now();
//!             let _span = operation.measure_process().iterations(iters);
//!             for _ in 0..iters {
//!                 black_box(vec![1, 2, 3, 4, 5]); // This allocates memory
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
//! # Overhead
//!
//! Capturing a single measurement by calling `measure_xyz()` incurs an overhead of
//! approximately 50 nanoseconds on an arbitrary sample machine. While small, this can
//! still be considerable for tiny benchmarks. You are recommended to batch your
//! measurements over a whole Criterion sample (via `.iterations(iters)` from
//! `iter_custom`) to amortize this overhead. Operating without batching, on individual
//! iterations, is only viable for macrobenchmarks for which a single iteration is a
//! large unit of work (e.g. an HTTP request).
//!
//! # Machine-readable output
//!
//! Dropping a [`Session`] writes machine-readable JSON files (one per operation)
//! into the Cargo target directory at `target/alloc_tracker/<operation>.json`,
//! with operation names sanitized to be filesystem-safe. Each file records, for
//! both bytes and allocation count, the operation's total and per-iteration
//! figures. A human-readable summary is also printed to stdout.
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
//! use alloc_tracker::{Allocator, Session};
//!
//! #[global_allocator]
//! static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
//!
//! fn main() {
//!     let session = Session::new();
//! #   let session = session.no_stdout().no_file();
//!     let operation = session.operation("drain_queue");
//!
//!     let span = operation.measure_process();
//!     let mut processed = 0_u64;
//!     for item in 0..10 {
//!         let _data = format!("String number {item}"); // This allocates memory
//!         processed += 1;
//!     }
//!     drop(span.iterations(processed));
//!
//!     // Statistics are emitted automatically when `session` is dropped.
//! }
//! ```
//!
//! # Overhead
//!
//! In single-threaded scenarios, capturing a single measurement by calling
//! `Operation::measure_xyz()` incurs an overhead of approximately 10-15 nanoseconds
//! on an arbitrary sample machine. You are recommended to batch your measurements
//! over a whole Criterion sample (via `.iterations(iters)` from `iter_custom`) to
//! amortize this overhead.
//!
//! Memory allocator activity is likewise slightly impacted by the tracking logic.
//!
//! # Session management
//!
//! Multiple [`Session`] instances can be used concurrently as they track memory allocation
//! independently. Each session maintains its own set of operations and statistics.
//!
//! While [`Session`] itself is single-threaded, reports from sessions can be converted to
//! thread-safe [`Report`] instances and sent to other threads for processing:
//!
//! ```
//! use std::thread;
//!
//! use alloc_tracker::{Allocator, Session};
//!
//! #[global_allocator]
//! static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
//!
//! # fn main() {
//! let session = Session::new();
//! # let session = session.no_stdout().no_file();
//! {
//!     let operation = session.operation("work");
//!     let _span = operation.measure_process().iterations(1);
//!     let _data = vec![1, 2, 3]; // Some allocation work
//! }
//!
//! let report = session.to_report();
//!
//! // Reports are `Send`, so they can be moved to another thread for processing.
//! let total_bytes: u64 = thread::spawn(move || {
//!     report
//!         .operations()
//!         .map(|(_, op)| op.total_bytes_allocated())
//!         .sum()
//! })
//! .join()
//! .unwrap();
//!
//! println!("Captured {total_bytes} bytes across all operations");
//! # }
//! ```
//!
//! # Miri compatibility
//!
//! Miri replaces the global allocator with its own logic, so you cannot execute code that uses
//! this package under Miri.

mod allocator;
mod constants;
mod finalized;
mod operation;
mod operation_metrics;
mod process_span;
mod report;
mod session;
mod target_output;
mod thread_span;

pub use allocator::*;
pub(crate) use constants::*;
pub use finalized::*;
pub use operation::*;
pub(crate) use operation_metrics::*;
pub use process_span::*;
pub use report::*;
pub use session::*;
pub use thread_span::*;

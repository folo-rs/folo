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
//! Allocation counts are not deterministic across benchmark runs. Even when the
//! code under test allocates by a fixed rule, the first iterations pay one-off
//! costs (lazily initialized caches, growing a buffer to its steady-state
//! capacity) that later iterations do not, and Criterion decides how many
//! iterations to run and how many to discard as warm-up. A raw pooled **mean**
//! therefore folds those one-off allocations into the per-iteration figure and
//! drifts as that sample mix changes between runs.
//!
//! The headline metric for both bytes and allocation count is instead the
//! **warmup-robust slope**: an iterations-weighted, through-origin fit of each
//! span's total against its iteration count, which recovers the marginal
//! per-iteration cost even when the blend of warm-up and steady-state spans
//! shifts. Each figure is paired with a bootstrap 95% confidence interval so its
//! run-to-run uncertainty can be inspected (it collapses to a point when every
//! iteration allocates identically). The stdout summary leads with the slopes
//! alone for readability; the JSON output records the slopes together with their
//! confidence intervals, and the pooled means stay available through
//! [`Operation::mean`] and the report accessors for callers that still want them.
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
//! # Simple usage
//!
//! You can track allocations like this:
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
//!
//!     // Track a single operation
//!     {
//!         let operation = session.operation("my_operation");
//!         let _span = operation.measure_process();
//!         let _data = vec![1, 2, 3, 4, 5]; // This allocates memory
//!     }
//!
//!     // When `session` is dropped, the recorded statistics are printed to
//!     // stdout and written to the Cargo target directory as JSON.
//! }
//! ```
//!
//! # Machine-readable output
//!
//! Dropping a [`Session`] writes machine-readable JSON files (one per operation)
//! into the Cargo target directory at `target/alloc_tracker/<operation>.json`,
//! with operation names sanitized to be filesystem-safe. Each file records, for
//! both bytes and allocation count, the warmup-robust slope point estimate, a 95%
//! bootstrap confidence interval, the standard deviation, the observed minimum and
//! maximum, and the pooled mean. A human-readable summary leading with the same
//! slopes (see "Primary metric"), without the intervals, is also printed to
//! stdout.
//!
//! These outputs are produced automatically, so a typical benchmark only needs
//! to create a session and record work.
//!
//! # Tracking allocations per iteration
//!
//! For benchmarking scenarios, where you run multiple iterations of an operation, use [`Operation`]:
//!
//! ```
//! use alloc_tracker::{Allocator, Operation, Session};
//!
//! #[global_allocator]
//! static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
//!
//! fn main() {
//!     let session = Session::new();
//! #   let session = session.no_stdout().no_file();
//!
//!     // Record multiple iterations (batched for efficiency) so the
//!     // per-iteration slope can separate steady-state cost from warm-up.
//!     {
//!         let mut string_op = session.operation("string_allocations");
//!         let _span = string_op.measure_process().iterations(10);
//!         for i in 0..10 {
//!             let _data = format!("String number {}", i); // This allocates memory
//!         }
//!     }
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
//! over multiple benchmark iterations to amortize this overhead via `.iterations(N)`.
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
//!     let _span = operation.measure_process();
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
mod operation;
mod operation_metrics;
mod process_span;
mod report;
mod session;
mod target_output;
mod thread_span;

pub use allocator::*;
pub(crate) use constants::*;
pub use operation::*;
pub(crate) use operation_metrics::*;
pub use process_span::*;
pub use report::*;
pub use session::*;
pub use thread_span::*;

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
//! - [`Operation`] - Calculates mean memory allocation per operation
//!
//! Additionally, when the `panic_on_next_alloc` feature is enabled:
//! - [`panic_on_next_alloc`] - Function to enable panic-on-next-allocation for debugging
//!
//! This package is not meant for use in production, serving only as a development tool.
//!
//! # Features
//!
//! - `panic_on_next_alloc`: Enables the [`panic_on_next_alloc`] function for debugging
//!   unexpected allocations. This feature adds some overhead to allocations, so it's optional.
//!  
//! # Simple Usage
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
//!
//!     // Track a single operation
//!     {
//!         let operation = session.operation("my_operation");
//!         let _span = operation.measure_process();
//!         let _data = vec![1, 2, 3, 4, 5]; // This allocates memory
//!     }
//!
//!     // Print results
//!     session.print_to_stdout();
//!
//!     // Session automatically cleans up when dropped
//! }
//! ```
//!
//! # Tracking Mean Allocations
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
//!
//!     // Track mean over multiple operations (batched for efficiency)
//!     {
//!         let mut string_op = session.operation("string_allocations");
//!         let _span = string_op.measure_process().iterations(10);
//!         for i in 0..10 {
//!             let _data = format!("String number {}", i); // This allocates memory
//!         }
//!     }
//!
//!     // Output statistics of all operations to console
//!     session.print_to_stdout();
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
//! use alloc_tracker::{Allocator, Report, Session};
//!
//! #[global_allocator]
//! static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
//!
//! # fn main() {
//! let session = Session::new();
//! {
//!     let operation = session.operation("work");
//!     let _span = operation.measure_process();
//!     let _data = vec![1, 2, 3]; // Some allocation work
//! }
//!
//! let report = session.to_report();
//!
//! // Report can be sent to another thread
//! thread::spawn(move || {
//!     report.print_to_stdout();
//! })
//! .join()
//! .unwrap();
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
mod thread_span;

pub use allocator::*;
pub(crate) use constants::*;
pub use operation::*;
pub(crate) use operation_metrics::*;
pub use process_span::*;
pub use report::*;
pub use session::*;
pub use thread_span::*;

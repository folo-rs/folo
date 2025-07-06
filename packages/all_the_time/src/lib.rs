//! Processor time tracking utilities for benchmarks and performance analysis.
//!
//! This package provides utilities to track processor time during code execution,
//! enabling analysis of processor usage patterns in benchmarks and performance tests.
//!
//! The core functionality includes:
//! - [`Session`] - Configures processor time tracking and provides access to tracking data
//! - [`ThreadSpan`] - Tracks thread processor time over a time period
//! - [`ProcessSpan`] - Tracks process processor time over a time period
//! - [`Operation`] - Calculates mean processor time per operation
//! - [`SpanBuilder`] - Builder for creating spans with explicit iteration counts
//!
//! This package is not meant for use in production, serving only as a development tool.
//!
//! # Example
//!
//! ```rust
//! use all_the_time::Session;
//!
//! # fn main() {
//! let mut session = Session::new();
//!
//! {
//!     let batch_op = session.operation("batch_work");
//!     let _span = batch_op.iterations(1000).measure_thread();
//!     for _ in 0..1000 {
//!         // Fast operation - measured once and divided by 1000
//!         std::hint::black_box(42 * 2);
//!     }
//! }
//!
//! // Output statistics of all operations to console
//! session.print_to_stdout();
//! # }
//! ```
//!
//! # Thread vs Process Processor Time
//!
//! You can choose between tracking thread processor time or process processor time:
//!
//! ```
//! use all_the_time::Session;
//!
//! # fn main() {
//! let mut session = Session::new();
//!
//! // Track thread processor time
//! {
//!     let op = session.operation("thread_work");
//!     let _span = op.iterations(1).measure_thread();
//!     // Work done here is measured for the current thread only
//! }
//!
//! // Track process processor time (all threads)
//! {
//!     let op = session.operation("process_work");
//!     let _span = op.iterations(1).measure_process();
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
//! `operation.iterations(12345).measure_xyz()`.
//!
//! # Session management
//!
//! Multiple [`Session`] instances can be used concurrently as they track processor time
//! independently. Each session maintains its own set of operations and statistics.

mod operation;
mod pal;
mod process_span;
mod session;
mod span_builder;
mod thread_span;

pub use operation::Operation;
pub use process_span::ProcessSpan;
pub use session::Session;
pub use span_builder::SpanBuilder;
pub use thread_span::ThreadSpan;

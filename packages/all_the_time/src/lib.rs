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
//! # Simple Usage
//!
//! You can track processor time like this:
//!
//! ```
//! use std::num::NonZero;
//!
//! use all_the_time::Session;
//!
//! # fn main() {
//! let mut session = Session::new();
//!
//! // Track a single operation
//! {
//!     let operation = session.operation("my_operation");
//!     let _span = operation.iterations(1).measure_thread();
//!     // Perform some CPU-intensive work
//!     let mut sum = 0;
//!     for i in 0..10000 {
//!         sum += i;
//!     }
//! }
//!
//! // Print results
//! session.print_to_stdout();
//!
//! // Session automatically cleans up when dropped
//! # }
//! ```
//!
//! # Tracking Mean Processor Time
//!
//! For benchmarking scenarios, where you run multiple iterations of an operation, use batched measurements:
//!
//! ```
//! use std::num::NonZero;
//!
//! use all_the_time::Session;
//!
//! # fn main() {
//! let mut session = Session::new();
//!
//! // Track mean over multiple operations (single spans)
//! for i in 0..10 {
//!     let string_op = session.operation("cpu_intensive_work");
//!     let _span = string_op.iterations(1).measure_thread();
//!     // Perform some CPU-intensive work
//!     let mut sum = 0;
//!     for j in 0..i * 1000 {
//!         sum += j;
//!     }
//! }
//!
//! // Or track with batch measurements (multiple iterations in one span)
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
//! println!("{}", session);
//! # }
//! ```
//!
//! # Thread vs Process Processor Time
//!
//! You can choose between tracking thread processor time or process processor time:
//!
//! ```
//! use std::num::NonZero;
//!
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
//! # Threading
//!
//! The processor time tracking types are primarily intended for single-threaded use cases. Processor time
//! is tracked per thread by default. Single-threaded testing/benchmarking is recommended
//! to ensure meaningful data.
//!
//! # Session management
//!
//! Multiple [`Session`] instances can be used concurrently as they track processor time independently.
//! Each session maintains its own set of operations and statistics.

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

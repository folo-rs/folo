//! CPU time tracking utilities for benchmarks and performance analysis.
//!
//! This package provides utilities to track CPU time during code execution,
//! enabling analysis of CPU usage patterns in benchmarks and performance tests.
//!
//! The core functionality includes:
//! - [`Session`] - Configures CPU time tracking and provides access to tracking data
//! - [`ThreadSpan`] - Tracks thread CPU time over a time period
//! - [`ProcessSpan`] - Tracks process CPU time over a time period
//! - [`Operation`] - Calculates average CPU time per operation
//!
//! This package is not meant for use in production, serving only as a development tool.
//!  
//! # Simple Usage
//!
//! You can track CPU time like this:
//!
//! ```
//! use cpu_time_tracker::Session;
//!
//! # fn main() {
//! let mut session = Session::new();
//!
//! // Track a single operation
//! {
//!     let operation = session.operation("my_operation");
//!     let _span = operation.thread_span();
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
//! # Tracking Average CPU Time
//!
//! For benchmarking scenarios, where you run multiple iterations of an operation, use [`Operation`]:
//!
//! ```
//! use cpu_time_tracker::{Operation, Session};
//!
//! # fn main() {
//! let mut session = Session::new();
//!
//! // Track average over multiple operations
//! for i in 0..10 {
//!     let string_op = session.operation("cpu_intensive_work");
//!     let _span = string_op.thread_span();
//!     // Perform some CPU-intensive work
//!     let mut sum = 0;
//!     for j in 0..i * 1000 {
//!         sum += j;
//!     }
//! }
//!
//! // Output statistics of all operations to console
//! println!("{}", session);
//! # }
//! ```
//!
//! # Thread vs Process CPU Time
//!
//! You can choose between tracking thread CPU time or process CPU time:
//!
//! ```
//! use cpu_time_tracker::Session;
//!
//! # fn main() {
//! let mut session = Session::new();
//!
//! // Track thread CPU time
//! {
//!     let op = session.operation("thread_work");
//!     let _span = op.thread_span();
//!     // Work done here is measured for the current thread only
//! }
//!
//! // Track process CPU time (all threads)
//! {
//!     let op = session.operation("process_work");
//!     let _span = op.process_span();
//!     // Work done here is measured for the entire process
//! }
//! # }
//! ```
//!
//! # Threading
//!
//! The CPU time tracking types are primarily intended for single-threaded use cases. CPU time
//! is tracked per thread by default. Single-threaded testing/benchmarking is recommended
//! to ensure meaningful data.
//!
//! # Session management
//!
//! Multiple [`Session`] instances can be used concurrently as they track CPU time independently.
//! Each session maintains its own set of operations and statistics.

mod operation;
mod session;

pub use operation::*;
pub use session::*;

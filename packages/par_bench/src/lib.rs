//! Multi-threaded benchmark execution framework for performance testing.
//!
//! This package provides utilities to execute multi-threaded benchmarks with precise control
//! over thread groups, state management, and measurement timing. It is designed to integrate
//! with benchmarking frameworks like Criterion while handling the complexities of coordinated
//! multi-threaded execution.
//!
//! The core functionality includes:
//! - [`Run`] - Configurable multi-threaded benchmark execution with builder pattern API
//! - [`ThreadPool`] - Pre-warmed thread pool to eliminate thread creation overhead in benchmarks
//! - [`RunMeta`] - Metadata about the benchmark run, including group information and iteration counts
//! - [`RunSummary`] - Results from benchmark execution, including timing and measurement data
//!
//! This package is not meant for use in production, serving only as a development tool for
//! benchmarking and performance analysis.
//!
//! # Operating Principles
//!
//! ## Thread Groups
//!
//! Benchmarks can divide threads into equal-sized groups, allowing for scenarios where different
//! groups perform different roles (e.g., readers vs writers, producers vs consumers). Each thread
//! receives metadata about which group it belongs to and can behave differently based on this.
//!
//! ## State Management
//!
//! The framework supports multiple levels of state:
//! - **Thread State**: Created once per thread, shared across all iterations
//! - **Iteration State**: Created for each iteration, allowing per-iteration setup
//! - **Cleanup State**: Returned by iteration functions, dropped after measurement
//!
//! ## Measurement Timing
//!
//! Measurement wrappers allow precise control over what gets measured. The framework separates
//! preparation (unmeasured) from execution (measured) phases, ensuring benchmarks capture only
//! the intended work.
//!
//! # Basic Example
//!
//! ```
//! use std::sync::Arc;
//! use std::sync::atomic::{AtomicU64, Ordering};
//!
//! use many_cpus::ProcessorSet;
//! use par_bench::{Run, ThreadPool};
//!
//! # fn main() {
//! // Create a thread pool with default processor set
//! let mut pool = ThreadPool::new(&ProcessorSet::default());
//!
//! // Shared counter for all threads to increment
//! let counter = Arc::new(AtomicU64::new(0));
//!
//! let run = Run::new()
//!     .prepare_thread({
//!         let counter = Arc::clone(&counter);
//!         move |_| Arc::clone(&counter)
//!     })
//!     .prepare_iter(|args| Arc::clone(args.thread_state()))
//!     .iter(|mut args| {
//!         // This is the measured work
//!         args.iter_state().fetch_add(1, Ordering::Relaxed);
//!     });
//!
//! // Execute 1000 iterations across all threads
//! let results = run.execute_on(&mut pool, 1000);
//! println!("Mean duration: {:?}", results.mean_duration());
//! # }
//! ```
//!
//! # Multi-Group Example
//!
//! ```
//! use std::sync::Arc;
//! use std::sync::atomic::{AtomicU64, Ordering};
//!
//! use many_cpus::ProcessorSet;
//! use new_zealand::nz;
//! use par_bench::{Run, ThreadPool};
//!
//! # fn main() {
//! # if let Some(processors) = ProcessorSet::builder().take(nz!(4)) {
//! let mut pool = ThreadPool::new(&processors);
//!
//! let reader_count = Arc::new(AtomicU64::new(0));
//! let writer_count = Arc::new(AtomicU64::new(0));
//!
//! let run = Run::new()
//!     .groups(nz!(2)) // Divide 4 threads into 2 groups of 2 threads each
//!     .prepare_thread({
//!         let reader_count = Arc::clone(&reader_count);
//!         let writer_count = Arc::clone(&writer_count);
//!         move |args| {
//!             if args.meta().group_index() == 0 {
//!                 ("reader", Arc::clone(&reader_count))
//!             } else {
//!                 ("writer", Arc::clone(&writer_count))
//!             }
//!         }
//!     })
//!     .prepare_iter(|args| args.thread_state().clone())
//!     .iter(|mut args| {
//!         let (role, counter) = args.take_iter_state();
//!         match role {
//!             "reader" => {
//!                 // Reader work
//!                 counter.fetch_add(1, Ordering::Relaxed);
//!             }
//!             "writer" => {
//!                 // Writer work
//!                 counter.fetch_add(10, Ordering::Relaxed);
//!             }
//!             _ => unreachable!(),
//!         }
//!     });
//!
//! let results = run.execute_on(&mut pool, 100);
//! println!("Results: {:?}", results.mean_duration());
//! # }
//! # }
//! ```
//!
//! # Resource Usage Tracking
//!
//! When either the `alloc_tracker` or `all_the_time` features are enabled, the [`ResourceUsageExt`]
//! extension trait becomes available, providing convenient resource usage tracking for benchmarks:
//!
//! ```ignore
//! use alloc_tracker::{Allocator, Session as AllocSession};
//! use all_the_time::Session as TimeSession;
//! use par_bench::{ResourceUsageExt, Run, ThreadPool};
//!
//! #[global_allocator]
//! static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
//!
//! let allocs = AllocSession::new();
//! let processor_time = TimeSession::new();
//! let mut pool = ThreadPool::new(&ProcessorSet::default());
//!
//! let results = Run::new()
//!     .measure_resource_usage(|measure| {
//!         measure
//!             .allocs(&allocs, "my_operation")
//!             .processor_time(&processor_time, "my_operation")
//!     })
//!     .iter(|_| {
//!         let _data = vec![1, 2, 3, 4, 5]; // This allocates memory
//!         
//!         // Perform processor-intensive work
//!         let mut sum = 0;
//!         for i in 0..1000 {
//!             sum += i * i;
//!         }
//!         std::hint::black_box(sum);
//!     })
//!     .execute_on(&mut pool, 1000);
//!
//! // Access the combined resource usage data
//! for output in results.measure_outputs() {
//!     if let Some(alloc_report) = output.allocs() {
//!         println!("Allocation data available");
//!     }
//!     if let Some(time_report) = output.processor_time() {
//!         println!("Processor time data available");
//!     }
//! }
//! ```
//!
//! You can also use just one type of measurement:
//!
//! ```ignore
//! // Just allocation tracking
//! let results = Run::new()
//!     .measure_resource_usage(|measure| {
//!         measure.allocs(&allocs, "alloc_only")
//!     })
//!     .iter(|_| { /* work */ })
//!     .execute_on(&mut pool, 1000);
//!
//! // Just processor time tracking
//! let results = Run::new()
//!     .measure_resource_usage(|measure| {
//!         measure.processor_time(&processor_time, "time_only")
//!     })
//!     .iter(|_| { /* work */ })
//!     .execute_on(&mut pool, 1000);
//! ```

mod run;
mod run_configured;
mod run_configured_criterion;
mod run_meta;
mod threadpool;

// These are in a separate module because 99% of the time the user never needs to name
// these types, so it makes sense to de-emphasize them in the API documentation.
pub mod args;
pub mod configure;

#[cfg(any(feature = "alloc_tracker", feature = "all_the_time"))]
mod resource_usage_ext;

#[cfg(any(feature = "alloc_tracker", feature = "all_the_time"))]
pub use resource_usage_ext::*;
pub use run::*;
pub use run_configured::*;
pub use run_meta::*;
pub use threadpool::*;

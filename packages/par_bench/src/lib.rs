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
//! use par_bench::{Run, ThreadPool};
//!
//! # fn main() {
//! // Create a thread pool with default processor set
//! let pool = ThreadPool::default();
//!
//! // Shared counter for all threads to increment
//! let counter = Arc::new(AtomicU64::new(0));
//!
//! let run = Run::new()
//!     .prepare_thread_fn({
//!         let counter = Arc::clone(&counter);
//!         move |_run_meta| Arc::clone(&counter)
//!     })
//!     .prepare_iter_fn(|_run_meta, counter| Arc::clone(counter))
//!     .iter_fn(|counter: Arc<AtomicU64>| {
//!         // This is the measured work
//!         counter.fetch_add(1, Ordering::Relaxed);
//!     });
//!
//! // Execute 1000 iterations across all threads
//! let results = run.execute_on(&pool, 1000);
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
//! use new_zealand::nz;
//! use par_bench::{Run, ThreadPool};
//!
//! # fn main() {
//! # if let Some(processors) = many_cpus::ProcessorSet::builder().take(nz!(4)) {
//! let pool = ThreadPool::new(&processors);
//!
//! let reader_count = Arc::new(AtomicU64::new(0));
//! let writer_count = Arc::new(AtomicU64::new(0));
//!
//! let run = Run::new()
//!     .groups(nz!(2)) // Divide 4 threads into 2 groups of 2 threads each
//!     .prepare_thread_fn({
//!         let reader_count = Arc::clone(&reader_count);
//!         let writer_count = Arc::clone(&writer_count);
//!         move |run_meta| {
//!             if run_meta.group_index() == 0 {
//!                 ("reader", Arc::clone(&reader_count))
//!             } else {
//!                 ("writer", Arc::clone(&writer_count))
//!             }
//!         }
//!     })
//!     .prepare_iter_fn(|_run_meta, thread_state| thread_state.clone())
//!     .iter_fn(|(role, counter): (&str, Arc<AtomicU64>)| {
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
//! let results = run.execute_on(&pool, 100);
//! println!("Results: {:?}", results.mean_duration());
//! # }
//! # }
//! ```

mod run;
mod run_configured;
mod run_configured_criterion;
mod run_meta;
mod threadpool;

// This is in a separate module because 99% of the time the user never needs to name
// these types, so it makes sense to de-emphasize them in the API documentation.
pub mod configure;

pub use run::*;
pub use run_configured::*;
pub use run_meta::*;
pub use threadpool::*;

//! Memory allocation tracking utilities for benchmarks and performance analysis.
//!
//! This crate provides utilities to track memory allocations during code execution,
//! enabling measurement of memory usage patterns in benchmarks and performance tests.
//!
//! The core functionality includes:
//! - [`Allocator`] - A Rust memory allocator wrapper that enables allocation tracking
//! - [`Session`] - Configures allocation tracking and provides access to tracking data
//! - [`Span`] - Tracks memory allocation changes over a time period
//! - [`Operation`] - Calculates average memory allocation per operation
//! - [`OperationSet`] - Collects and displays memory usage measurements
//!
//! # Quick Start
//!
//! Add this to your `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! alloc_tracker = "0.1.0"
//! ```
//!
//! # Simple Usage
//!
//! With the new simplified API, you can track allocations like this:
//!
//! ```
//! use std::alloc::System;
//!
//! use alloc_tracker::{Allocator, Session, Span};
//!
//! #[global_allocator]
//! static ALLOCATOR: Allocator<System> = Allocator::system();
//!
//! fn main() {
//!     let session = Session::new();
//!
//!     // Track a single operation
//!     let tracker = Span::new(&session);
//!     let data = vec![1, 2, 3, 4, 5]; // This allocates memory
//!     let delta = tracker.to_delta();
//!     println!("Allocated {} bytes", delta);
//!
//!     // Session automatically cleans up when dropped
//! }
//! ```
//!
//! # Tracking Average Allocations
//!
//! For benchmarking scenarios, use [`Operation`]:
//!
//! ```
//! use std::alloc::System;
//!
//! use alloc_tracker::{Allocator, Operation, Session};
//!
//! #[global_allocator]
//! static ALLOCATOR: Allocator<System> = Allocator::system();
//!
//! fn main() {
//!     let session = Session::new();
//!     let mut average = Operation::new("string_allocations".to_string());
//!
//!     // Track average over multiple operations
//!     for i in 0..10 {
//!         let _contributor = average.span(&session);
//!         let _data = format!("String number {}", i); // This allocates memory
//!     }
//!
//!     println!(
//!         "Average allocation: {} bytes per operation",
//!         average.average()
//!     );
//!     println!("Operation name: {}", average.name());
//! }
//! ```
//!
//! ## Global allocator setup
//!
//! For real allocation tracking, you need to set up [`Allocator`] as your global allocator.
//! This requires a binary crate (application), not a library crate:
//!
//! ```rust
//! use std::alloc::System;
//!
//! use alloc_tracker::{Allocator, Session, Span};
//!
//! #[global_allocator]
//! static ALLOCATOR: Allocator<System> = Allocator::system();
//!
//! fn main() {
//!     // Create a tracking session (automatically handles setup)
//!     let session = Session::new();
//!
//!     // Track a single operation
//!     let tracker = Span::new(&session);
//!     let data = vec![1, 2, 3, 4, 5]; // This allocates memory
//!     let bytes_allocated = tracker.to_delta();
//!
//!     println!("Allocated {} bytes", bytes_allocated);
//!
//!     // Session automatically disables tracking when dropped
//! }
//! ```
//!
//! ## Collecting multiple measurements
//!
//! Use [`OperationSet`] to collect and display measurements from different operations:
//!
//! ```
//! use std::alloc::System;
//!
//! use alloc_tracker::{Allocator, Operation, OperationSet, Session};
//!
//! #[global_allocator]
//! static ALLOCATOR: Allocator<System> = Allocator::system();
//!
//! fn main() {
//!     let session = Session::new();
//!     let mut results = OperationSet::new();
//!
//!     // Create and measure different operations
//!     let mut vec_measurement = Operation::new("vector_creation".to_string());
//!     {
//!         let _contributor = vec_measurement.span(&session);
//!         let _vec = vec![1, 2, 3, 4, 5]; // This allocates memory
//!     }
//!     results.add(vec_measurement);
//!
//!     let mut string_measurement = Operation::new("string_creation".to_string());
//!     {
//!         let _contributor = string_measurement.span(&session);
//!         let _string = String::from("Hello, world!"); // This allocates memory  
//!     }
//!     results.add(string_measurement);
//!
//!     // Print all results
//!     println!("{}", results);
//!
//!     // Access individual results
//!     if let Some(bytes) = results.get("vector_creation") {
//!         println!("Vector creation used {} bytes", bytes);
//!     }
//! }
//! ```
//!
//! ## Use in Criterion benchmarks
//!
//! This crate integrates well with Criterion for memory-aware benchmarking:
//!
//! ```rust
//! use std::alloc::System;
//!
//! use alloc_tracker::{Allocator, Operation, Session};
//! use criterion::{Criterion, criterion_group, criterion_main};
//!
//! #[global_allocator]
//! static ALLOCATOR: Allocator<System> = Allocator::system();
//!
//! fn bench_string_allocation(c: &mut Criterion) {
//!     let session = Session::new();
//!
//!     let mut group = c.benchmark_group("memory_usage");
//!
//!     group.bench_function("string_formatting", |b| {
//!         let mut average_memory_delta = Operation::new("string_formatting".to_string());
//!
//!         b.iter(|| {
//!             let _contributor = average_memory_delta.span(&session);
//!             let s = format!("Hello, {}!", "world");
//!             std::hint::black_box(s);
//!         });
//!
//!         println!(
//!             "Average allocation: {} bytes",
//!             average_memory_delta.average()
//!         );
//!     });
//!
//!     group.finish();
//! }
//!
//! criterion_group!(benches, bench_string_allocation);
//! criterion_main!(benches);
//! ```
//!
//! # Thread Safety
//!
//! The allocation tracking is thread-safe using atomic operations. Measurements are global
//! and account for all allocations on all threads, so single-threaded testing is recommended
//! for precise measurements.
//!
//! # Session Management
//!
//! Only one [`Session`] can be active at a time. Attempting to create
//! multiple sessions simultaneously will result in an error. This ensures that tracking
//! state is properly managed and measurements are accurate.

mod allocator;
mod operation;
mod session;
mod span;
mod tracker;

pub use allocator::*;
pub use operation::*;
pub use session::*;
pub use span::*;

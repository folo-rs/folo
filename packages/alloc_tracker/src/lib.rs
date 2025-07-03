//! Memory allocation tracking utilities for benchmarks and performance analysis.
//!
//! This package provides utilities to track memory allocations during code execution,
//! enabling analysis of allocation patterns in benchmarks and performance tests.
//!
//! The core functionality includes:
//! - [`Allocator`] - A Rust memory allocator wrapper that enables allocation tracking
//! - [`Session`] - Configures allocation tracking and provides access to tracking data
//! - [`Span`] - Tracks memory allocation changes over a time period
//! - [`Operation`] - Calculates average memory allocation per operation
//!
//! This package is not meant for use in production, serving only as a development tool.
//!  
//! # Simple Usage
//!
//! You can track allocations like this:
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
//!     {
//!         let span = Span::new(&session);
//!         let data = vec![1, 2, 3, 4, 5]; // This allocates memory
//!         let delta = span.to_delta();
//!         println!("Allocated {delta} bytes");
//!     }
//!
//!     // Session automatically cleans up when dropped
//! }
//! ```
//!
//! # Tracking Average Allocations
//!
//! For benchmarking scenarios, where you run multiple iterations of an operation, use [`Operation`]:
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
//!     let mut session = Session::new();
//!
//!     // Track average over multiple operations
//!     for i in 0..10 {
//!         let string_op = session.operation("string_allocations");
//!         let _span = string_op.span();
//!         let _data = format!("String number {}", i); // This allocates memory
//!     }
//!
//!     // Output statistics of all operations to console
//!     println!("{}", session);
//! }
//! ```
//!
//! # Threading
//!
//! The allocation tracking types are primarily intended for single-threaded use cases. However,
//! memory allocations are tracked globally. Single-threaded testing/benchmarking is recommended
//! to ensure meaningful data.
//!
//! # Session management
//!
//! Only one [`Session`] can be active at a time. Attempting to create
//! multiple sessions simultaneously will result in an error. This ensures that tracking
//! state is properly managed and statistics are accurate.
//! 
//! # Miri compatibility
//! 
//! Miri replaces the global allocator with its own logic, so you cannot execute code that uses
//! this crate under Miri.

mod allocator;
mod operation;
mod session;
mod span;
mod tracker;

pub use allocator::*;
pub use operation::*;
pub use session::*;
pub use span::*;

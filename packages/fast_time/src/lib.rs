#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! Provides efficient mechanisms to capture the current timestamp and measure elapsed time.
//!
//! This crate offers a [`Clock`] that can provide timestamps with low overhead, making it
//! suitable for high-frequency querying scenarios like metrics collection and logging.
//! The clock prioritizes efficiency over absolute precision, making it ideal for applications
//! that need to capture thousands of timestamps per second.
//!
//! # Key Features
//!
//! - **Low overhead**: Optimized for rapid, repeated timestamp capture on supported platforms
//! - **Monotonic timestamps**: Guarantees that timestamps always increase
//! - **Cross-platform**: Works on all platforms, with optimizations for Windows and Linux
//! - **Standard library compatibility**: Converts to/from [`std::time::Instant`]
//!
//! # Trade-offs
//!
//! On optimized platforms (Windows, Linux):
//!
//! - May lag behind wall-clock time by a few milliseconds
//! - May not reflect explicit wall clock adjustments (e.g., NTP synchronization)
//! - Optimized for frequency over precision
//!
//! On other platforms, performance is equivalent to `std::time::Instant`.
//!
//! # Hot loop performance comparison (optimized platforms)
//!
//! | Platform                 | `fast_time` | `std::time` |
//! |--------------------------|-------------|-------------|
//! | Windows                  |        2 ns |       25 ns |
//! | Linux                    |        6 ns |       19 ns |
//!
//! # Basic Usage
//!
//! ```rust
//! use fast_time::Clock;
//!
//! let mut clock = Clock::new();
//! let start = clock.now();
//!
//! // Do some work...
//! std::thread::sleep(std::time::Duration::from_millis(10));
//!
//! let elapsed = start.elapsed(&mut clock);
//! println!("Operation took: {:?}", elapsed);
//! ```
//!
//! # High-frequency timestamp collection
//!
//! ```rust
//! use std::time::Duration;
//!
//! use fast_time::Clock;
//!
//! let mut clock = Clock::new();
//! let mut timestamps = Vec::new();
//!
//! // Collect many timestamps rapidly
//! for _ in 0..1000 {
//!     timestamps.push(clock.now());
//! }
//!
//! // Measure elapsed time between first and last
//! let total_time = timestamps
//!     .last()
//!     .unwrap()
//!     .saturating_duration_since(*timestamps.first().unwrap());
//!
//! println!(
//!     "Collected {} timestamps in {:?}",
//!     timestamps.len(),
//!     total_time
//! );
//! ```
//!
//! # Supported platforms
//!
//! This crate provides optimized implementations for:
//!
//! * Windows
//! * Linux
//!
//! On other platforms, the crate functions as a lightweight wrapper around the
//! Rust standard library's `Instant` type, ensuring the crate compiles on all
//! platforms without requiring platform-specific optimizations.

mod pal;

mod clock;
mod instant;

pub use clock::*;
pub use instant::*;

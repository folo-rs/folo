//! Provides efficient mechanisms to capture the current timestamp and measure elapsed time.
//!
//! This crate offers a [`Clock`] that can provide timestamps with low overhead, making it
//! suitable for high-frequency querying scenarios like metrics collection and logging.
//! The clock prioritizes efficiency over absolute precision, making it ideal for applications
//! that need to capture thousands of timestamps per second.
//!
//! # Key Features
//!
//! - **Low overhead**: Optimized for rapid, repeated timestamp capture
//! - **Monotonic timestamps**: Guarantees that timestamps always increase
//! - **Cross-platform**: Works on both Windows and Linux
//! - **Standard library compatibility**: Converts to/from [`std::time::Instant`]
//!
//! # Trade-offs
//!
//! - May lag behind wall-clock time by a few milliseconds
//! - May not reflect explicit wall clock adjustments (e.g., NTP synchronization)
//! - Optimized for frequency over precision
//!
//! # Hot loop performance comparison
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
//! let clock = Clock::new();
//! let start = clock.now();
//!
//! // Do some work...
//! std::thread::sleep(std::time::Duration::from_millis(10));
//!
//! let elapsed = start.elapsed(&clock);
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
//! let clock = Clock::new();
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
//! This crate is tested on:
//! 
//! * Windows 11
//! * Ubuntu 24

mod pal;

mod clock;
mod instant;

pub use clock::*;
pub use instant::*;

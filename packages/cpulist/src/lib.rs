#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! Utilities for parsing and emitting strings in the the `cpulist` format often used by Linux
//! utilities that work with processor IDs, memory region IDs and similar numeric hardware
//! identifiers.
//!
//! Example cpulist string: `0-9,32-35,40`
//!
//! This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
//! high-performance hardware-aware programming in Rust.
//!
//! # Format
//!
//! The value is a comma-separated list of zero or more integers or integer ranges, where each item
//! is either:
//!
//! * a single integer (e.g. `1`)
//! * a range of integers (e.g. `2-4`)
//! * a range of integers with a stride (step size) operator (e.g. `5-9:2` which is equivalent to `5,7,9`)
//!
//! Whitespace or extra characters are not allowed anywhere in the string.
//!
//! The identifiers in the list are of size `u32`.
//!
//! # Example
//!
//! Basic conversion from/to strings:
//!
//! ```
//! let selected_processors = cpulist::parse("0-9,32-35,40").unwrap();
//! assert_eq!(
//!     selected_processors,
//!     vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 32, 33, 34, 35, 40]
//! );
//!
//! println!("Selected processors: {selected_processors:?}");
//! println!("As cpulist: {}", cpulist::emit(selected_processors));
//! ```
//!
//! The stride operator is also supported for parsing:
//! ```
//! let evens = cpulist::parse("0-16:2").unwrap();
//! let odds = cpulist::parse("1-16:2").unwrap();
//!
//! let all = cpulist::emit(odds.iter().chain(evens.iter()).copied());
//!
//! println!("Evens: {evens:?}");
//! println!("Odds: {odds:?}");
//!
//! println!("All as cpulist: {all}");
//! ```

mod emit;
mod error;
mod parse;

pub use emit::*;
pub use error::*;
pub use parse::*;

pub(crate) type Item = u32;

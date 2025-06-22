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
#![doc = source_file!("examples/cpulist_basic.rs")]
//! ```
//!
//! The stride operator is also supported for parsing:
//! ```
#![doc = source_file!("examples/cpulist_stride.rs")]
//! ```

use include_doc::source_file;

mod emit;
mod error;
mod parse;

pub use emit::*;
pub use error::*;
pub use parse::*;

pub(crate) type Item = u32;

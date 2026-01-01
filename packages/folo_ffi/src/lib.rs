#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! Utilities for working with FFI logic; exists for internal use in Folo packages; no stable API surface.

mod native_buffer;

pub use native_buffer::*;

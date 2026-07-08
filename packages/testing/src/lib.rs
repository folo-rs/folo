#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(coverage_nightly, coverage(off))] // This is all test code, no need to test it.

//! Private helpers for testing and examples in Folo packages.

mod assert_panics;
mod cwd_guard;
mod float;
mod reentrant_waker;
mod watchdog;

pub use assert_panics::{assert_panics, assert_panics_with};
pub use cwd_guard::CwdGuard;
pub use float::f64_diff_abs;
pub use reentrant_waker::ReentrantWakerData;
pub use watchdog::with_watchdog;

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use windows::*;

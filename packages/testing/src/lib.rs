#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(coverage_nightly, coverage(off))] // This is all test code, no need to test it.

//! Private helpers for testing and examples in Folo packages.

mod assert_panics;
mod clone_waker;
mod cwd_guard;
mod drop_waker;
mod float;
mod reentrant_waker;
mod wake_action_waker;
mod watchdog;

pub use assert_panics::{assert_panics, assert_panics_with};
pub use clone_waker::{clone_action_waker, clone_action_waker_panicking_on_clone_release};
pub use cwd_guard::CwdGuard;
pub use drop_waker::{DropOnWakerRelease, PanicsOnDrop, drop_waker};
pub use float::f64_diff_abs;
pub use reentrant_waker::ReentrantWakerData;
pub use wake_action_waker::wake_action_waker;
pub use watchdog::with_watchdog;

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use windows::*;

//! Filesystem session storage and its Win32 record-file mechanics.

mod policy;
mod store;
#[cfg_attr(coverage_nightly, coverage(off))]
mod windows;

pub(crate) use store::*;
#[cfg(any(test, feature = "private-test-util"))]
pub(crate) use windows::RecordFile;

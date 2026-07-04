#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Utilities for internal use in Folo packages; no stable API surface.

mod file_name;
mod span_stats;
mod target_dir;

pub use file_name::*;
pub use span_stats::*;
pub use target_dir::*;

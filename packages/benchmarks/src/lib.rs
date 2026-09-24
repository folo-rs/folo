#![cfg_attr(docsrs, feature(doc_cfg))]

//! Manual exploratory benchmarks and their workload helpers.

mod memory_workload;

pub use memory_workload::*;

#[cfg(test)]
::testing::set_allocator!();

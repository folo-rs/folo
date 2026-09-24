//! Replays benchmark collection across an inclusive historical commit range.

mod execution;

pub(crate) use execution::*;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;

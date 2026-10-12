//! Bounded frame accumulation and its generated-input checks.

mod reader;

pub(crate) use reader::*;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;

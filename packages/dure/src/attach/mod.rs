//! Client attach and console relay.

mod relay;

pub(crate) use relay::*;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;

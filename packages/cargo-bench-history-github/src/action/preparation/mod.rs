//! Offline configuration, commit, package-scope and historical-range workflow preparation.

mod args;
mod execute;
mod inputs;
mod scope;

pub(crate) use args::*;
pub(crate) use execute::*;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;

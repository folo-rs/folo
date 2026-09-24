//! Offline configuration, commit, package-scope and historical-range workflow preparation.

mod args;
mod backfill;
mod execute;
mod inputs;
mod scope;

pub(crate) use args::*;
#[cfg(any(test, feature = "private-test-util"))]
pub use backfill::prepare_backfill_at;
pub(crate) use execute::*;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;

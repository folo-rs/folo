//! Offline configuration, commit, package-scope and historical-range workflow preparation.

mod args;
mod backfill;
mod execute;
mod inputs;
mod scope;

pub(crate) use args::*;
pub use backfill::prepare_backfill_at;
pub(crate) use execute::*;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;

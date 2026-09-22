//! Post-install action validation and planning over separately supplied native effects.

mod args;
mod artifact_path;
mod environment;
mod errors;
mod execute;
mod flags;
mod inputs;
mod native;
mod plan;
mod port;
mod preparation;
mod publication;

pub(crate) use args::*;
pub(crate) use execute::run;
#[cfg(any(test, feature = "private-test-util"))]
pub use preparation::prepare_backfill_at;
pub(crate) use preparation::{PrepareWorkflowArgs, prepare_workflow};

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;

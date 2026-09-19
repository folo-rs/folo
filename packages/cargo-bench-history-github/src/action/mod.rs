//! Post-install action validation and planning over separately supplied native effects.

mod args;
mod artifact_path;
mod environment;
mod errors;
mod execute;
mod inputs;
mod native;
mod plan;
mod port;
mod publication;

pub(crate) use args::*;
pub(crate) use execute::run;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;

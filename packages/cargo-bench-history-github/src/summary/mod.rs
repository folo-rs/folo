//! Offline workflow cards; benchmark verdicts remain owned by checked report evidence.

mod input;
mod render;
mod run;

pub(crate) use run::*;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;

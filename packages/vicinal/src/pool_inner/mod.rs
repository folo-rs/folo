mod admission;
mod inner;
mod worker_loop;

pub(crate) use inner::*;
pub(crate) use worker_loop::*;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;

#[cfg(test)]
pub(crate) use tests::helpers::{OnDrop, PanickingPayload};

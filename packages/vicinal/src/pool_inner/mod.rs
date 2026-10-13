mod inner;

pub(crate) use inner::*;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;

#[cfg(test)]
pub(crate) use tests::helpers::{OnDrop, PanickingPayload};

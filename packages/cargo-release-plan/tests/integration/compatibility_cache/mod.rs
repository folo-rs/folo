//! Cross-invocation decision reuse must not extend compatibility source admission.

#![cfg_attr(coverage_nightly, coverage(off))]

mod admission;
mod checker;
mod harness;
mod modes;
mod provenance;
mod recovery;

pub(crate) use harness::*;

//! Captured compatibility admission, unchanged intervals and independent checker outcomes.

#![cfg_attr(coverage_nightly, coverage(off))]

mod admission;
mod checker;
mod evidence;
mod harness;
mod provenance;

pub(crate) use harness::*;

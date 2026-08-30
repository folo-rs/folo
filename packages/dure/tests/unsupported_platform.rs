//! Stands in for the test suite on platforms `dure` does not support.
//!
//! The crate is gated to Windows (`docs/implementation.md`, "Platform gate"), so
//! everywhere else this package compiles away to nothing and offers no behavior
//! to exercise. A test run narrowed to this package alone would then find no
//! tests at all, which the runner reports as a failure. Carrying one test here
//! keeps that admission local to the package it concerns, rather than making
//! every package in the workspace tolerate having no tests.

#![cfg(not(windows))]

#[test]
fn there_is_nothing_to_supervise_here() {}

#![cfg_attr(docsrs, feature(doc_cfg))]

//! UI tests for compile-time error checking.
//!
//! This package uses the `trybuild` test harness to verify that certain code patterns
//! correctly result in compilation errors, particularly for lifetime and soundness requirements.
//!
//! The package consists only of a single integration test.

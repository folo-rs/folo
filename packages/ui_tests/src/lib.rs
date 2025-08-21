//! UI tests for compile-time error checking.
//!
//! This package uses the `trybuild` test harness to verify that certain code patterns
//! correctly result in compilation errors, particularly for lifetime and soundness requirements.
//!
//! # Important limitations
//!
//! This package contains only a single test function to prevent parallel test execution.
//! `trybuild` has a built-in limitation that prevents it from supporting parallel test
//! execution safely. To ensure test reliability, all UI tests are consolidated into a
//! single test function that runs sequentially.
//!
//! When adding new UI tests:
//! - Add test files to the appropriate subdirectory under `tests/ui/`
//! - Use the folder structure: `tests/ui/{package}/{compile_fail|pass}/`
//! - The test discovery uses wildcards, so new files will be picked up automatically
//! - Do NOT add additional `#[test]` functions to this package

#![doc(html_root_url = "https://docs.rs/ui_tests/0.1.0/")]
#![warn(missing_docs)]

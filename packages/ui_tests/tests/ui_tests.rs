#![cfg(not(miri))] // Miri and trybuild do not go together.

//! UI tests for compile-time error checking.
//!
//! This module contains a single test function that runs all UI tests sequentially.
//! This is required because `trybuild` does not support parallel test execution.
//!
//! # Adding new tests
//!
//! To add new UI tests:
//! 1. Create test files in the appropriate subdirectory under `tests/ui/`
//! 2. Use the folder structure: `tests/ui/{package}/{compile_fail|pass}/`
//! 3. Files will be automatically discovered by the wildcard patterns below
//! 4. Do NOT add additional `#[test]` functions - keep everything in the single `ui()` function

#[test]
fn ui() {
    let t = trybuild::TestCases::new();

    // Auto-discover all compile_fail tests using wildcards
    // This pattern will pick up any .rs file in any package's compile_fail directory
    t.compile_fail("tests/ui/*/compile_fail/*.rs");

    // Auto-discover all pass tests using wildcards
    // This pattern will pick up any .rs file in any package's pass directory
    t.pass("tests/ui/*/pass/*.rs");
}

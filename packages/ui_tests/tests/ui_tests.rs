//! UI tests for compile-time error checking.
//!
//! This module contains a single test function that runs all UI tests sequentially.
//! This is required because `trybuild` does not support parallel test execution.
//!
//! # Only failing tests
//!
//! The only purpose to have UI tests is to verify that compilation fails.
//! Do not add tests for successful cases - use regular unit/integration tests for that.
//!
//! # Adding new tests
//!
//! To add new UI tests:
//! 1. Create test files in the appropriate subdirectory under `tests/ui/`
//! 2. Use the folder structure: `tests/ui/{package}/`
//! 3. Files will be automatically discovered by the wildcard patterns below
//! 4. Do NOT add additional `#[test]` functions - keep everything in the single `ui()` function

#[test]
#[cfg_attr(miri, ignore)] // Miri and trybuild do not go together.
#[cfg_attr(careful, ignore)] // Careful is nightly build, may have different expected output.
fn ui() {
    let t = trybuild::TestCases::new();

    // Auto-discover all tests using wildcards
    // This pattern will pick up any .rs file in any package's directory
    t.compile_fail("tests/ui/*/*.rs");
}

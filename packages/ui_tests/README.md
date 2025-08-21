# UI Tests

This package provides compile-time error checking using the `trybuild` test harness to verify that certain code patterns correctly result in compilation errors, particularly for lifetime and soundness requirements.

## Important limitations

This package contains only a single test function to prevent parallel test execution. `trybuild` has a built-in limitation that prevents it from supporting parallel test execution safely. To ensure test reliability, all UI tests are consolidated into a single test function that runs sequentially.

## Folder structure

Test files are organized in the following structure:

```
tests/ui/{package}/{compile_fail|pass}/
```

Where:
- `{package}` is the name of the package being tested (e.g., `blind_pool`)
- `compile_fail` contains tests that should fail to compile
- `pass` contains tests that should compile successfully

## Adding new tests

To add new UI tests:

1. Create test files in the appropriate subdirectory under `tests/ui/`
2. Use the folder structure described above
3. Files will be automatically discovered by the wildcard patterns in the test
4. **Do NOT add additional `#[test]` functions** - keep everything in the single `ui()` function

## Current packages tested

- `blind_pool`: Tests for lifetime requirements (`T: 'static`) in pool insertion methods

## Example test file names

- `tests/ui/blind_pool/compile_fail/blind_pool_non_static_fail.rs`
- `tests/ui/blind_pool/pass/blind_pool_static_pass.rs`
- `tests/ui/some_package/compile_fail/some_test_fail.rs`
- `tests/ui/some_package/pass/some_test_pass.rs`

## Running the Tests

```bash
just package=ui_tests test
```

These tests are part of the overall validation process and help maintain the soundness guarantees of the workspace packages.
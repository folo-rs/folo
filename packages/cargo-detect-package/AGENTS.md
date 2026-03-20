# Agent notes for cargo-detect-package

## When `#[serial]` is required on tests

Tests in `workspace::tests` that depend on the process-global current working directory must be
marked `#[serial]` from the `serial_test` crate. This includes:

* Tests that explicitly call `std::env::set_current_dir` (obvious).
* Tests that **implicitly** depend on the current directory being inside a Cargo workspace. For
  example, `validate_workspace_context_nonexistent_file` does not change the directory itself but
  calls `validate_workspace_context`, which internally calls `current_dir()` and then
  `find_workspace_root()`. If a concurrent test has changed the working directory to a location
  outside any workspace, `find_workspace_root` fails with a different error than expected, causing
  the test to flake.

The rule is simple: if a test calls any function that reads or writes the process current directory,
it must be `#[serial]`.

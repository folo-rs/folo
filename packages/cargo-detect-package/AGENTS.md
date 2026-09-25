# Agent notes for cargo-detect-package

## When `#[serial]` is required on tests

Integration tests that depend on the process-global current working directory must be
marked `#[serial]` from the `serial_test` crate. This includes:

* Tests that explicitly call `std::env::set_current_dir` (obvious).
* Tests that **implicitly** depend on the current directory being inside a Cargo workspace.
  Calling `run()` reaches workspace validation, which reads the current directory. If another test
  changes that directory, validation may select a different workspace or report a different error.

The rule is simple: if an integration test calls any function that reads or writes the process
current directory, it must be `#[serial]`. Unit tests inject `MockFilesystem` instead of accessing
the real filesystem or process current directory.

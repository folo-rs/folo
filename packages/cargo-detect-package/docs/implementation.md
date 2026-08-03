# cargo-detect-package - Implementation

The implementation separates command-line parsing, workspace discovery, package detection,
manifest parsing, and child-process execution. The `run` entry point coordinates these stages and
does not start the child process until the current directory and target path resolve to the same
workspace.

Filesystem access and process execution are represented by small internal abstractions so unit
tests can exercise discovery and mapping without depending on ambient state. Target
canonicalization retains the attempted path and the operating-system error rather than assuming
that every canonicalization failure means the path is absent.

Every application-owned failure condition is private and converts into `ohno::AppError` at the
public `run` boundary. Same-crate unit tests verify exact condition and source mappings;
integration tests assert command behavior, diagnostics, side effects, and the `AppError`
boundary without publishing the private taxonomy.

# Release-plan implementation partition

`crp_impl` owns repository and workspace acquisition, release classification,
artifact planning and application for
[`cargo-release-plan`](../../cargo-release-plan/docs/design.md).
The application's [implementation guide](../../cargo-release-plan/docs/implementation.md)
describes the shared architecture.

The public application re-exports only its supported API and owns the executable.
This partition exposes the internal operations needed by maintainer integration
tests without extending that application API. Both crates are released in
lockstep through the application's exact dependency.

Unit tests retain in-process parsing, decisions and orchestration. Real Git,
Cargo and filesystem assertions run in this package's integration targets.
The shell's executable-connected end-to-end suite stays with its binary so Cargo
provides the actual executable path; no nested Cargo build or duplicate binary
is needed by the harness.

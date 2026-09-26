# Release-plan implementation partition

`crp_impl` owns repository and workspace acquisition, release classification,
artifact planning and application for
[`cargo-release-plan`](../../cargo-release-plan/docs/design.md).
The application's [implementation guide](../../cargo-release-plan/docs/implementation.md)
describes the shared architecture.

The application package owns the executable and re-exports only the Rust items
needed by that executable and maintainer tests. This partition exposes the other
internal operations needed by its integration tests. Neither library target
defines a supported Rust API. Both crates are released in lockstep through the
application's exact dependency.

Unit tests retain in-process parsing, decisions and orchestration. Real Git,
Cargo and filesystem assertions run in this package's integration targets.
The shell's executable-connected end-to-end suite stays with its binary so Cargo
provides the actual executable path; no nested Cargo build or duplicate binary
is needed by the harness.

The explicitly packaged algorithm benchmark selects mimalloc directly through
a registry-backed development dependency. Other development targets select their
allocator through the unpublished workspace testing helper; ordinary library
builds leave allocator selection to the consumer.

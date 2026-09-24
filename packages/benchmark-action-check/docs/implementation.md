# Benchmark-action relevance check

This nonpublished helper implements the inexpensive first step of the repository-specific
action-pairing skill. General crate version planning remains in `cargo-release-plan`.

The input is the final verified release report. The checker projects its pending package names
and intersects them with names from the action's authoritative release manifest. This includes
retained pending versions, first publication and dependent/group movement without deriving
versions a second time. Nonpublished alignment-only helpers are not publication candidates.

An empty pending set returns `false` before any manifest access. Other changes read only the
small manifest through the existing GitHub CLI authentication, not a checkout or PR listing.
Bootstrap and proposed action manifests can be supplied explicitly as a local file. Missing,
malformed or unsupported inputs fail rather than returning a success-shaped `false`.

Stdout contains exactly `true` or `false` on success; verbose reasoning goes to stderr.
The pure decision and deferred-loader ordering have in-memory tests. Native executable tests
exercise file input, exit status and output shape without network access.

The process entry point and its native adapters are excluded from library-only mutation testing:
observing their argument acquisition, file/process I/O and standard streams requires integration
tests. The small output/status mapping stays in this shell rather than adding an injectable
layer solely to duplicate its executable coverage. The relevance decision remains mutation-tested
in process, while `tests/cli.rs` verifies exact boolean-line output, stderr-only verbose diagnostics
and failing status with a diagnostic and empty stdout for invalid input. This follows the repository's
[unit/integration boundary and mutation exclusion policy](../../../docs/testing.md).

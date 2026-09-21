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

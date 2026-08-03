# cbh_git - Implementation

`cbh_git` implements process and repository access for the behavior described by the
[`cargo-bench-history` design](../../cargo-bench-history/docs/DESIGN.md). It owns benchmark-process
launching, captured helper commands, and the read-only git-history operations used to reconstruct
topology.

Port traits separate orchestration from Tokio process execution. Production adapters stream
benchmark output and invoke git commands, while feature-gated in-memory implementations provide
resolved refs, first-parent histories, merge bases, and failures deterministically in tests.

Repository commands preserve the distinction between invocation failure and a successful command
that reports no resolved ref. The latter remains an ambiguous unresolved result for callers to
describe neutrally; it does not claim that the selected path is outside a repository.

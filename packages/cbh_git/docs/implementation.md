# cbh_git implementation

`cbh_git` supports the repository and process behavior specified by the
[`cargo-bench-history` design](../../cargo-bench-history/docs/DESIGN.md). Its place in the
application is defined by the
[`cargo-bench-history` implementation guide](../../cargo-bench-history/docs/implementation.md)
and the workspace rules for [implementation documentation](../../../docs/implementation.md).

The crate owns benchmark execution and shared subprocess capture, including environment handling
for commands in historical worktrees. `GitHistory` provides the read-only repository-topology
boundary. The crate returns portable process outcomes and repository facts, while callers retain
command policy, history selection, worktree lifecycle, and analysis meaning. Parsing of captured
command output remains pure and separate from process execution.

Production adapters use Tokio processes, while private test support provides deterministic
repository substitutes for component tests. Unexpected invocation failures remain `io::Error`
values so the calling layer can add operation-specific context under the workspace
[error-handling guide](../../../docs/error-handling.md). A successful repository command that
does not resolve a ref remains a neutral unresolved result for its caller to contextualize rather
than being reclassified as an invocation failure.

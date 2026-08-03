# cbh_analyze - Implementation

`cbh_analyze` implements the query and mutation commands described by the
[`cargo-bench-history` design](../../cargo-bench-history/docs/DESIGN.md). The binary crate owns
the user-visible contract; this crate owns the selection, history reconstruction, analysis,
blessing, pruning, and report-production pipeline behind that contract.

The command implementations depend on storage, git history, reporting, clocks, and task spawning
through injected ports. Production entry points assemble concrete adapters, while unit tests use
in-memory implementations so selection and rendering behavior can be exercised deterministically.
Shared selection and data-set reconstruction paths keep `analyze`, `list`, `prune`, and `examine`
aligned where the design requires lockstep behavior.

Operational failures cross the crate boundary through a transparent aggregate error. Each layer
adds only the context it owns and retains lower-level typed errors as sources, including failures
that arise while validating command selections. This preserves condition-specific inspection and
causal diagnostics when the binary converts the result into `ohno::AppError`.

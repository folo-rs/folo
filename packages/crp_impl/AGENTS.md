# Working on crp_impl

Follow the [cargo-release-plan guidance](../cargo-release-plan/AGENTS.md) for Git
configuration, error behavior and in-process unit tests.

Keep implementation-boundary integrations and benchmarks in this package. The
`cargo-release-plan` package owns its supported facade and executable-connected
end-to-end suite. Do not forward implementation-only items through that facade.

Run `cargo test -p crp_impl -p cargo-release-plan --tests` for the combined
test surface. Mutation testing selects only library unit tests; real external
acquisition belongs in integration targets.

`DepTargets::declares` and `metadata::resolved_member` fall back to filesystem
canonicalization when lexical member lookup misses. Unit fixtures must supply
matching lexical member paths; put alias and lookup-miss cases in the boundary
integration target.

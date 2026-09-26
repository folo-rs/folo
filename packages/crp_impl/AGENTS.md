# Working on crp_impl

Follow the [cargo-release-plan guidance](../cargo-release-plan/AGENTS.md) for Git
configuration, error behavior and in-process unit tests.

Keep implementation-boundary integrations and benchmarks in this package. The
`cargo-release-plan` package owns its executable wiring and executable-connected
end-to-end suite. Do not forward implementation-only items through that boundary.
Keep both packages' library targets private to the application and maintainer tests;
follow the owning package's CLI-only release-contract guidance.

Run `cargo test -p crp_impl -p cargo-release-plan --tests` for the combined
test surface. Mutation testing selects only library unit tests; real external
acquisition belongs in integration targets.

After moving test modules, run
`just package="cargo-release-plan crp_impl" coverage-measure` on a supported native
platform. Each test executable using `coverage(off)` needs its own
`coverage_attribute` feature gate. Put an exclusion on the module declaration
or in its source file, not both.

`DepTargets::declares` and `metadata::resolved_member` fall back to filesystem
canonicalization when lexical member lookup misses. Unit fixtures must supply
matching lexical member paths; put alias and lookup-miss cases in the boundary
integration target.

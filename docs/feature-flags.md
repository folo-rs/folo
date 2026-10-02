# Feature flags

This chapter covers conditional compilation in the workspace: gating test-only
code with `#[cfg(test)]`, gating optional functionality behind Cargo features,
and how the two interact in `test` builds.

## Test-only code requires cfg(test)

If there are functions that are only used in tests, mark them (and their `use`
statements) with `#[cfg(test)]`. Do not just suppress "dead code" warnings.

## Feature-gated code should also be enabled by `test` build

If code is feature-gated, it should always also be enabled in test builds:
`#[cfg(any(test, feature = "foo"))]`

When a feature gate controls a dependency (e.g. `dep:futures-core` behind
`futures-stream`), ensure the dependency is also listed as a dev-dependency so it
is available in test builds without requiring the feature to be explicitly
activated.

## Private implementation APIs do not need visibility features

Use plain `pub` for lightweight implementation helpers that other workspace
crates' tests or benchmarks need. A private-API package already establishes their
unsupported status; `private-test-util` is not another hiding mechanism.

Use `private-test-util` only to exclude code that would be problematic to compile
in production, such as substantial fixture machinery or costly test-support
dependencies. Document the concrete compilation concern, not just the helper's
audience. The feature is permitted only on private-API packages and is never
forwarded by a public facade.

See [the implementation-crate guidance](impl-crate-split.md#internal-only-testbench-helpers-private-test-util)
for examples and dependency wiring. Same-crate unit-test helpers continue to use
`#[cfg(test)]`, without a Cargo feature.

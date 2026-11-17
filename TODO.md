# TODO

Tracking notes for follow-up work that is intentionally deferred. Each entry
should describe the task, the trigger condition that makes it actionable, and
links to the relevant code.

## Apply the latest-runner policy to the delegated release workflow

The action revision selected by [Release](.github/workflows/release.yml) delegates
native runner selection to
[cargo-release-plan-action's manifest](https://github.com/folo-rs/cargo-release-plan-action/blob/abcdd1b5f94b6c6e409b94a7e32724f22fb42540/release.json),
which selects `macos-15`, `ubuntu-24.04` and `windows-2025` where latest labels
can preserve the required architecture. Update those selections and directly
coupled workflow logic in the owning action repository under the
[hosted-runner policy](docs/build-and-tooling.md#hosted-runner-images).
The macOS correction belongs to
[folo-rs/cargo-release-plan-action#6](https://github.com/folo-rs/cargo-release-plan-action/pull/6);
use its merged revision rather than adopting an unmerged PR head.
Once that action revision is available, advance Folo's release and identity-probe
caller pins together, preserving native target coverage and publication behavior.
Keep the ARM64 Linux and Windows labels unless GitHub offers matching latest labels.

## Give dure-test-helper preparations stable executable ownership

The [helper locator](packages/dure-test-helper/src/locate.rs) returns a shared
Cargo final artifact after its build lock is released. Concurrent helper builds
can replace that path while readers inspect or spawn it. Its target-directory
comment also overstates what crate-level isolation guarantees. Address this in
a separately scoped helper repair, with deterministic filesystem/process
integration coverage, using the
[faker executable-ownership model](packages/cargo-bench-history-faker/docs/implementation.md#executable-ownership)
as prior art. Do not wait for a timing-dependent failure to make this actionable.

## Consider filter-safe cross-package classification hashing

Classification shares eligible fresh Git listings but keeps per-package
[`hash_objects`](packages/crp_workspace/src/git.rs) boundaries. Coalescing hashes is
deferred: the existing effective-attribute gate for mode sharing would need to cover
hash-process coalescing while preserving built-in normalization, per-package
symlink admission and ordering for ineligible requests. Revisit only when further
hash-process elimination justifies this extra acquisition and eligibility model;
do not infer safety from checked-in `.gitattributes` or presumed filter statelessness.

## Reorder bench-history object-key segments to `triple/machine/engine`

`cargo-bench-history` keys stored objects as
`v1/<project>/objects/<engine>/<target_triple>/<machine>/<commit>/…`
(`packages/cbh_model/src/comparability.rs`), putting the engine outermost.

Target triple and machine key identify completely independent data sets, so
they belong outermost, with the engines nested inside one machine's data. Under
the current order "everything this machine key recorded" is not a single
prefix, so a partition-scoped scan — such as `backfill`'s skip-existing
pre-check — needs one listing per engine instead of one listing overall.

Reordering rewrites every stored key, so it is not worth a storage-schema break
on its own. Do it as part of the next change that breaks the schema anyway.

## Enable newer Clippy module-root and bit-width lints

When the pinned Rust toolchain provides `clippy::definition_in_module_root` and
`clippy::manual_bit_width`, enable both as warnings in the root `Cargo.toml` and
adjust every reported violation. Rust 1.98.1 does not recognize either lint.

## Isolate remaining workflow concurrency groups by pull request

When updating general CI concurrency, replace source-branch grouping in
[Standard validation](.github/workflows/standard-validation.yml),
its [PR-close companion](.github/workflows/cancel-standard-validation.yml), and
[Book](.github/workflows/book.yml) with PR-number grouping. Same-named branches in
different forks can currently cancel each other's runs. Keep Standard validation
and its close companion's group expressions synchronized, including the companion's
reported group name, and preserve scheduled-run isolation.

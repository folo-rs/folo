# Agent notes for cargo-release-plan

## Git is a subprocess

Do not add `git2`, `gix`, or any other Git library. All Git access is
`std::process::Command` spawning `git`; the rationale and its trade-off are in
[docs/implementation.md](docs/implementation.md), "Subprocess boundaries".
Classification may also spawn `cargo metadata --no-deps`; verify-packaging may
spawn `cargo package --list`. Keep full resolution in explicit preparation and
preview, using `cargo update --offline --workspace`, never in report, check, or
application. Do not contact crates.io and do not compile as part of classification.

## Integration tests must be hermetic Git

Tests that create repositories must inject identity and throughput config on
every `git` invocation rather than relying on the user or host config:

* `user.email` / `user.name` pinned
* `commit.gpgsign=false`
* `gc.auto=0`

Use the helper in `tests/integration/fixture.rs` for executable-connected tests
or `crp_workspace`'s `private-test-util` repository fixture for implementation boundaries.
Do not add real-time delays.

The executable-connected integration suite is one test binary, `tests/integration/`, split into a
topic module per area of behavior over the shared `harness`. Add a new case to
the module that matches its subject rather than growing a single file.

Keep field and decision matrices in the owning module's unit tests. Do not build
a prepared/previewed workspace merely to test a validator or an output format.
Reuse one classification report for assertions about the same unchanged state.
Keep real Git/Cargo tests for boundary behavior; see
[test boundaries](docs/implementation.md#test-boundaries).

Library unit tests must not acquire real Git, Cargo or filesystem state, including
through fixture helpers or production acquisition methods. Small temporary Git
repositories and filesystem probes belong in `tests/integration/`, not `src/`.
Implementation-boundary assertions belong beside their owning component; its
ordinary internal operations may be public within the private application family.
Inject acquired observations into decision tests; do not recreate subprocesses
behind a fake protocol or widen the executable's internal re-export boundary.

## Modules own subjects, not categories

Follow the package ownership map in `docs/implementation.md`. Keep CLI parsing,
command values, dispatch and external compatibility execution in this application.
Components must not depend on it, including through dev-dependencies.
Do not add a shared module for
"types", "constants", or "utilities"; there is deliberately none to add to.

A subject-owned module keeps an item next to the code that gives it meaning, so
a reader who has found the behavior has also found everything that defines it,
and a change to that behavior touches one module. A category module inverts
this: it gathers unrelated items that share only a syntactic kind, so it accretes
dependencies on every subject and every subject depends back on it.

## Miri

Tests that spawn `git` or `cargo`, or that touch the real filesystem,
must be `#[cfg_attr(miri, ignore = "specific reason")]`. A Miri ignore is not
permission to put external I/O in the library harness. Pure unit
tests (packaging rules, group verdicts, plan expansion, inherited-value
comparison, anchor resolution over a synthetic timeline) must keep running
under Miri.

## Version groups come from exact dependencies

Every Git-tracked workspace member participates in version grouping, including
`publish = false` helpers. A valid exact local dependency
`=major.minor.patch` creates an undirected edge; connected components with more
than one member are groups. Preserve raw manifest syntax through validation
because Cargo metadata normalizes away distinctions the contract rejects. Keep
release classification publishable-only and keep the broader Cargo-visible
member set for dependent requirement rewrites. See `docs/design.md`, "Version
groups", and `docs/implementation.md`, "Workspace snapshots".

## Release-process ownership

The application owns reusable release behavior; implement it in the owning component and
document it in the owning design, implementation guide and user book.
Repository instructions and workflow callers select Folo policy rather than
reimplementing that behavior. Keep the shared action revision immutable and
validated. Preserve the registered `release.yml` caller and its publication
authorization boundary; source-mode checks do not authorize a live release.

Keep every application-family library target marked `private-api = true` with library documentation
disabled. Assess the CLI and artifact contracts defined in `docs/design.md`,
not internal Rust construction or exhaustive matching, when choosing release levels.

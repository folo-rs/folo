# Agent notes for cargo-release-plan

## Git is a subprocess

Do not add `git2`, `gix`, or any other Git library. All Git access is
`std::process::Command` spawning `git`; the rationale and its trade-off are in
[docs/implementation.md](docs/implementation.md), "Subprocess boundaries".
Classification may also spawn `cargo metadata --no-deps` (and
`cargo package --list` / `cargo update` for verify-packaging and apply). Do not
contact crates.io and do not compile as part of classification.

## Integration tests must be hermetic Git

Tests that create repositories must inject identity and throughput config on
every `git` invocation rather than relying on the user or host config:

* `user.email` / `user.name` pinned
* `commit.gpgsign=false`
* `gc.auto=0`

Use the helper in `tests/integration/fixture.rs`. Do not add real-time delays.

The integration suite is one test binary, `tests/integration/`, split into a
topic module per area of behavior over the shared `harness`. Add a new case to
the module that matches its subject rather than growing a single file.

## Miri

Tests that spawn `git` or `cargo`, or that touch the real filesystem beyond
in-memory data, must be `#[cfg_attr(miri, ignore)]` with a reason. Pure unit
tests (packaging rules, group verdicts, plan expansion, inherited-value
comparison, anchor resolution over a synthetic timeline) must keep running
under Miri.

## Version groups live in two declarations

Group membership is `[workspace.metadata.release-plan.groups]` in the repo-root
`Cargo.toml`. `release-plz.toml` `version_group` keys also participate in
release automation. When adding or moving a grouped package, update both.

## Release-process ownership

`docs/git-workflow.md`, `RELEASING.md`, Validation workflows, and
`just gh-release` own the remaining release workflow. Do not change them from
this package.

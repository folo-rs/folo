# Agent notes for cargo-release-plan

## Git is a subprocess

Do not add `git2`, `gix`, or any other Git library. All Git access is
`std::process::Command` spawning `git`. Classification may also spawn
`cargo metadata --no-deps` (and `cargo package --list` / `cargo update` for
verify-packaging and apply). Do not contact crates.io and do not compile as
part of classification.

## Integration tests must be hermetic Git

Tests that create repositories must inject identity and throughput config on
every `git` invocation rather than relying on the user or host config:

* `user.email` / `user.name` pinned
* `commit.gpgsign=false`
* `gc.auto=0`

Use the helper in `tests/common/mod.rs`. Do not add real-time delays.

## Miri

Tests that spawn `git` or `cargo`, or that touch the real filesystem beyond
in-memory data, must be `#[cfg_attr(miri, ignore)]` with a reason. Pure unit
tests (packaging rules, group verdicts, plan expansion, inherited-value
comparison, anchor resolution over a synthetic timeline) must keep running
under Miri.

## Version groups live in the workspace manifest

Group membership is `[workspace.metadata.release-plan.groups]` in the repo-root
`Cargo.toml`. `release-plz.toml` `version_group` keys still exist and must be
kept in lockstep until a later layer drops `release-plz update`. When adding a
grouped crate, update both.

## Do not invert the current release process

This package implements the agreed versioning tool. It does not change
`docs/git-workflow.md`, `RELEASING.md`, Validation workflows, or
`just gh-release`.

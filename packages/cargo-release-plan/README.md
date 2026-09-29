# cargo-release-plan

A Cargo subcommand for reviewing version changes and publishing the resulting
crates, GitHub releases and native binaries. It connects pre-merge release
planning with delivery of the exact versions approved in the merged source.

The supported interfaces are the command line and documented configuration and
artifact formats, not a Rust library API.

## Install

Use [`cargo-binstall`](https://github.com/cargo-bins/cargo-binstall) to select a
prebuilt binary, with source installation as its fallback:

```text
cargo binstall cargo-release-plan --locked
```

Or build the published source:

```text
cargo install cargo-release-plan --locked
```

For automation, select an exact published version and a matching documentation
and action revision. See the
[installation guide](https://folo-rs.github.io/folo/cargo-release-plan/integration/installation.html)
for toolchain prerequisites and verification.

## Prepare a version-increment PR

Install the complete `increment-versions` skill and ask an agent to assess the
contribution. The skill is the supported version-planning interface; it reads
the release report, chooses semantic impacts, previews workspace effects and
applies the complete plan.

For a stacked PR, identify its parent so the skill can assess the child's changes
against that parent's final version and content. An explicitly requested
standalone run can prepare a version-increment PR without publishing configuration.

## Plan and publish

Follow the [user guide](https://folo-rs.github.io/folo/cargo-release-plan/) for the
complete process:

- [Configure a repository](https://folo-rs.github.io/folo/cargo-release-plan/integration/repository.html):
  declare version groups, consumer contracts and publication inputs.
- [Prepare a version plan](https://folo-rs.github.io/folo/cargo-release-plan/integration/local-planning.html):
  use the agent skill to assess and apply the complete workspace change.
- [Connect publication](https://folo-rs.github.io/folo/cargo-release-plan/integration/publication.html):
  publish reviewed exact versions through the reusable GitHub workflows.
- [Recover a failed release](https://folo-rs.github.io/folo/cargo-release-plan/operations/recovery.html):
  use the workflow failure and its issue to choose a safe retry or repair.

The [command reference](https://folo-rs.github.io/folo/cargo-release-plan/reference/commands.html)
documents prerequisites, outputs and side effects. New packages require the
maintainer setup in the
[first-publication guide](https://folo-rs.github.io/folo/cargo-release-plan/operations/first-publication.html)
before automatic publication.

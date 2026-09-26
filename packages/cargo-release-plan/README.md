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

## Inspect a workspace

From a Git-tracked Cargo workspace with release history available:

```powershell
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
cargo release-plan --version
cargo release-plan report --out-dir .release-plan-review
```

The report records every publishable package's release status and dependency
relationships, with patches for changed released files. Each package is compared
with its version anchor: the most recent first-parent commit that changed its
parsed version within the selected release history.

Pass `--base <release-baseline>` to select that history explicitly, especially in
CI or a stacked pull request. The release baseline is the release-branch commit,
not an unreleased parent PR. Reporting does not choose version changes or publish
anything.

## Plan and publish

Follow the [user guide](https://folo-rs.github.io/folo/cargo-release-plan/) for the
complete process:

- [Configure a repository](https://folo-rs.github.io/folo/cargo-release-plan/integration/repository.html):
  declare version groups, consumer contracts and publication inputs.
- [Prepare a version plan](https://folo-rs.github.io/folo/cargo-release-plan/integration/local-planning.html):
  inspect evidence, choose semantic decisions and preview complete dependency
  effects before applying them. The agent skill is optional.
- [Connect publication](https://folo-rs.github.io/folo/cargo-release-plan/integration/publication.html):
  publish reviewed exact versions through the reusable GitHub workflows.
- [Verify delivery](https://folo-rs.github.io/folo/cargo-release-plan/operations/verification.html)
  and [recover incomplete work](https://folo-rs.github.io/folo/cargo-release-plan/operations/recovery.html):
  distinguish version readiness from actual package and archive availability.

The [command reference](https://folo-rs.github.io/folo/cargo-release-plan/reference/commands.html)
documents prerequisites, outputs and side effects. New packages require the
maintainer setup in the
[first-publication guide](https://folo-rs.github.io/folo/cargo-release-plan/operations/first-publication.html)
before automatic publication.

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
cargo release-plan version
cargo release-plan report --out-dir .release-plan-review
```

The report records every publishable package's release status and dependency
relationships, with patches for changed released files. Each package is compared
with its version anchor: the most recent first-parent commit that changed its
parsed version within the selected release history.

Pass `--release-history <commit>` to select actual release history explicitly.
For a stacked PR, also pass `--merge-target <parent-commit>`: the parent's final
version and content are treated as an anticipated squash release, and additional
child changes need their own increment. Existing release-history anchors still
retain unversioned catch-up changes. Reporting does not choose versions or publish.

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

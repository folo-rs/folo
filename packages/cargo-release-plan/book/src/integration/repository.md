# Configure your repository

This walkthrough uses `example/widgets`, a placeholder GitHub repository.
Replace it with your repository and select the branch that actually releases.
The TOML fragments below belong in existing manifests; they are not complete
standalone Cargo projects.

## Workspace inputs

Track the workspace and package manifests, publication configuration, lockfile
and build-toolchain selection. Prepare a consistent lockfile before collecting
release evidence. Publication verifies the committed resolution rather than
repairing it.

Choose package inclusion rules using the
[released-content model](../concepts/released-content.md). For example:

```toml
# Workspace Cargo.toml
[workspace.package]
include = ["src/**/*"]
```

```toml
# packages/widget/Cargo.toml, within [package]
include.workspace = true
```

Packages needing additional compile-time inputs declare a complete allow-list of
their own.

## Declare groups and consumer contracts

The library uses its exact-version implementation:

```toml
# packages/widget/Cargo.toml
[dependencies]
widget_impl = { path = "../widget_impl", version = "=1.4.0" }

[package.metadata.cargo_check_external_types]
allowed_external_types = ["widget_impl::*"]
```

The implementation is published so Cargo can resolve it, but does not promise
an independently supported API:

```toml
# packages/widget_impl/Cargo.toml
[package.metadata.release-plan]
private-api = true
```

A helper can share alignment without being published:

```toml
# packages/widget-fixtures/Cargo.toml
[package]
name = "widget-fixtures"
version = "1.4.0"
publish = false

[dependencies]
widget_impl = { path = "../widget_impl", version = "=1.4.0" }
```

The exact edges join all these members. Do not add a separate workspace group
table. An ordinary `version = "1.4.0"` requirement names the same declared
version but does not create an exact-version group.

## Verify external-type exposure

Every library's allow-list describes types outside that crate which its public
API may expose. Use defining crate paths, including implementation paths reached
through re-exports. A crate-level glob is useful for your own implementation
partition; list other external types narrowly so accidental exposure remains
visible.

An omitted allow-list declares no external exposure; it does not mean exposure
is unknown. `cargo-release-plan check` reads declarations but does not compile
and verify them. Adopt
[`cargo-check-external-types`](https://github.com/awslabs/cargo-check-external-types)
as a required API-validation gate.

Pin the checker and its matching rustdoc nightly together. This concrete pair
illustrates that separate pinning:

```powershell
$ExternalTypesRevision = "705a0941997ebeb3bfb1a7f14070ba342f79d879"
$ExternalTypesNightly = "nightly-2026-03-20"
rustup toolchain install $ExternalTypesNightly --profile minimal
cargo install cargo-check-external-types --locked `
    --git https://github.com/awslabs/cargo-check-external-types `
    --rev $ExternalTypesRevision
$WidgetManifest = Join-Path "packages" "widget" "Cargo.toml"
cargo "+$ExternalTypesNightly" check-external-types --all-features `
    --manifest-path $WidgetManifest
```

Run the check for each library that can expose external types, including
implementation packages. Cover the relevant feature and platform surfaces; an
all-features run on Linux cannot prove a Windows-only signature. Review unused
entries in context because some are needed only on another target. Avoid
overlapping patterns.

Feed these checks into your required status alongside tests, builds and release
checks. A successful version check cannot substitute for this prerequisite.

## Configure publication

Commit `.cargo/release_plan.toml` at the selected workspace root:

```toml
schema-version = 1
repository = "example/widgets"
release-branch = "main"
targets = [
    "x86_64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
]
```

This example selects native Linux and Windows x64 binaries. A library-only
workspace can use `targets = []`. The full supported target set and precedence
are in the [configuration reference](../reference/configuration.md).

`--config` is workspace-relative, even when invoking from elsewhere. For a
nested workspace:

```powershell
$WorkspaceManifest = Join-Path "components" "widgets" "Cargo.toml"
$Configuration = Join-Path ".cargo" "release_plan.toml"
cargo release-plan check --manifest-path $WorkspaceManifest --config $Configuration
```

This reads `components\widgets\.cargo\release_plan.toml`. Artifact paths inside
publication files are instead repository-relative so different jobs can transport
them without sharing absolute checkout paths.

## Describe the binary archive

`widget-cli` is the package; `widget` is its executable:

```toml
# packages/widget-cli/Cargo.toml
[package]
name = "widget-cli"
version = "2.0.0"
repository = "https://github.com/example/widgets"

[[bin]]
name = "widget"
path = "src/main.rs"

[dependencies]
widget = { path = "../widget", version = "1.4.0" }

[package.metadata.binstall]
pkg-url = "{ repo }/releases/download/{ name }-v{ version }/{ name }-v{ version }-{ target }.zip"
bin-dir = "{ bin }{ binary-ext }"
pkg-fmt = "zip"
```

Binary packages must have one executable buildable with default features, and
their repository and binstall layout must match publication configuration.
Package `release-targets` can narrow the workspace selection:

```toml
[package.metadata.release-plan]
release-targets = ["x86_64-pc-windows-msvc"]
```

Use this only for a genuinely restricted package. Binary packages must retain at
least one selected target.

Validate the inputs without querying a registry:

```powershell
cargo release-plan check --config (Join-Path ".cargo" "release_plan.toml")
```

Configuration changes are often released manifest changes too. Continue with
[local planning](local-planning.md) rather than fixing a failing version check by
hand-incrementing isolated packages.

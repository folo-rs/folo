# Configuration reference

Publication uses committed TOML and package metadata. Source commits, output
locations and attempt identities are invocation inputs, not persistent policy.

## Workspace publication configuration

The default file is `.cargo/release_plan.toml` relative to the selected Cargo
workspace. `--config` selects another workspace-relative file. A nested workspace
uses its own root, not the shell's invocation directory.

This is a complete example of the configuration format:

```toml
schema-version = 1
repository = "example/widgets"
release-branch = "main"
targets = [
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
    "aarch64-apple-darwin",
]
```

Replace the example repository and branch; no consumer branch name is mandated.

| Key | Meaning |
| --- | --- |
| `schema-version` | Configuration schema, currently `1`. This spelling uses a hyphen, unlike JSON artifact fields. |
| `repository` | GitHub destination in `owner/repository` form. |
| `release-branch` | Branch whose merged source is eligible for publication. |
| `targets` | Selected supported native targets for binary archives. |

For a library-only workspace, an empty `targets = []` is valid. For a binary
package, the effective target set must be nonempty.

The supported native target set is:

| Target | Platform |
| --- | --- |
| `x86_64-unknown-linux-gnu` | Linux x64 |
| `aarch64-unknown-linux-gnu` | Linux ARM64 |
| `x86_64-pc-windows-msvc` | Windows x64 |
| `aarch64-pc-windows-msvc` | Windows ARM64 |
| `aarch64-apple-darwin` | macOS ARM64 |

The action revision selects matching native runners. Intel macOS is not in this
support set.

`check --config` validates configuration and binary publication inputs offline.
Ordinary `check` without that option retains its version-readiness scope.
Publication reads configuration by default. None of these settings silently
changes the baseline default for version assessment; automation supplies
`--base` explicitly.

## Package metadata

These entries are optional:

```toml
[package.metadata.release-plan]
private-api = true
release-targets = ["x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc"]
```

`private-api` declares that a library does not offer an independently supported
consumer API. It defaults to public when absent; binary-only packages have no
library contract to compare. The declaration does not disable publication or
dependency propagation.

A CLI package with an internal library target must also declare `private-api = true`
when it offers no supported Rust library API. This does not waive compatibility
requirements for its command line or documented artifact formats.

`release-targets` narrows the workspace's binary target selection. Omit it for
the whole workspace selection. Invalid targets or an empty effective binary
selection are errors.

Cargo's `publish = false` is different: it excludes the package from publication
while retaining tracked version-group alignment.

Version groups come from exact local workspace dependencies, not metadata.
The obsolete `[workspace.metadata.release-plan.groups]` table is not accepted.

## External types

```toml
[package.metadata.cargo_check_external_types]
allowed_external_types = [
    "widget_impl::*",
    "other_library::PublicType",
]
```

Entries describe defining crate paths, including re-exported types. Verify them
with `cargo-check-external-types` for supported features and platforms. An absent
list allows no external exposure. See the
[adoption prerequisite](../integration/repository.md#verify-external-type-exposure).

## Binary metadata and naming

Each publishable binary package has one executable buildable with default
features. Its Cargo `repository` matches the configured GitHub destination, and
its binstall metadata describes the standard ZIP layout:

```toml
[package.metadata.binstall]
pkg-url = "{ repo }/releases/download/{ name }-v{ version }/{ name }-v{ version }-{ target }.zip"
bin-dir = "{ bin }{ binary-ext }"
pkg-fmt = "zip"
```

The brace expressions are literal cargo-binstall template syntax, not
placeholders to replace manually.

| Artifact | Naming contract |
| --- | --- |
| Package tag and binary release tag | `{package}-v{version}` |
| Archive | `{package}-v{version}-{target}.zip` |
| Sidecar | `{package}-v{version}-{target}.sha256` |
| ZIP member | Executable name at archive root, with `.exe` on Windows |

The sidecar does **not** append `.sha256` to the ZIP filename. Package and
executable names can differ. Unix ZIP members retain executable permissions.
Both assets must be uploaded for the pair to be complete.

Publication does not silently enable additional features to reach a gated
binary, accept arbitrary Cargo build arguments or cross-compile an archive.

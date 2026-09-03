# Determining a change level

Use this guide for every publishable package in the release-plan report. The decision concerns
the complete released change since the package's version anchor, including changes already
pending release.

## Evidence to inspect

Read the package entry in `report.json`, its referenced diff when present, the package's
`Cargo.toml`, the workspace `Cargo.toml`, and the decisions for its dependencies. Direct
`[package]` and dependency-table edits appear in the package's released-content diff alongside
any other changed published files. Values inherited through `.workspace = true` instead
appear in the report's `changed` array with `source: "inherited"` and may have no package
diff.

Evaluate every `source: "inherited"` entry for every package. A workspace-level edit affects
each package that inherits the edited field; do not limit the decision to a package selected
by the original file diff. For example, increasing `[workspace.package] rust-version`
establishes a `patch` change for every publishable package that inherits `rust-version`.
Every package in this workspace inherits that field, so an MSRV increase requires a `patch`
decision for every previously published package in the report. A new package has no version
anchor to increment and follows the first-publication path instead.

`cargo-semver-checks` detects part of the Rust API surface. A finding establishes a
`breaking` minimum. No finding means the tool could not determine a required version
increment; it does not establish that the package is compatible.

## Breaking

Choose `breaking` when existing consumers may need to change or may observe an incompatible
contract. Examine public API removal or reshaping, stricter input requirements, changed output
or persisted formats, incompatible command-line behavior, and changed semantic guarantees.

For a public package backed by implementation packages, judge the behavior and API exposed
through the public package. Internal handoff changes matter only when they alter that exposed
contract.

## Nonbreaking

Choose `nonbreaking` for a meaningful compatible capability: a new API, supported input,
command, output option, or documented behavior that existing consumers can ignore.

Do not choose this level merely because implementation volume is large. The level describes
the consumer-visible change.

## Patch

Choose `patch` for compatible corrections, performance improvements, documentation changes
included in the published package, or internal changes that alter released content without
adding a meaningful consumer-facing capability.

A direct `[package]` metadata change or the corresponding inherited
`[workspace.package]` change establishes at least `patch` for every affected package. This
rule applies to every package metadata field, including `rust-version` (the minimum supported
Rust version). Combine this minimum with the package's other evidence and choose the highest
applicable change level.

Treat dependency and feature-table changes separately: analyze the consumer impact of the
resulting dependency or feature behavior rather than assuming every `Cargo.toml` edit is
metadata-only.

## No increment

Choose no increment only when the package's released-content diff, its changed inherited
workspace fields, and the decisions for its dependencies require no change. Signal this by
omitting the package from the semantic change-decision file.

Analyze packages that are already `pending-release` with the same criteria. Existing version
movement is retained, but it does not replace analysis of the accumulated changes.

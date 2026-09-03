# Determining a change level

Use this guide for every publishable package in the release-plan report. The decision concerns
the complete released change since the package's version anchor, including changes already
pending release.

## Evidence to inspect

Read the package entry in `report.json`, its referenced diff when present, and the decisions
for its dependencies. Inherited workspace fields and dependency requirement changes can be
released changes even when the package has no source-file diff.

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

## No increment

Choose no increment when neither the package's released content nor the decisions for its
dependencies require a change. Signal this by omitting the package from the semantic
change-decision file.

Analyze packages that are already `pending-release` with the same criteria. Existing version
movement is retained, but it does not replace analysis of the accumulated changes.

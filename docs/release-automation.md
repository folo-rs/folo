# Release automation in Folo

The [cargo-release-plan user guide](https://folo-rs.github.io/folo/cargo-release-plan/)
owns the reusable publication process, source identities and recovery rules.
This chapter records Folo's selection and operational entry points.

## The release.yml workflow

`.github/workflows/release.yml` is the caller registered with crates.io Trusted
Publishing for `folo-rs/folo`. It runs on pushes to `main` and explicit dispatches.
Keep that filename stable when changing reusable orchestration; it is part of
the publisher identity. Registry jobs use OIDC, GitHub reconciliation/binary jobs
use the ambient repository token, and failure reporting needs issue permission.

The workflow implementation and compatibility entry points are described in
[the workflow implementation guide](../.github/workflows/implementation.md).
The invocation checkout at the workflow event SHA supplies the release controller;
binary builds use separate immutable source worktrees at the peeled package-tag commits.

Publication runs are not cancelled when another merge arrives. A failed run
retains its original requests and is retried rather than replacing them with the
newest branch state. Follow the book's
[publication walkthrough](https://folo-rs.github.io/folo/cargo-release-plan/integration/publication.html)
and [delivery verification](https://folo-rs.github.io/folo/cargo-release-plan/operations/verification.html).

## Target selection

`.cargo/release_plan.toml` selects Folo's repository, release branch and supported
native targets. The binary set is discovered from publishable Cargo packages,
not a maintained package list. Package restrictions use
`[package.metadata.release-plan] release-targets`; `dure` selects Windows targets.

ZIP creation and checksums are built into the application; no archiver installation
is required. The reusable action supplies its own minimal compiler/tool bootstrap
rather than importing Folo's complete development environment.

## The asset-naming contract

The book's [configuration reference](https://folo-rs.github.io/folo/cargo-release-plan/reference/configuration.html)
defines package tags, root-executable ZIPs and SHA-256 sidecars, including the
matching `cargo-binstall` metadata. Folo uses that contract without a separate
archive convention. Changes to the release engine must preserve already
published tag and asset identities.

## Failure recovery

Follow the [recovery guide](https://folo-rs.github.io/folo/cargo-release-plan/operations/recovery.html).
The selected publisher's diagnostics and original workflow run identify the
recovery work. The unified publication report additionally retains the exact
missing tag and original publication source for superseded-version recovery.
An operator creates only that missing tag with appropriate rights, then retries
the original failed workflow. Existing tags remain unchanged.

Other successfully published packages and archive pairs remain in place.
The original workflow artifacts are required for ordinary failed-job retry;
expired evidence needs explicit-source recovery rather than silent rediscovery.

## First publish of a new crate

Complete the [Folo maintainer handoff](../RELEASING.md#first-publish-of-a-new-crate)
before the crate's first merge.

## Tool and action release coordination

Tool and action versions are independent. Actual registry packages and every
promised archive must be available before their corresponding action release.
Source-mode CI does not replace that installation gate.

Folo's benchmark integration has its own
[paired-action policy](benchmark-action-releases.md).
[`folo-rs/cargo-release-plan-action`](https://github.com/folo-rs/cargo-release-plan-action)
has its own release stream and exact published-installation gate.

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

The workflow callers and validation integration are described in
[the workflow implementation guide](../.github/workflows/implementation.md).
Source changes to the release tool are exercised from the event checkout; binary
release sources remain separate immutable tag checkouts.

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

Folo's native archive prerequisites are maintained by
`scripts/setup/ReleaseArchiveTools.psm1` through `just install-tools`.
The reusable action supplies its own minimal bootstrap rather than importing
Folo's complete development environment.

## The asset-naming contract

The book's [configuration reference](https://folo-rs.github.io/folo/cargo-release-plan/reference/configuration.html)
defines package tags, root-executable ZIPs and SHA-256 sidecars, including the
matching `cargo-binstall` metadata. Folo uses that contract without a separate
archive convention. Changes to the release engine must preserve already
published tag and asset identities.

## Failure recovery

Follow the [recovery guide](https://folo-rs.github.io/folo/cargo-release-plan/operations/recovery.html).
The workflow reports incomplete publication with a run-qualified issue. If
concurrent merges make an older version impossible to tag automatically, its
issue names the exact missing tag and original publication-source commit.
An operator creates only that missing tag with appropriate rights, then retries
the original failed workflow. Do not retag an existing release or use today's
branch tip in place of the recorded source.

Other successfully published packages and archive pairs remain in place.
The original workflow artifacts are required for ordinary failed-job retry;
expired evidence needs explicit-source recovery rather than silent rediscovery.
Dispatch `release.yml` on `main` with `source` set to the original full commit ID
for that recovery. Leave `source` empty for ordinary releases. The controller
still builds from the invocation checkout, independently of the release source.

The mutually exclusive `verify-publishing-identity` dispatch exchanges and revokes
a credential without publishing. Use it only to verify the registered caller,
not as proof of complete package or archive delivery.

## Cutover authorization

Enabling a publisher revision requires explicit authorization and the shared
action's completed acceptance gates. Keep the caller pinned to the tested action
commit; source dogfooding does not replace its published-installation or live
acceptance evidence. Repository protection and first-publication setup remain
operator responsibilities.

Retain the outer `release-${{ github.ref }}` concurrency group with non-cancelling
`queue: max` until all earlier publisher runs, including pending runs and queued
reruns, have drained or been explicitly handled. The shared workflow's inner
workspace lock must remain distinct. Inspect those runs before cutover; never
enable two publisher paths concurrently.

## First publish of a new crate

Use the [first-publication procedure](https://folo-rs.github.io/folo/cargo-release-plan/operations/first-publication.html)
before the crate's first merge. Folo's registration is owner `folo-rs`, repository
`folo`, caller workflow `release.yml`. The first automated release carries a
version above the manually published bootstrap version.

## Tool and action release coordination

Tool and action versions are independent. Actual registry packages and every
promised archive must be available before their corresponding action release.
Source-mode CI does not replace that installation gate.

Folo's benchmark integration has its own
[paired-action policy](benchmark-action-releases.md). The release-action bootstrap
uses the same pinning and publication-order pattern, with its own repository and
release stream.

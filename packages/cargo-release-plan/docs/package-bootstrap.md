# Implementation package release prerequisites

The private implementation packages are published only to make the command-line
application installable. None provides a supported Rust library API. Their exact
normal dependencies declare one version group with the executable.

## Publication ordering

The dependency order is diagnostics, workspace, versioning and native,
publication, then the executable. No implementation package depends on the
executable. The unified `publish registry` operation delegates dependency scheduling
to Cargo and uses the exact versions from the reviewed release plan.
Folo's selected publisher is described by the
[workflow implementation](../../../.github/workflows/implementation.md#release-publication);
the `release-plz` bootstrap path does not exercise the unified registry operation.

## Introducing another implementation package

Follow the [first-publication procedure](../../../RELEASING.md#first-publish-of-a-new-crate)
before a new package's first merge, using explicitly authorized maintainer
authentication for its bootstrap. Configure that package's Trusted Publisher for
`folo-rs/folo` and the registered `release.yml` caller, including its configured
environment where applicable. The first merged automated version must be strictly
higher than the bootstrap version and aligned with its version group.

## Verification and release authorization

Generate a fresh source-bound version plan against the actual release branch.
Require the plan-scoped registry preflight to pass, apply the captured plan
unchanged, and verify group alignment, dependency requirements and lockfiles.
Registry availability and operator-confirmed Trusted Publisher configuration are
distinct from evidence that a production upload has succeeded.

Path installation and passing native tests do not substitute for installation of
the intended published packages and archives. Version-plan application does not
authorize publication or merging. The
[`cargo-release-plan-action` release](https://github.com/folo-rs/cargo-release-plan-action)
owns the required published-installation gate; Folo's caller pins its selected action
revision under the [action integration boundary](implementation.md#reusable-action-boundary).

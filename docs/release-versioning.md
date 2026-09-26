# Release versioning in Folo

The [cargo-release-plan user guide](https://folo-rs.github.io/folo/cargo-release-plan/)
owns the reusable release model and authoring workflow. This chapter selects Folo's
repository policy rather than maintaining another explanation of that process.

## The invariant

Folo releases from `main`. A contribution changing released content includes its
reviewed version changes; merging the complete contribution starts publication.
The repository configuration is `.cargo/release_plan.toml`.

Read the book for [release history and anchors](https://folo-rs.github.io/folo/cargo-release-plan/concepts/history.html),
[released content](https://folo-rs.github.io/folo/cargo-release-plan/concepts/released-content.html),
and [semantic version decisions](https://folo-rs.github.io/folo/cargo-release-plan/concepts/versions.html).
Registry delivery progress is separate from version validity.

## On a pull request

Run the self-contained `increment-versions` skill without a separate approval pause.
In this repository, explicitly select the `cargo-release-plan` executable built
from the current source checkout, so unreleased command changes are exercised
rather than accidentally invoking an older installed release.

Human review of the complete PR is the approval step. Keep its
[Version/release plan](git-workflow.md#versionrelease-plan-section) current with
the complete expanded target set and retained pending releases. After every
assessment or reassessment, run the repository-specific
`pair-benchmark-action-release` extension; its fast relevance check determines
whether benchmark-action coordination is needed.

The [local-planning walkthrough](https://folo-rs.github.io/folo/cargo-release-plan/integration/local-planning.html)
owns the command sequence and evidence interpretation. The skill uses installed
tool operations for compatibility evidence and publication preflight, not Folo
Just wrappers. Repository convenience recipes remain available to development
and CI callers.

## Release-branch movement during planning

Use the skill's recovery procedure and the book's
[recovery guide](https://folo-rs.github.io/folo/cargo-release-plan/operations/recovery.html).
Preserve independently authored edits; only a verified generated versioning delta
may be undone automatically. Refresh the release baseline and the PR section.

## Version groups

Follow the [version-group model](https://folo-rs.github.io/folo/cargo-release-plan/concepts/versions.html)
and [workspace dependency conventions](dependencies.md). Folo's exact
intra-workspace dependencies declare shared versions, including private
alignment-only helpers. Do not widen an exact requirement merely to evade group
membership.

## Conservative breaking-change propagation

Folo validates its external-type allow-lists through
[the external-types gate](external-types.md). Those declarations support the
book's [public-dependency propagation rule](https://folo-rs.github.io/folo/cargo-release-plan/concepts/versions.html).
An unchecked allow-list is not evidence that a public dependency exposes no types.

## Repository CI

Standard validation runs workspace-wide version readiness regardless of Cargo
delta selection, together with publication metadata and scoped API compatibility.
The merge queue deliberately runs the narrower version-readiness gate against
its tested queue base. Keep the literal `required-checks` fan-in name and treat
unavailable mandatory checks as failures, not valid skips.

The owning [workflow design](../.github/workflows/design.md) and
[workflow maintenance instructions](../.github/workflows/AGENTS.md) define Folo's
job wiring. Generic caller setup is in the book's
[GitHub-check integration](https://folo-rs.github.io/folo/cargo-release-plan/integration/github-checks.html).

## Publication and first releases

See [Folo release operations](release-automation.md) and [RELEASING.md](../RELEASING.md).
New crates require the explicit pre-merge maintainer bootstrap described in the
[first-publication guide](https://folo-rs.github.io/folo/cargo-release-plan/operations/first-publication.html).

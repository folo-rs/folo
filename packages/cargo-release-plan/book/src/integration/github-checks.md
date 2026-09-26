# Add GitHub checks

The standard reusable check workflow is
`folo-rs/cargo-release-plan-action/.github/workflows/check.yml`.
Select a tested published release from the
[action repository](https://github.com/folo-rs/cargo-release-plan-action) and
replace `ACTION_REVISION` below with its verified immutable commit. Use that same
commit for every action/workflow example in this book; `ACTION_REVISION` is a
placeholder, not an existing release or a GitHub Actions variable.

Add a caller such as `.github/workflows/release-checks.yml`:

```yaml
name: Release checks

on:
  pull_request:
  merge_group:

permissions:
  contents: read
  actions: read

jobs:
  release-check:
    uses: folo-rs/cargo-release-plan-action/.github/workflows/check.yml@ACTION_REVISION
    with:
      working-directory: .
      config: .cargo/release_plan.toml
      install-method: binstall
      source-path: .
```

These inputs show their defaults. `working-directory` selects the consumer
workspace; `config` is relative to it. `source-path` selects tool source only
with `install-method: path`, not the consumer workspace for released
installation. The reusable workflow and its internal composite use the same
immutable action revision.

For a custom graph, the root composite provides individual operations. Its
installation and configuration inputs are described under
[custom jobs](../advanced/custom-jobs.md). A version-readiness operation is
deliberately narrower than the complete PR gate.

## Required check coverage

A complete release gate combines:

- Workspace-wide version readiness, including packages not directly edited.
- Publication-input validation using committed configuration.
- External compatibility evidence for the selected public library contracts.
- Consumer-owned external-type exposure checks and ordinary build/test checks.

Do not scope offline version readiness to a changed-package list. It must find
unversioned released content anywhere in the workspace, including inherited
inputs and effects carried from an earlier contribution.

The core offline invocation, after the caller has selected `$Baseline`, is:

```powershell
cargo release-plan check --base $Baseline `
    --config (Join-Path ".cargo" "release_plan.toml") --format github
```

This command alone is not a replacement for external compatibility checking.
`--format github` adds annotations; it does not change the release rules.

The equivalent compatibility gate uses fresh bound evidence and rejects
insufficient increments:

```powershell
cargo release-plan check-compatibility --base $Baseline `
    --output (Join-Path ".release-plan-work" "ci-compatibility") --deny-findings
```

Use a new output directory. `check-compatibility` acquires a read-only report
from the selected source and checks its identity around the comparison; it does
not accept a detached report as permission to check another checkout.

The compatibility stage must distinguish a valid empty selection from a broken
checker or unavailable comparison. Run the checker on the assessed source with
the supported feature selection, and retain diagnostics. All-features checking
does not remove the author's feature-subset and behavioral review obligations.

## Fetch and select the correct history

Use a full-history checkout. Resolve the baseline once for the tested event:

| Event | Baseline selection |
| --- | --- |
| Ordinary or stacked PR | Actual base-repository release-branch tip, not the PR target branch. |
| Merge queue | The queue candidate's release-branch base, `merge_group.base_sha`. |
| Release-branch push | The immutable source commit being tested. |
| Scheduled or manual source check | The explicitly selected immutable release source. |

Fetch the required base-repository history for fork PRs too. A similarly named
branch in the fork is not a substitute.

For a custom graph, `release-context --config <path>` fetches the configured
release branch and returns `release_base`. Supply
`release-context --base <tested-commit>` when the event already fixes the
baseline, especially for a merge queue. Readiness receives that same frozen
commit; it does not independently fetch a newer one.

A repository can keep a deliberately narrow queue gate using the lower
version-readiness operation and the tested queue baseline. That does not remove
the full compatibility and publication-input checks from ordinary PR validation.

## Protect the branch

Configure branch protection and, where appropriate, a merge queue for the chosen
release branch. Require a stable status that represents all merge-blocking work.

A final aggregation job is useful for dynamic matrices: it succeeds only when
required prerequisites succeeded or were deliberately not applicable. Failure,
cancellation, missing results and an unexpectedly skipped unconditional gate do
not count as success.

Require the status on both PR and queue events. A queue check whose workflow
never runs cannot establish readiness. Choose your own stable check name; no
particular Folo job name is required.

## Keep checks read-only

PR checking needs source/history access, not publication credentials, write
permission or OIDC authority. Follow the repository's normal approval policy for
fork workflow runs. An unapproved or policy-disallowed check is not an executed
pass.

Do not use privileged `pull_request_target` execution of contributor source as
an adoption shortcut. Mutation privileges belong only in the separately gated
[publication workflow](publication.md).

Before making the check required, exercise a passing change, an intentionally
missing increment, a group/dependency violation and the queue event. Confirm the
external-type gate covers your supported public API surfaces and that
compatibility execution failures remain failures in the final status.

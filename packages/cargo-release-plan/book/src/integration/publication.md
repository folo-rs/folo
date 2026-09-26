# Connect publication

Publication starts from clean merged source under your repository's release
policy. Its preparation step is automatic execution, not another human version
approval gate.

The standard reusable flow is
`folo-rs/cargo-release-plan-action/.github/workflows/release.yml`.
Select a tested published action release and replace `ACTION_REVISION` with the
same verified immutable commit used for [release checks](github-checks.md).
Keep registry, GitHub reconciliation, platform batches and reporting in the same
workflow run. Tag and release events created using the workflow's `GITHUB_TOKEN`
do not trigger downstream workflows.

## Add the release caller

For the running example, use `.github/workflows/release.yml`:

```yaml
name: Release

on:
  push:
    branches: [main]
  workflow_dispatch:
    inputs:
      source:
        description: Optional original release-source commit for explicit recovery
        type: string
        default: ''
      verify-publishing-identity:
        description: Verify OIDC exchange and revocation without publication
        type: boolean
        default: false

permissions: {}

jobs:
  release:
    if: github.repository == 'example/widgets' && !inputs.verify-publishing-identity
    permissions:
      contents: write
      actions: read
      id-token: write
      issues: write
    uses: folo-rs/cargo-release-plan-action/.github/workflows/release.yml@ACTION_REVISION
    with:
      source: ${{ inputs.source }}

  identity:
    if: github.repository == 'example/widgets' && github.event_name == 'workflow_dispatch' && inputs.verify-publishing-identity
    permissions:
      contents: read
      actions: read
      id-token: write
    uses: folo-rs/cargo-release-plan-action/.github/workflows/identity-probe.yml@ACTION_REVISION
```

Replace `example/widgets` and the example `main` branch with the configuration
for your repository. The shared flow also validates the destination and release
branch before writes. The caller grants the scopes the flow needs; individual
jobs narrow them.

Omitted inputs use the [shared workflow defaults](../advanced/custom-jobs.md#shared-workflow-inputs).
For a protected environment, add the same setting to both jobs and register that
environment with crates.io:

```yaml
with:
  publishing-environment: release
```

For source installation, add `install-method: path` and select the tool checkout
with `source-path`. Released installation does not use `source-path`.

An empty `source` uses the invocation's commit. For explicit recovery, supply the
full original source SHA; the tool still verifies its release-branch membership.
This is a new invocation with new evidence, never an automatic fallback when an
old artifact is missing. The source override selects the release snapshot only.
The **controller** is the `cargo-release-plan` executable run by the workflow.
In source-install mode, `source-path` selects its source from the invocation
checkout, so recovering old publication source does not rebuild an old controller.

## Establish publishing identity

Every package must already exist on crates.io before automatic publication.
Complete [first publication](../operations/first-publication.md) before a new
package's first merge.

Register each package's crates.io Trusted Publisher with:

- Your GitHub repository owner and repository name.
- Your **calling entry workflow filename**, including when it calls a reusable
  workflow from the action repository.
- The publishing environment, if you use a protected GitHub environment.

The called action repository is not the publisher identity. The job exchanging
OIDC credentials must use the environment registered for that caller. See
[GitHub's reusable-workflow OIDC documentation](https://docs.github.com/en/actions/how-tos/secure-your-work/security-harden-deployments/oidc-with-reusable-workflows).

Use phase-specific permissions. Registry publication needs source read access and
`id-token: write`; GitHub tag, release and asset operations need repository
content writes; failure-issue reporting needs issue writes. Grant only the scopes
required by each job, including any read access the selected workflow needs for
artifact transport and action-revision identification.

Do not provision a registry PAT or stored-token fallback. The application acquires
short-lived upload credentials through OIDC after verification and handles their
lifecycle. Package verification shares the publication job's trust boundary,
including its job-level OIDC identity. Run only reviewed repository code and
trusted build dependencies in that job; credential handling is not a sandbox
against code running under the same account.

### Verify identity without publishing

The permanent caller above includes a mutually exclusive identity-only mode.
Dispatch it through the **same registered entry workflow filename**:

```powershell
gh workflow run release.yml --repo example/widgets --ref main `
    --field verify-publishing-identity=true
```

Run this before enabling routine publication and whenever the registered caller
or environment changes. The reusable probe checks OIDC exchange and revocation
without a Cargo workspace or publication; it has no `working-directory` or
`config` input. It uses the same action commit and environment as publication.
Do not move the probe to a differently named caller or replace the production file
to run it.

Its CLI operation is:

```powershell
cargo release-plan check-publishing-identity
```

Run this in the authorized GitHub Actions OIDC context. It exchanges and
immediately revokes a temporary credential, uploading nothing. Success exercises
the caller identity path, not every package's Trusted Publisher grant; verify
package administration separately.

## Capture the merged source

These commands demonstrate the CLI boundaries independently of workflow
filenames. Run preparation from a clean checkout of the selected merged commit:

```powershell
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
$Source = (git rev-parse HEAD).Trim()
$Work = ".release-plan-work"
$Publication = Join-Path $Work "publication.json"
$Outcomes = Join-Path $Work "outcomes"
cargo release-plan prepare-publish --source $Source `
    --config (Join-Path ".cargo" "release_plan.toml") --output $Publication
```

`$Source` must be the full commit ID. Preparation verifies HEAD, clean source,
tracked Cargo inputs, locked resolution and first-parent membership in the
configured release branch. It fetches that branch to establish eligibility; it
does not replace the selected source with the fetched tip.

Keep output in an ignored artifact directory or outside tracked source.
Preparation writes no registry package, tag or release. Repeating identical
intent may reuse the manifest; different intent needs a different destination.

In CI, prepare this manifest once and retain it for every subsequent phase and
retry. A failed-job rerun downloads original intent rather than regenerating it
from the current release-branch tip.

## Inspect registry work without uploading

```powershell
cargo release-plan publish registry --publication $Publication `
    --output (Join-Path $Work "registry-dry-run-1" "outcome.json") --dry-run
```

The dry run queries exact-version availability without exchanging publication
credentials or uploading. Its outcome is never marked complete, even if every
requested version already exists.

The following invocation performs live registry work and belongs **only in the
authorized publishing job**, not tests or the local version-planning skill:

```powershell
cargo release-plan publish registry --publication $Publication `
    --output (Join-Path $Outcomes "registry-attempt-1" "outcome.json")
```

Use a new outcome path on every attempt. Naming each live phase file
`outcome.json` inside its own artifact subdirectory lets the final reporter
discover it. Keep local dry-run and nonpublishing verification output separate
from those publication outcomes. When the workspace is nested or the source
checkout moved between jobs, `--manifest-path` must select the original
publication-source workspace. Do not substitute a prospective planning workspace
or later source commit.

The [Cargo runtime requirement](../reference/commands.md#prepare-and-publish-exact-versions)
supports verification and dependency-ordered workspace uploads. The application
verifies packaged binary dependency identities against the assessed locked closure.
Neither a failed upload command nor an old successful outcome replaces a fresh
exact-version observation.

## Continue through GitHub and native jobs

After successful registry reconciliation, inspect GitHub work without writes:

```powershell
cargo release-plan publish github --publication $Publication `
    --output (Join-Path $Work "github-dry-run-1" "outcome.json") `
    --batches (Join-Path $Work "batches-dry-run-1") --dry-run
```

A dry run is not completed delivery and cannot establish missing tags for native
builds. The authorized GitHub job performs live reconciliation:

```powershell
$Batches = Join-Path $Work "batches-attempt-1"
$GithubOutcome = Join-Path $Outcomes "github-attempt-1" "outcome.json"
cargo release-plan publish github --publication $Publication `
    --output $GithubOutcome --batches $Batches
```

The outcome supplies relative paths and `batch_id` values for frozen platform
batches. Both the outcome path and batch directory must be new. For a nested
workspace, pass `--manifest-path` for the original publication source, just as
in registry publication.

Do not derive binary work from "packages uploaded in this attempt." A registry
no-op can still require a missing tag, release or checksum.

A failure to tag one release does not suppress valid work for another. Valid
manifest-linked batches can continue after registry prerequisites succeed, while
the reconciliation failure remains a workflow failure. Missing batch artifacts,
invalid identities or cancellation do not authorize downstream work.

On the matching native runner, select the emitted batch for its target. This
example assumes the outcome includes Windows x64 work:

Choose an absolute shared Cargo build-cache location outside the clean source,
or beneath an ignored directory. This preserves source cleanliness in repositories
that do not ignore Cargo's default `target` directory:

```powershell
$env:CARGO_TARGET_DIR = [IO.Path]::GetFullPath((Join-Path $Work "build-cache"))
```

```powershell
$Routing = Get-Content $GithubOutcome -Raw | ConvertFrom-Json
$Selected = $Routing.batches | Where-Object target -eq "x86_64-pc-windows-msvc"
if ($null -eq $Selected) {
    throw "This outcome contains no Windows x64 batch."
}
$Batch = Join-Path $Batches $Selected.path
cargo release-plan publish binaries --publication $Publication --batch $Batch `
    --output (Join-Path $Outcomes "windows-attempt-1" "outcome.json") `
    --artifacts (Join-Path $Work "windows-artifacts-attempt-1")
```

The tool rechecks tag identities and assets, then builds each executable
separately from its actual peeled tag commit. To stage the frozen batch without
querying or writing GitHub, use `--no-upload` with separate destinations:

```powershell
cargo release-plan publish binaries --publication $Publication --batch $Batch `
    --output (Join-Path $Work "windows-verification-1" "outcome.json") `
    --artifacts (Join-Path $Work "windows-verification-artifacts-1") --no-upload
```

This mode still verifies source and archives. It does not discover a batch,
select substitute source or produce a completed publication outcome.

## Report every attempt

The final reporting job downloads each phase's artifact subdirectory, retaining
its `outcome.json`, and supplies current job results as `jobs.json`. The fixed
keys are `prepare`, `registry`, `github` and `binaries`; values come from GitHub's
job results, including the binary matrix's aggregate result:

```json
{
  "prepare": "success",
  "registry": "success",
  "github": "failure",
  "binaries": "skipped"
}
```

This is a format example, not values to hardcode into a real report. Run the
reporter even after failed phases:

```powershell
cargo release-plan publish report --repository example/widgets `
    --publication $Publication --outcomes $Outcomes `
    --jobs (Join-Path $Work "jobs.json") `
    --output (Join-Path $Work "publication-report-attempt-1.md")
```

The command requires the GitHub run context, writes the Markdown report even for
incomplete delivery, and exits nonzero when work remains. It creates or updates
the run-qualified failure issue with exact operator recovery instructions.
`--no-issue` disables issue writes but does not remove the run-context
requirement or turn an incomplete report into success.

If the publication manifest is unavailable, omit `--publication`. The reporter
still writes a report and failure issue; missing intent is always incomplete,
never a successful empty release.

Outcomes carry optional `github: {run_id, run_attempt}` metadata when produced in
GitHub Actions. Preserve it so the reporter can enforce
[outcome identity and freshness](../reference/artifacts.md#derived-batches-and-later-outcomes).

## Prepare native build environments

Configure the subset your packages support from the
[supported native targets](../reference/configuration.md#workspace-publication-configuration).
The action revision owns the corresponding runner mapping.

Provide native libraries or other source-build prerequisites through the fixed
optional `.github/actions/release-plan-setup/action.yml`. It prepares the
environment in Cargo verification and binary-build jobs, not GitHub-only
reconciliation jobs, and must not edit captured source.

The action's controller installation, the invocation checkout and each tagged
binary source are distinct. A historical tag need not contain the current
controller or current publication configuration.

## Queue and retain, without hiding failures

Serialize publication runs for the same repository, release branch and workspace
with non-cancelling queued concurrency (`queue: max`). Disable matrix fail-fast
so independent native targets can finish. Queue capacity and platform
cancellation still need operational attention.

Choose artifact retention long enough for operator recovery. Preserve the
manifest, frozen batches, every attempt outcome and diagnostics, including on
failure. Use run/attempt-qualified artifact names from the selected action
interface rather than overwriting intent.

Queuing publication does not prevent a newer version from merging. For
superseded-version tag failures, follow the
[original-source manual-tag recovery](../operations/recovery.md#missing-tag-after-a-newer-version-merges).

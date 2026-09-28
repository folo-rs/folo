# Adopt local version planning

Local planning is preparation for review, not permission to merge or publish.
The same workflow works when performed by a maintainer or guided by an agent.

## Prerequisites

Use compatible tool and skill schemas: report/plan/prepared `5`, semantic
decisions and compatibility `1`, and release context `2`. Read the installed
values with `cargo release-plan version`. Git, the selected Cargo/Rust toolchain
and the API compatibility checker (`cargo-semver-checks`) are prerequisites.
Private release repositories also need GitHub
CLI authentication. Installation or upgrades follow your authorization policy.

Tracked files may have staged or unstaged edits. No prior commit or clean checkout
is required for planning. Track newly created release inputs before assessment;
publication separately requires clean committed source.

For direct CLI planning, continue with [Select history and prepare](#select-history-and-prepare).

## Optional agent integration

Copy the complete `.github/skills/increment-versions` directory from a known,
immutable source revision matching your selected tool into the same location
in your repository. Obtain the directory from the
[source repository](https://github.com/folo-rs/folo) at that revision, not a
floating branch. You do not need the rest of the repository or its scripts.
Include its command reference, decision guide and license, not only `SKILL.md`.

Record the source revision and tool version in your repository's adoption notes.
Read the copied skill's prerequisites and compare its supported schemas with
`cargo release-plan version` before allowing it to edit files. Matching package
versions are unnecessary when the schemas agree. On mismatch, update the skill
from the canonical directory; inside the canonical repository, report the mismatch
instead of overwriting work. Unexpected CLI errors can warrant upgrading tool and
skill together.

The copied skill does not grant permission to install tools.

Your repository's agent instructions should state its release branch, required
checks and the expectation to run the skill when released content changes.
The skill makes semantic decisions, resolves effects and applies the complete
result without a separate version-choice approval gate. Human review of the
complete PR remains approval of the contribution.

## Select history and prepare

The following PowerShell commands run at the selected workspace root on a named
feature branch. Native command failures should stop the session. Configuration
selects the actual repository and release branch, without assuming a local
remote nickname or a branch named `main`:

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
$EvidenceRoot = ".release-plan-work"
$Configuration = Join-Path ".cargo" "release_plan.toml"
$Branch = (git symbolic-ref --quiet --short HEAD).Trim()
New-Item -ItemType Directory -Path $EvidenceRoot -Force | Out-Null
git check-ignore --quiet -- $EvidenceRoot
$Work = Join-Path $EvidenceRoot ([guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $Work | Out-Null
$ContextPath = Join-Path $Work "context.json"
$TargetRef = "" # For a stacked PR, set this to its parent branch/ref.
$TargetArguments = @()
if ($TargetRef) { $TargetArguments = @("--merge-target", $TargetRef) }
cargo release-plan release-context --config $Configuration @TargetArguments > $ContextPath
$Context = Get-Content $ContextPath -Raw | ConvertFrom-Json
if ($Branch -eq $Context.release_branch) {
    throw "Version planning requires a feature branch, not the release branch."
}
$HistoryCommit = $Context.release_history
$AssessmentArguments = @("--release-history", $HistoryCommit)
if ($Context.merge_target) {
    $AssessmentArguments += @("--merge-target", $Context.merge_target)
}
try {
    $PSNativeCommandUseErrorActionPreference = $false
    cargo release-plan check-published
    if ($LASTEXITCODE -ne 0) { Write-Warning "Retain the registry discovery diagnostics for assessment." }
} finally {
    $PSNativeCommandUseErrorActionPreference = $true
}
$Prepared = Join-Path $Work "prepared"
cargo release-plan prepare @AssessmentArguments --output $Prepared
cargo release-plan check-compatibility `
    --prepared (Join-Path $Prepared "prepared.json") `
    --output (Join-Path $Work "compatibility")
```

The evidence root must already be covered by an ignore rule; an external root is
also suitable and does not need `git check-ignore`. Each assessment gets a new
child directory while retaining earlier evidence. Preparation produces a
consistent workspace lockfile and evidence describing that same source.
It can modify `Cargo.lock`; inspect that change with the contribution.

`release-context` fetches the configured release branch and records `release_history`
and the optional normalized `merge_target`. It also reports source HEAD, input locations and a workspace-scoped
concurrency identity; it does not require clean source or prepare publication.
Detached HEAD is not a valid feature-branch planning state.

The workspace-wide `check-published` call is advisory. Record every missing or
unknown package and continue assessment, arranging the
[maintainer bootstrap handoff](../operations/first-publication.md) before the
fail-closed resolved-plan gate.

## Assess and propose

```powershell
cargo release-plan analysis-order --report $Prepared
cargo release-plan semver-targets --report $Prepared
```

Read the report, patches, inherited changes and locked dependencies in
dependency-first order. Read `compatibility/compatibility.json` and
`semver-checks.log`; require `completed: true`. Each comparison's `required_level`
is a semantic floor, not a numeric increment. `compared: false` means no
comparison, not compatibility.

`check-compatibility` regenerates a bound, read-only report from the prepared
source and verifies that source before and after the checker. It does not accept
a detached `--report` artifact. A checker execution failure is not a pass; an
empty contract selection requires neither checker execution nor a registry query.
The wider semantic assessment still covers behavioral and feature-subset promises
and can require a stronger decision than the checker's floor.

For the running example, save this literal decisions document as
`.release-plan-work\decisions.json`:

```json
{
  "schema_version": 1,
  "changes": [
    { "name": "widget", "level": "nonbreaking" },
    { "name": "widget_impl", "level": "patch" },
    { "name": "widget-cli", "level": "patch" }
  ]
}
```

These names and judgments are examples, not a default policy. Omit packages
requiring no semantic increment. Non-publishable version targets receive no
decision.

```powershell
$Decisions = Join-Path $Work "decisions.json"
$Proposal = Join-Path $Work "proposal.json"
$Preview = Join-Path $Work "preview"
cargo release-plan propose --report $Prepared --decisions $Decisions --out $Proposal
cargo release-plan preview --prepared (Join-Path $Prepared "prepared.json") `
    --plan $Proposal --output $Preview
```

## Inspect the prospective release

Read the preview's `report.json`, diffs and complete `plan.json`. Assess new
dependent or binary lockfile effects and revise semantic decisions if necessary,
then propose and preview again into fresh destinations.

Inspect the final artifact:

```powershell
$Plan = Join-Path $Preview "plan.json"
$Inspection = cargo release-plan inspect-plan --plan $Plan --require-resolved |
    ConvertFrom-Json
$Inspection.publication_targets
$Inspection.evidence_manifest_path
```

Check the final release against its retained prospective workspace, including
its manifest versions and lockfile:

```powershell
cargo release-plan check-compatibility --plan $Plan `
    --output (Join-Path $Work "preview-compatibility")
cargo release-plan verify-preview --plan $Plan `
    --manifest-path $Inspection.evidence_manifest_path
```

The compatibility command verifies captured inputs itself. `verify-preview`
also supplies a read-only guard after any additional external analysis you run.
Read the completed comparisons and their floors before applying. Findings are
planning evidence by default; `--deny-findings` is useful for a gate that must
reject insufficient increments.

## Apply and verify

Refresh the release context and stop for
[reassessment](../operations/recovery.md#release-branch-movement) if its history or
merge target moved. The plan-scoped registry check is fail-closed, unlike workspace discovery:

```powershell
$RefreshedPath = Join-Path $Work "refreshed-context.json"
cargo release-plan release-context --config $Configuration @TargetArguments > $RefreshedPath
$Refreshed = Get-Content $RefreshedPath -Raw | ConvertFrom-Json
if ($Refreshed.release_history -cne $HistoryCommit -or
    $Refreshed.merge_target -cne $Context.merge_target) {
    throw "Release history or the merge target moved; refresh the assessment."
}
cargo release-plan check-published --plan $Plan
```

Every publishable target must already be established on crates.io; missing or
unknown registry state blocks application. Non-publishable version targets require no
query. This does not verify Trusted Publisher administration, which the
maintainer completes separately.

Preserve enough before/after evidence to identify generated edits, then apply
the captured result and verify it with fresh evidence:

```powershell
cargo release-plan apply --plan $Plan --dry-run
cargo release-plan apply --plan $Plan
cargo metadata --locked --format-version 1 > $null
cargo release-plan check-compatibility @AssessmentArguments `
    --output (Join-Path $Work "after") --deny-findings
cargo release-plan check @AssessmentArguments --config $Configuration
```

The fresh compatibility output includes its read-only report; inspect completed
comparisons and floors as well as the command result. Original preparation and
preview evidence remain intact.

Run the repository's affected build and test checks as well.
Separate fixture workspaces have their own lockfiles; a root-workspace check
does not validate them.

Apply uses captured files without a late resolution step. If inputs changed,
regenerate evidence instead of editing the plan's captured state.

Present the complete release in the PR's
[Version/release plan section](../operations/ordinary-release.md#present-the-complete-release).
Assess all changes covered by existing pending increments, retaining each when
sufficient rather than counting only packages changed by the latest tool invocation. Use the union of resolved-plan
targets and all pending-release entries in the final report.

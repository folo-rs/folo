# Adopt local version planning

Local planning is preparation for review, not permission to merge or publish.
The same workflow works when performed by a maintainer or guided by an agent.

## Copy the matching skill

Copy the complete `.github/skills/increment-versions` directory from a known,
immutable source revision matching your selected tool into the same location
in your repository. Obtain the directory from the
[source repository](https://github.com/folo-rs/folo) at that revision, not a
floating branch. You do not need the rest of the repository or its scripts.
Include its decision guide and license, not only `SKILL.md`.

Record the source revision and tool version in your repository's adoption notes.
Read the copied skill's prerequisites and compare its command interface with
`cargo release-plan --version` and `--help` **before allowing it to edit files**.
Selecting a newer skill does not upgrade an older executable.

The matching skill supports `cargo-release-plan` `0.5.0`, report/plan schema `4`
and semantic-decision schema `1`. Git, the selected Cargo/Rust toolchain and
`cargo-semver-checks` are prerequisites. Private release repositories also need
GitHub CLI authentication. Installation or upgrades follow your repository's
authorization policy; the skill does not grant permission to install tools.

Your repository's agent instructions should state its release branch, required
checks and the expectation to run the skill when released content changes.
The skill makes semantic decisions, resolves effects and applies the complete
result without a separate version-choice approval gate. Human review of the
complete PR remains approval of the contribution.

## Freeze history and prepare

The following PowerShell commands run at the selected workspace root on a named
feature branch. Native command failures should stop the session. Configuration
selects the actual repository and release branch, without assuming a local
remote nickname or a branch named `main`:

```powershell
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
$Work = ".release-plan-work"
$Configuration = Join-Path ".cargo" "release_plan.toml"
$Branch = (git symbolic-ref --quiet --short HEAD).Trim()
New-Item -ItemType Directory -Path $Work | Out-Null
$ContextPath = Join-Path $Work "context.json"
cargo release-plan release-context --config $Configuration > $ContextPath
$Context = Get-Content $ContextPath -Raw | ConvertFrom-Json
if ($Branch -eq $Context.release_branch) {
    throw "Version planning requires a feature branch, not the release branch."
}
$Baseline = $Context.release_base
cargo release-plan check-published
$Prepared = Join-Path $Work "prepared"
cargo release-plan prepare --base $Baseline --output $Prepared
cargo release-plan check-compatibility `
    --prepared (Join-Path $Prepared "prepared.json") `
    --output (Join-Path $Work "compatibility")
```

Use a fresh output location for a new assessment. Preparation performs the
intended offline workspace dependency refresh before capturing evidence. It can
modify `Cargo.lock`; it does not request blanket third-party upgrades. Missing
offline dependencies are a setup problem to resolve explicitly, not permission
to replace assessment with an uncontrolled online update.

`release-context` fetches the configured release branch and captures its immutable
`release_base`. It also reports source HEAD, input locations and a workspace-scoped
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
The author still assesses behavioral and feature-subset promises.

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
requiring no semantic increment. Nonpublishable alignment helpers receive no
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
dependent or binary lockfile effects and raise semantic levels if necessary,
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
[reassessment](../operations/recovery.md#release-branch-movement) if its baseline
moved. The plan-scoped registry check is fail-closed, unlike workspace discovery:

```powershell
$RefreshedPath = Join-Path $Work "refreshed-context.json"
cargo release-plan release-context --config $Configuration > $RefreshedPath
$Refreshed = Get-Content $RefreshedPath -Raw | ConvertFrom-Json
if ($Refreshed.release_base -ne $Baseline) {
    throw "The release baseline moved; refresh the assessment before applying."
}
cargo release-plan check-published --plan $Plan
```

Every publishable target must already be established on crates.io; missing or
unknown registry state blocks application. Alignment-only helpers require no
query. This does not verify Trusted Publisher administration, which the
maintainer completes separately.

Preserve enough before/after evidence to identify generated edits, then apply
the captured result and verify it with fresh evidence:

```powershell
cargo release-plan apply --plan $Plan --dry-run
cargo release-plan apply --plan $Plan
cargo metadata --locked --format-version 1 > $null
cargo release-plan check-compatibility --base $Baseline `
    --output (Join-Path $Work "after") --deny-findings
cargo release-plan check --base $Baseline --config $Configuration
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
Include pending increments already on the branch, not merely the packages
changed by the most recent tool invocation. Use the union of resolved-plan
targets and all pending-release entries in the final report.

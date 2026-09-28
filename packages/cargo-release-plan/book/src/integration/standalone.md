# One-off version increments

Use the `increment-versions` skill in **standalone mode** when you want a
version-increment PR without adopting the toolkit's publishing workflows.
The user supplies the planning inputs instead of `release_plan.toml`.
No publishing configuration, GitHub remote or workflow installation is required.

The outcome is a reviewed version-increment PR merged under the repository's
normal policy. The skill does not grant merge authority, and this mode does not
continue into publication. The shared workflow remains the toolkit's canonical
supported publishing path.

Request this mode explicitly, rather than using it to bypass a failed configured
check. Existing repository CI and merge-triggered workflows still apply; choosing
standalone planning does not disable automation already installed in the repository.

## Supply the planning inputs

Tell the agent:

| Input | Purpose |
| --- | --- |
| Cargo workspace | The `Cargo.toml` to assess. |
| Release-history ref | A local branch, tag or commit identifying the existing release history. |
| Parent target ref, when applicable | The final unmerged parent snapshot for a stacked PR. |

For example:

> Run increment-versions in standalone mode for this workspace. Use the local
> `origin/main` ref as release history and no unmerged parent. Prepare a PR with
> the version changes; do not configure or run publication.

The ref names are your choices, not conventions inferred by the skill. A local
repository can instead use a local branch or commit with no remote at all.
Refresh remote-tracking refs beforehand when you want current remote history.
The tool still follows the [history and anchor model](../concepts/history.md);
standalone does not mean comparing arbitrary unrelated trees.

Git, Cargo/Rust, the release-plan tool and `cargo-semver-checks` remain prerequisites.
The skill checks supported schemas with `cargo release-plan version`. Tracked files
may have staged or unstaged edits; track newly created intended release inputs
before collecting evidence.

## Use the existing planning commands

Standalone mode is a skill operating scenario, not another tool configuration
format. The existing explicit history/target options supply everything planning
needs. No dummy publishing config is created.

This example runs in the selected workspace on a feature branch:

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
$Manifest = (Resolve-Path "Cargo.toml").Path
$HistoryRef = "origin/main" # Replace with the history ref you selected.
$HistoryCommit = (git rev-parse --verify --end-of-options "$HistoryRef^{commit}").Trim()
$Work = Join-Path ([IO.Path]::GetTempPath()) ("version-plan-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $Work | Out-Null
$AssessmentArguments = @("--release-history", $HistoryCommit)

# For a stacked PR, resolve its selected parent ref and add --merge-target <commit>.
cargo release-plan prepare --manifest-path $Manifest @AssessmentArguments --output $Work
cargo release-plan check-compatibility --manifest-path $Manifest `
    --prepared (Join-Path $Work "prepared.json") `
    --output (Join-Path $Work "compatibility")
```

Keep the original refs and resolved commits in the handoff so you can detect
movement before applying. A repository-local evidence directory is also suitable
when an existing ignore rule covers it.

Continue through [semantic assessment and preview](local-planning.md#assess-and-propose):
read the complete report, choose decisions, propose the plan and preview all
group and dependency effects. These operations are identical in either mode.
Compatibility comparison can still read published crates independently of any
publishing workflow; unavailable comparison evidence is not proof of compatibility.

## Apply without publishing prerequisites

Before applying, resolve the original history and parent refs again. If either
moved, prepare fresh evidence rather than reusing the old plan.

Standalone mode omits the configured workflow's `check-published` discovery and
registry gate, Trusted Publisher/bootstrap handoff and `check --config`.
A crate need not already exist on crates.io merely to edit its version. This does
not relax source verification, semantic assessment, dependency/group expansion
or captured-plan application.

After reviewing the resolved plan, use the ordinary application and verification
commands, with `$Plan` pointing to the preview's generated `plan.json`:

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
$Plan = Join-Path $Work "preview\plan.json"
cargo release-plan apply --manifest-path $Manifest --plan $Plan
cargo metadata --manifest-path $Manifest --locked --format-version 1 > $null
cargo release-plan check-compatibility --manifest-path $Manifest @AssessmentArguments `
    --output (Join-Path $Work "verified")
cargo release-plan check --manifest-path $Manifest @AssessmentArguments
```

Inspect the fresh report and completed comparison evidence. Explain all proposed
versions and dependent/group movements in the PR, review the source and version
changes together, and merge only with the repository's normal authorization.
**That merge is the end of the standalone workflow.**

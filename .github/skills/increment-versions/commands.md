# Command reference

Execute these examples in PowerShell, in stage order. Replace the placeholders
before execution. Each block stops on failed native commands except the explicitly
advisory registry discovery. Retain the original diagnostic and inspect partial
writes before retrying a failed stage.

| Placeholder | Meaning |
| --- | --- |
| `TOOL` | Installed `cargo-release-plan` executable or the selected tested source-built executable. |
| `MANIFEST` | Absolute path to the selected workspace's `Cargo.toml`. |
| `CONFIG` | Publication configuration path; relative paths start at the selected workspace. |
| `WORK_DIR` | New absolute ignored or external directory for assessment evidence. |
| `VERIFY_DIR` | Separate new absolute ignored or external directory for final verification; do not create it manually. |
| `MERGE_TARGET_REF` | Unmerged parent PR's actual target ref, or an empty string for release-branch assessment. |
| `HISTORY_COMMIT` | Full `release_history` commit from the initial context. |
| `TARGET_COMMIT` | Full `merge_target` commit from context, or an empty string when null. |
| `PACKAGE` | Package name from a report entry. |
| `DIFF_PATH` | That entry's `diff_path`: the generated patch filename relative to the directory containing `report.json`; absent when there is no file patch. |

For repository-local evidence directories, run this for each path before writing:

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
git check-ignore --quiet -- "{{WORK_DIR}}"
git check-ignore --quiet -- "{{VERIFY_DIR}}"
```

A nonzero result does not establish an ignored path. Choose external directories
or an existing ignored location instead. Do not run these checks for external paths.

## Stage 1: Check prerequisites and select history

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
$Identity = & "{{TOOL}}" version | ConvertFrom-Json
$Expected = @{ plan = 5; report = 5; prepared = 5; decisions = 1; compatibility = 1; release_context = 2 }
foreach ($Name in $Expected.Keys) {
    if ($Identity.schemas.$Name -ne $Expected[$Name]) {
        throw "Unsupported $Name schema; follow the skill upgrade instructions."
    }
}
Get-Command cargo-semver-checks -ErrorAction Stop | Out-Null
$Branch = (git symbolic-ref --quiet --short HEAD).Trim()
New-Item -ItemType Directory -Path "{{WORK_DIR}}" | Out-Null
$TargetArguments = @()
if (-not [string]::IsNullOrWhiteSpace("{{MERGE_TARGET_REF}}")) {
    $TargetArguments = @("--merge-target", "{{MERGE_TARGET_REF}}")
}
& "{{TOOL}}" release-context --manifest-path "{{MANIFEST}}" --config "{{CONFIG}}" `
    @TargetArguments --verbose > "{{WORK_DIR}}\context.json"
$Context = Get-Content -LiteralPath "{{WORK_DIR}}\context.json" -Raw | ConvertFrom-Json
if ($Branch -eq $Context.release_branch) { throw "Run version planning on a feature branch." }
```

The version query requires no repository and reports schema revisions, not a
permission to ignore later command failures. Use context's full history/target
commit IDs as `HISTORY_COMMIT` and `TARGET_COMMIT`. Record the original target ref
for Stage 6.

Advisory registry discovery uses its own explicit error handling:

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $false
& "{{TOOL}}" check-published --manifest-path "{{MANIFEST}}" --verbose
if ($LASTEXITCODE -ne 0) {
    Write-Warning "Registry discovery was incomplete. Retain its diagnostics and assess the reported packages."
}
```

## Stage 2: Prepare complete evidence

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
$TargetArguments = @()
if (-not [string]::IsNullOrWhiteSpace("{{TARGET_COMMIT}}")) {
    $TargetArguments = @("--merge-target", "{{TARGET_COMMIT}}")
}
& "{{TOOL}}" prepare --manifest-path "{{MANIFEST}}" --release-history "{{HISTORY_COMMIT}}" `
    @TargetArguments --output "{{WORK_DIR}}" --verbose
& "{{TOOL}}" check-compatibility --manifest-path "{{MANIFEST}}" `
    --prepared "{{WORK_DIR}}\prepared.json" --output "{{WORK_DIR}}\compatibility" --verbose
```

Read `report.json`, its `diffs` patches, `prepared.json`, and the compatibility
evidence in `WORK_DIR`. A consistent lockfile may be installed during preparation;
the report describes that same source. Compatibility checks require completed
evidence before semantic assessment.

## Stage 3: Assess in dependency order

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
& "{{TOOL}}" analysis-order --report "{{WORK_DIR}}\report.json" --verbose `
    > "{{WORK_DIR}}\analysis-order.json"
```

Use the returned ordered batches for the assessment. Resolve a package's patch
with `Join-Path "{{WORK_DIR}}" "{{DIFF_PATH}}"` when it has `diff_path`.

## Stage 4: Choose semantic decisions

Write the assessed decisions to `WORK_DIR\decisions.json`, for example:

```json
{
  "schema_version": 1,
  "changes": [
    { "name": "example-api", "level": "nonbreaking" },
    { "name": "example-tool", "level": "patch" }
  ]
}
```

Names and levels are scenario values, not defaults. Use the decision guide and
report evidence; keep the explanation alongside this agent-authored document.

## Stage 5: Review resolved effects

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
& "{{TOOL}}" propose --report "{{WORK_DIR}}\report.json" `
    --decisions "{{WORK_DIR}}\decisions.json" --out "{{WORK_DIR}}\plan.json" --verbose
& "{{TOOL}}" preview --manifest-path "{{MANIFEST}}" --prepared "{{WORK_DIR}}\prepared.json" `
    --plan "{{WORK_DIR}}\plan.json" --output "{{WORK_DIR}}\preview" --verbose
& "{{TOOL}}" check-compatibility --manifest-path "{{MANIFEST}}" `
    --plan "{{WORK_DIR}}\preview\plan.json" --output "{{WORK_DIR}}\preview-compatibility" --verbose
```

Read the complete `preview\report.json`, patches, resolved `preview\plan.json`
and completed preview compatibility evidence. Repeating this stage requires new
preview and compatibility output directories; do not overwrite retained evidence.

## Stage 6: Refresh and apply

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
$TargetArguments = @()
if (-not [string]::IsNullOrWhiteSpace("{{MERGE_TARGET_REF}}")) {
    $TargetArguments = @("--merge-target", "{{MERGE_TARGET_REF}}")
}
& "{{TOOL}}" release-context --manifest-path "{{MANIFEST}}" --config "{{CONFIG}}" `
    @TargetArguments --verbose > "{{WORK_DIR}}\refreshed-context.json"
$Original = Get-Content -LiteralPath "{{WORK_DIR}}\context.json" -Raw | ConvertFrom-Json
$Current = Get-Content -LiteralPath "{{WORK_DIR}}\refreshed-context.json" -Raw | ConvertFrom-Json
if ($Original.release_history -cne $Current.release_history -or
    $Original.merge_target -cne $Current.merge_target) {
    throw "Release history or merge target moved; restart assessment using the recovery instructions."
}
& "{{TOOL}}" check-published --manifest-path "{{MANIFEST}}" `
    --plan "{{WORK_DIR}}\preview\plan.json" --verbose
& "{{TOOL}}" apply --manifest-path "{{MANIFEST}}" --plan "{{WORK_DIR}}\preview\plan.json" --verbose
```

Preserve pre-application file contents before the `apply` command. Only a successful
plan-scoped registry check permits application; missing or unknown identities are
explicit first-publication or access blockers.

## Stage 7: Verify and hand off

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
$TargetArguments = @()
if (-not [string]::IsNullOrWhiteSpace("{{TARGET_COMMIT}}")) {
    $TargetArguments = @("--merge-target", "{{TARGET_COMMIT}}")
}
cargo metadata --manifest-path "{{MANIFEST}}" --locked --format-version 1 > $null
& "{{TOOL}}" check-compatibility --manifest-path "{{MANIFEST}}" `
    --release-history "{{HISTORY_COMMIT}}" @TargetArguments --output "{{VERIFY_DIR}}" --verbose
& "{{TOOL}}" check --manifest-path "{{MANIFEST}}" --release-history "{{HISTORY_COMMIT}}" `
    @TargetArguments --config "{{CONFIG}}" --format github --verbose
```

Read `VERIFY_DIR\report.json` and completed compatibility evidence, verify all
decisions and resolved targets, and confirm that intended release inputs are
tracked. The metadata exit status verifies the lockfile; its JSON is not a second
assessment report. Keep the reviewed release plan and handoff current.

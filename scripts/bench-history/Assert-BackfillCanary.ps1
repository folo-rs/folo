#Requires -Version 7.6

# The hosted caller canary uses this verifier after shared backfill completes.
# It queries actual stored commits rather than treating older aggregate counts as
# evidence for the current range. The independent listing is diagnostic test output,
# not an analysis/report stage of the public backfill workflow.
# Ref: .github/workflows/implementation.md#reusable-workflow-canary.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string] $Workspace,
    [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string] $From,
    [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string] $To,
    [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string] $OutputPath,
    [ValidateNotNullOrEmpty()][string] $Config = '.cargo/backfill_history.toml',
    [ValidateNotNullOrEmpty()][string] $Project = 'reusable-backfill-canary',
    [ValidateNotNullOrEmpty()][string[]] $TargetTriple = @(
        'x86_64-unknown-linux-gnu', 'x86_64-pc-windows-msvc', 'aarch64-apple-darwin'
    )
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'

$fixture = Join-Path -Path $Workspace -ChildPath '.github' -AdditionalChildPath 'fixtures', 'bench-history-caller'
$configPath = Join-Path $fixture $Config
$manifest = Join-Path $Workspace 'Cargo.toml'
cargo run --manifest-path $manifest --locked --package cargo-bench-history --bin cargo-bench-history -- `
    list runs --repo $fixture --config $configPath --context $To --base $To `
    --engine criterion --target-triple all --machine-key all --no-dirty --no-text --json $OutputPath
$report = Get-Content -LiteralPath $OutputPath -Raw | ConvertFrom-Json
if ($report.project -cne $Project) {
    throw 'Backfill verification did not read the expected synthetic project.'
}

# Each supported target needs both current endpoints in one comparable hardware
# partition. Existing matching data is valid resumption; another range or target is not.
$completeTargets = @(foreach ($set in $report.sets) {
        if ($set.engine -cne 'criterion' -or $set.series -le 0) { continue }
        $complete = $true
        foreach ($endpoint in @($From, $To)) {
            $matching = @($set.commits | Where-Object {
                    $_.commit -ceq $endpoint -and $_.clean -gt 0 -and $_.dirty -eq 0 -and $_.runs -gt 0
                })
            if ($matching.Count -eq 0) { $complete = $false; break }
        }
        if ($complete) { $set.target_triple }
    }) | Sort-Object -Unique
if (($completeTargets -join ',') -cne (($TargetTriple | Sort-Object -Unique) -join ',')) {
    throw 'Backfill did not store both current endpoint commits for every target.'
}

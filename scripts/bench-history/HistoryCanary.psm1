#Requires -Version 7.6

# The caller canary verifies workflow outputs and downloaded reports without preparing Rust
# in its artifact-only verification job. These assertions use the runner's PowerShell.
# Ref: .github/workflows/implementation.md#reusable-workflow-canary.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

function Assert-HistoryCanaryOutput {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][AllowEmptyString()][string] $Outcome,
        [Parameter(Mandatory)][AllowEmptyString()][string] $State,
        [Parameter(Mandatory)][AllowEmptyString()][string] $Notable,
        [Parameter(Mandatory)][AllowEmptyString()][string] $Regressions,
        [Parameter(Mandatory)][AllowEmptyString()][string] $PartialPlatformCoverage,
        [Parameter(Mandatory)][AllowEmptyString()][string] $ArtifactId,
        [Parameter(Mandatory)][AllowEmptyString()][string] $ArtifactUrl
    )

    if ($Outcome -cnotin @('clean', 'insufficient_baseline', 'partial')) {
        throw "Unexpected synthetic analysis outcome '$Outcome'."
    }
    if ($State -cnotin @('clean', 'inconclusive') -or
        $Notable -cne 'false' -or $Regressions -cne '0' -or $PartialPlatformCoverage -cne 'false') {
        throw 'The deterministic matrix did not produce a complete, non-regressing report.'
    }
    if ($ArtifactId -notmatch '^[1-9][0-9]*$' -or [string]::IsNullOrWhiteSpace($ArtifactUrl)) {
        throw 'The reusable workflow did not expose its uploaded report artifact.'
    }
}

function Assert-HistoryCanaryReport {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string] $ReportDirectory,
        [Parameter(Mandatory)][ValidateSet('clean', 'insufficient_baseline', 'partial')][string] $ExpectedOutcome
    )

    foreach ($name in @('report.md', 'report.json', 'summary.md')) {
        $content = Get-Content -LiteralPath (Join-Path $ReportDirectory $name) -Raw
        if ([string]::IsNullOrWhiteSpace($content)) { throw "Empty report file '$name'." }
    }
    $report = Get-Content -LiteralPath (Join-Path $ReportDirectory 'report.json') -Raw | ConvertFrom-Json
    if ($report.mode -cne 'history' -or $report.outcome -cne $ExpectedOutcome -or
        $report.notable -ne $false -or $report.regressions -ne 0) {
        throw 'The downloaded report does not match the non-regressing reusable workflow outputs.'
    }

    $expectedTargets = @('x86_64-unknown-linux-gnu', 'x86_64-pc-windows-msvc', 'aarch64-apple-darwin')
    $sets = @($report.sets)
    $actualTargets = @($sets | ForEach-Object { $_.target_triple } | Sort-Object -Unique)
    if (($actualTargets -join ',') -cne (($expectedTargets | Sort-Object) -join ',')) {
        throw 'The report did not contain the synthetic measurements from every supported target.'
    }

    # Machine-key selection can include several partitions per target. The fixture contributes
    # one Criterion series per partition, not per distinct target. Ghosts are outside in_scope.
    # Ref: .github/workflows/implementation.md#reusable-workflow-canary.
    $partitions = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($set in $sets) {
        if ($set.engine -cne 'criterion' -or $set.series -ne 1 -or $set.runs -le 0 -or
            $set.regressions -ne 0 -or [string]::IsNullOrWhiteSpace($set.machine_key) -or
            -not $partitions.Add("$($set.target_triple)/$($set.machine_key)")) {
            throw 'The report contains an invalid synthetic measurement partition.'
        }
    }
    if ($report.census.in_scope -ne $sets.Count -or $report.series -ne $sets.Count) {
        throw 'The report series totals do not match its synthetic measurement partitions.'
    }
}

Export-ModuleMember -Function Assert-HistoryCanaryOutput, Assert-HistoryCanaryReport

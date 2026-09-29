#requires -Version 7.6

# Thin local/queue boundary. The CLI resolves history and an anticipated merge target,
# then owns the configured local check or narrow queue verdict. Standard uses the shared action.
# Ref: docs/release-versioning.md and .github/workflows/implementation.md#release-validation.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

function Invoke-ReleasePlanTool {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string[]] $Command,
        [Parameter(Mandatory)][scriptblock] $Tool,
        [string] $History,
        [string] $MergeTarget
    )

    $argument = $Command
    if (-not [string]::IsNullOrWhiteSpace($History)) {
        $argument += @('--release-history', $History)
    }
    if (-not [string]::IsNullOrWhiteSpace($MergeTarget)) {
        $argument += @('--merge-target', $MergeTarget)
    }
    $output = & $Tool $argument
    if ($LASTEXITCODE -ne 0) {
        throw "cargo-release-plan $($Command[0]) failed with exit code $LASTEXITCODE."
    }
    return $output
}

function Invoke-ReleaseValidation {
    [CmdletBinding()]
    param(
        [string] $History = $env:RELEASE_PLAN_HISTORY,
        [string] $MergeTarget = $env:RELEASE_PLAN_MERGE_TARGET,
        [switch] $VersionReadinessOnly,
        [scriptblock] $Tool = {
            param([string[]] $Argument)
            & cargo run --quiet --locked -p cargo-release-plan -- @Argument
        }
    )

    if ([string]::IsNullOrWhiteSpace($History) -and
        [string]::IsNullOrWhiteSpace($MergeTarget) -and $env:GITHUB_ACTIONS -cne 'true') {
        # Folo's ordinary local readiness check remains offline. Explicit targets use the
        # CLI's configured-history acquisition; a missing cached local ref is an error.
        $History = 'origin/main'
    }
    $json = Invoke-ReleasePlanTool -Command @('release-context', '--config', '.cargo/release_plan.toml') `
        -History $History -MergeTarget $MergeTarget -Tool $Tool
    $context = ConvertFrom-Json -InputObject ($json -join "`n") -NoEnumerate
    if ($context -isnot [pscustomobject] -or
        @('schema_version', 'release_history', 'merge_target' |
            Where-Object { $_ -cnotin $context.PSObject.Properties.Name }).Count -gt 0) {
        throw 'release-context must return explicit schema_version, release_history and merge_target fields.'
    }
    if (($context.schema_version -isnot [long] -and $context.schema_version -isnot [int]) -or
        $context.schema_version -ne 2 -or
        $context.release_history -isnot [string] -or
        $context.release_history -cnotmatch '^[0-9a-f]{40}$' -or
        ($null -ne $context.merge_target -and
            ($context.merge_target -isnot [string] -or $context.merge_target -cnotmatch '^[0-9a-f]{40}$'))) {
        throw 'release-context schema 2 must supply full history and null or full normalized target commits.'
    }
    $target = if ($null -eq $context.merge_target) { 'none' } else { $context.merge_target }
    Write-Verbose "Release assessment uses actual history $($context.release_history) and normalized merge target $target." -Verbose
    $check = @('check', '--format', 'github', '--verbose')
    if (-not $VersionReadinessOnly) {
        $check += @('--config', '.cargo/release_plan.toml')
    }
    Invoke-ReleasePlanTool -Command $check -History $context.release_history `
        -MergeTarget $context.merge_target -Tool $Tool
}

Export-ModuleMember -Function Invoke-ReleaseValidation

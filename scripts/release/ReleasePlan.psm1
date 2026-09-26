#requires -Version 7.6

# Thin local Just boundary: cargo-release-plan owns the version/configuration verdict.
# Hosted checks and the portable skill invoke the application directly.
# Ref: docs/release-versioning.md.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

function Invoke-ReleaseValidation {
    [CmdletBinding()]
    param(
        [string] $Base = $env:RELEASE_PLAN_BASE,
        [scriptblock] $Cargo = { param([string[]] $Argument) & cargo @Argument }
    )

    $argument = @('run', '--quiet', '--locked', '-p', 'cargo-release-plan', '--',
        'check', '--config', '.cargo/release_plan.toml', '--format', 'github', '--verbose')
    if (-not [string]::IsNullOrWhiteSpace($Base)) {
        $argument += @('--base', $Base)
    }
    $output = & $Cargo $argument
    if ($LASTEXITCODE -ne 0) {
        throw "cargo-release-plan check failed with exit code $LASTEXITCODE."
    }
    return $output
}

Export-ModuleMember -Function Invoke-ReleaseValidation

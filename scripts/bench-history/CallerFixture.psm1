#Requires -Version 7.6

# Local and Standard-validation preflight for the standalone caller workspace. Cargo, not
# source-text inspection, checks nested path-dependency resolution before Azure work starts.
# Ref: .github/workflows/implementation.md#reusable-workflow-canary.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

function Assert-CallerFixture {
    [CmdletBinding()]
    param([Parameter(Mandatory)][string] $Workspace)

    $workspacePath = (Resolve-Path -LiteralPath $Workspace).ProviderPath
    $before = @(git -C $workspacePath status --porcelain=v1 --untracked-files=normal)
    if ($before.Count -ne 0) {
        throw "Caller fixture validation requires a clean checkout: $($before -join '; ')"
    }

    Assert-CallerFixtureLock -Workspace $workspacePath
    $after = @(git -C $workspacePath status --porcelain=v1 --untracked-files=normal)
    if ($after.Count -ne 0) {
        throw "Caller fixture validation modified the checkout: $($after -join '; ')"
    }
}

function Assert-CallerFixtureLock {
    # Version planning checks resolution while edits are still uncommitted; collection also
    # requires the clean-checkout contract above. Both use the same standalone Cargo boundary.
    [CmdletBinding()]
    param([Parameter(Mandatory)][string] $Workspace)

    $manifest = Join-Path (Resolve-Path -LiteralPath $Workspace).ProviderPath `
        '.github/fixtures/bench-history-caller/Cargo.toml'
    # --no-deps bypasses lockfile resolution and cannot detect stale nested path versions.
    # Keep the complete graph and fail rather than repairing inputs during measurement.
    Push-Location (Split-Path -Parent $manifest)
    try {
        cargo metadata --manifest-path $manifest --locked --format-version 1 | Out-Null
    } finally {
        Pop-Location
    }
}

Export-ModuleMember -Function Assert-CallerFixture, Assert-CallerFixtureLock

#requires -Version 7

# cargo-delta orchestration shared by the local `just delta*` recipes and the CI `delta` job.
#
# cargo-delta answers "which workspace packages are affected by this branch's changes vs.
# origin/main", so the whole validation matrix can be scoped to just those packages. The same
# analyze-current / analyze-baseline-via-worktree / run / parse pipeline was previously copied both
# into the justfile and into .github/workflows/validation.yml, which is exactly how the two drift
# apart. It lives here once now: the fragile parsing (Read-DeltaAffectedPackage), the removed-package
# filtering (Select-ExistingPackage) and the CI output shaping (Get-DeltaOutput) are pure and
# Pester-tested, and the orchestration (Invoke-CargoDelta) is the single seam both callers use. The
# only intentional difference between callers is baseline
# freshness: the local recipe fetches origin/main first, whereas CI checks out with full history
# (fetch-depth: 0) and passes -SkipFetch.

Set-StrictMode -Version Latest

function Read-DeltaAffectedPackage {
    # Parses the JSON emitted by `cargo delta run` and returns its list of affected package names
    # as a string array (empty when nothing is affected). Tolerates a missing or null `Affected`
    # field so a well-formed "nothing changed" report is not mistaken for an error - which also
    # keeps it safe under Set-StrictMode, where blindly reading an absent property would throw.
    [CmdletBinding()]
    [OutputType([string[]])]
    param(
        [Parameter(Mandatory)][AllowEmptyString()][string] $DeltaJson
    )

    if ([string]::IsNullOrWhiteSpace($DeltaJson)) { return @() }

    $delta = $DeltaJson | ConvertFrom-Json
    if ($null -eq $delta) { return @() }
    if (-not ($delta.PSObject.Properties.Name -contains 'Affected')) { return @() }
    if ($null -eq $delta.Affected) { return @() }
    return @($delta.Affected)
}

function Select-ExistingPackage {
    # Filters affected package names down to those that still exist in the current workspace,
    # preserving order. cargo-delta compares the branch against origin/main, so a package deleted or
    # renamed away on this branch is reported as affected (its files changed - they were removed) yet
    # cannot be validated here: a scoped `cargo <cmd> -p <name>` would fail with "package ID
    # specification `<name>` did not match any packages". Dropping such a package is correct - there
    # is nothing left on this branch to validate - while every package that still exists (including
    # the rename's replacement and anything depending on it) is retained. Pure, so the membership
    # logic is test-covered independently of the cargo/git orchestration.
    [CmdletBinding()]
    [OutputType([string[]])]
    param(
        [Parameter(Mandatory)][AllowEmptyCollection()][string[]] $Affected,
        [Parameter(Mandatory)][AllowEmptyCollection()][string[]] $WorkspacePackage
    )

    $existing = [System.Collections.Generic.HashSet[string]]::new(
        [string[]] $WorkspacePackage,
        [System.StringComparer]::Ordinal)
    return @($Affected | Where-Object { $existing.Contains($_) })
}

function Get-DeltaOutput {
    # Shapes an affected-package list into the three step outputs the CI `delta` job publishes:
    # `packages` (space-separated, the form `just package="..."` expects), `packages_json` (a JSON
    # array the matrix `contains(fromJson(...))` checks consume), and `skip_all` ('true' when
    # nothing is affected, so dependent jobs can short-circuit). Pure so the exact JSON shaping is
    # test-covered independently of the cargo/git orchestration.
    [CmdletBinding()]
    [OutputType([pscustomobject])]
    param(
        [Parameter(Mandatory)][AllowEmptyCollection()][string[]] $Affected
    )

    $packagesJson = if ($Affected.Count -eq 0) {
        '[]'
    } else {
        '[' + (($Affected | ForEach-Object { ConvertTo-Json $_ -Compress }) -join ',') + ']'
    }

    return [pscustomobject]@{
        Packages     = $Affected -join ' '
        PackagesJson = $packagesJson
        SkipAll      = if ($Affected.Count -eq 0) { 'true' } else { 'false' }
    }
}

function Get-WorkspacePackage {
    # Returns the current workspace's member package names as a string array, via `cargo metadata`.
    # Used to drop packages that cargo-delta reports as affected but that no longer exist on this
    # branch (see Select-ExistingPackage). `--no-deps` keeps it to workspace members and avoids
    # resolving - or touching - the dependency graph and lockfile. `--locked` guarantees the call is
    # read-only, failing loudly rather than regenerating `Cargo.lock` as a surprise side effect.
    [CmdletBinding()]
    [OutputType([string[]])]
    param()

    $metadataJson = cargo metadata --no-deps --format-version 1 --locked | Out-String
    $metadata = $metadataJson | ConvertFrom-Json
    return @($metadata.packages | ForEach-Object { $_.name })
}

function Invoke-CargoDelta {
    # Runs the full cargo-delta pipeline and returns the affected package names as a string array.
    # Analyzes the current checkout, then analyzes origin/main in a throwaway git worktree (so the
    # branch is never switched), runs the comparison, and parses the result. Unless -SkipFetch is
    # given, origin/main is refreshed first (unshallowing a shallow clone) so the baseline is
    # current; CI passes -SkipFetch because it already checks out full history. All intermediate
    # artifacts go in a temp directory that is always cleaned up.
    [CmdletBinding()]
    [OutputType([string[]])]
    param(
        [string] $ConfigPath = (Resolve-Path 'delta.toml').Path,
        [switch] $SkipFetch
    )

    $PSNativeCommandUseErrorActionPreference = $true

    if (-not $SkipFetch) {
        # We need the full history of both branches to compare them. A shallow clone (e.g. a
        # default CI checkout) must be unshallowed first; a full clone just fetches origin/main.
        $isShallow = git rev-parse --is-shallow-repository
        if ($isShallow -eq 'true') {
            git fetch --unshallow origin main | Out-Null
        } else {
            git fetch origin main | Out-Null
        }
    }

    $tempDir = Join-Path ([System.IO.Path]::GetTempPath()) "cargo-delta-$([guid]::NewGuid().ToString('n'))"
    New-Item -ItemType Directory -Path $tempDir -Force | Out-Null
    try {
        $baselineJson = Join-Path $tempDir 'baseline.json'
        $currentJson = Join-Path $tempDir 'current.json'

        # Analyze the current branch first (we are already on it).
        Write-Host 'Analyzing current branch...'
        cargo delta -c $ConfigPath analyze | Set-Content -Path $currentJson -Encoding utf8

        # Use a git worktree to analyze origin/main without switching branches.
        $worktreeDir = Join-Path $tempDir 'main-worktree'
        git worktree add --quiet $worktreeDir origin/main | Out-Null
        try {
            Write-Host 'Analyzing baseline (origin/main)...'
            Push-Location $worktreeDir
            try {
                cargo delta -c $ConfigPath analyze | Set-Content -Path $baselineJson -Encoding utf8
            } finally {
                Pop-Location
            }
        } finally {
            git worktree remove $worktreeDir --force | Out-Null
        }

        Write-Host 'Computing delta...'
        # Capture stdout directly: when nothing changed, cargo-delta writes its "quitting" notice
        # to stderr and emits no stdout, which the parser treats as "nothing affected".
        $deltaJson = cargo delta -c $ConfigPath run --baseline $baselineJson --current $currentJson | Out-String

        $affected = Read-DeltaAffectedPackage -DeltaJson $deltaJson
        return Select-ExistingPackage -Affected $affected -WorkspacePackage (Get-WorkspacePackage)
    } finally {
        Remove-Item -Path $tempDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Export-ModuleMember -Function Read-DeltaAffectedPackage, Select-ExistingPackage, Get-WorkspacePackage, Get-DeltaOutput, Invoke-CargoDelta

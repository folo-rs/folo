#requires -Version 7

# Discovers benchmark targets for the benchmark smoke and measurement recipes.
#
# Callgrind benches live in `packages/<pkg>/benches/*_cg.rs` (the `_cg` suffix is the naming
# convention that pairs a Callgrind file with its Criterion counterpart); other `.rs` files in
# those directories are Criterion benches. Turning a package filter and an optional target filter
# into concrete (package, bench) pairs involves directory walking and filtering that is easy to get
# wrong and worth covering with tests, so it lives here. Recipes retain only orchestration.

Set-StrictMode -Version Latest

function Get-CallgrindBenchTarget {
    # Returns the Callgrind benchmark targets under $PackagesRoot as objects with `Package` and
    # `Bench` properties, sorted for deterministic ordering. An empty $PackageFilter scans every
    # package directory; otherwise it is a space-separated allow-list (e.g. cargo-delta output).
    # A non-empty $TargetFilter restricts results to a single bench name. Packages without a
    # `benches/` directory are skipped silently.
    [CmdletBinding()]
    [OutputType([pscustomobject])]
    param(
        [Parameter(Mandatory)][string] $PackagesRoot,
        [string] $PackageFilter = '',
        [string] $TargetFilter = ''
    )

    $packages = if ([string]::IsNullOrWhiteSpace($PackageFilter)) {
        Get-ChildItem -LiteralPath $PackagesRoot -Directory |
            ForEach-Object { $_.Name } |
            Sort-Object
    } else {
        # Support space-separated package lists (e.g. from cargo-delta output).
        $PackageFilter -split ' ' | Where-Object { $_ -ne '' }
    }

    $targets = [System.Collections.Generic.List[pscustomobject]]::new()
    foreach ($package in $packages) {
        $benchesDir = Join-Path $PackagesRoot $package 'benches'
        if (-not (Test-Path -LiteralPath $benchesDir)) { continue }

        $benchFiles = Get-ChildItem -LiteralPath $benchesDir -Filter '*_cg.rs' | Sort-Object Name
        foreach ($benchFile in $benchFiles) {
            $benchName = $benchFile.BaseName
            if (-not [string]::IsNullOrWhiteSpace($TargetFilter) -and $benchName -ne $TargetFilter) { continue }
            $targets.Add([pscustomobject]@{ Package = $package; Bench = $benchName })
        }
    }
    return $targets.ToArray()
}

function Get-CriterionBenchTarget {
    # Returns Criterion benchmark targets, excluding `_cg` Callgrind targets. An empty package
    # filter scans the workspace; otherwise the filter is a space-separated package allow-list.
    [CmdletBinding()]
    [OutputType([pscustomobject])]
    param(
        [Parameter(Mandatory)][string] $PackagesRoot,
        [string] $PackageFilter = ''
    )

    $packages = if ([string]::IsNullOrWhiteSpace($PackageFilter)) {
        Get-ChildItem -LiteralPath $PackagesRoot -Directory |
            ForEach-Object { $_.Name } |
            Sort-Object
    } else {
        $PackageFilter -split ' ' | Where-Object { $_ -ne '' }
    }

    $targets = [System.Collections.Generic.List[pscustomobject]]::new()
    foreach ($package in $packages) {
        $benchesDir = Join-Path $PackagesRoot $package 'benches'
        if (-not (Test-Path -LiteralPath $benchesDir)) { continue }

        $benchFiles = Get-ChildItem -LiteralPath $benchesDir -Filter '*.rs' |
            Where-Object { $_.BaseName -notlike '*_cg' } |
            Sort-Object Name
        foreach ($benchFile in $benchFiles) {
            $targets.Add([pscustomobject]@{ Package = $package; Bench = $benchFile.BaseName })
        }
    }
    return $targets.ToArray()
}

Export-ModuleMember -Function Get-CallgrindBenchTarget, Get-CriterionBenchTarget

#requires -Version 7

# Backs the `check-external-types` recipe: resolves which crate manifests the external-types check
# should run against.
#
# `check-external-types` runs cargo-check-external-types once per selected package so the
# delta-scoped CI job can check just the crates a branch touches. The one part worth testing in
# isolation is turning the `{{ package }}` filter into a concrete, deterministic list of Cargo.toml
# paths: an empty filter means "every package", a non-empty filter is the space-separated allow-list
# cargo-delta emits, and a name whose manifest no longer exists must be dropped rather than handed to
# cargo as a bad path. Deciding which crates are actually checkable (library vs. proc-macro vs.
# binary-only) is deliberately left to the tool's own `--skip-unsupported` flag, so this module never
# has to reimplement - and drift from - that classification.

Set-StrictMode -Version Latest

function Get-ExternalTypesManifest {
    # Returns the Cargo.toml manifest paths under $PackagesRoot to check, one per selected package,
    # sorted for deterministic ordering. An empty $PackageFilter selects every package that has a
    # Cargo.toml; otherwise it is a space-separated allow-list of package names (e.g. cargo-delta
    # output). Only packages whose manifest exists are returned, so a stale name is skipped instead
    # of being passed to cargo as a non-existent path.
    [CmdletBinding()]
    [OutputType([string])]
    param(
        [Parameter(Mandatory)][string] $PackagesRoot,
        [string] $PackageFilter = ''
    )

    $packages = if ([string]::IsNullOrWhiteSpace($PackageFilter)) {
        Get-ChildItem -LiteralPath $PackagesRoot -Directory |
            Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'Cargo.toml') } |
            ForEach-Object { $_.Name } |
            Sort-Object
    } else {
        # Support space-separated package lists (e.g. from cargo-delta output).
        $PackageFilter -split ' ' | Where-Object { $_ -ne '' } | Sort-Object
    }

    $manifests = [System.Collections.Generic.List[string]]::new()
    foreach ($package in $packages) {
        $manifest = Join-Path $PackagesRoot $package 'Cargo.toml'
        if (Test-Path -LiteralPath $manifest) {
            $manifests.Add($manifest)
        }
    }
    return $manifests.ToArray()
}

Export-ModuleMember -Function Get-ExternalTypesManifest

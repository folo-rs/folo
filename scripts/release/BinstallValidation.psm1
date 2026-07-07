# Validates that every publishable binary crate declares the `[package.metadata.binstall]`
# section the `Release` GitHub workflow (.github/workflows/release.yml) relies on. That
# workflow publishes each binary crate's prebuilt archives under release-plz's
# `<crate>-v<version>` tag, named `<crate>-v<version>-<target>.zip` with the binary at the
# archive root. cargo-binstall cannot derive that non-standard tag layout on its own, so each
# binary crate must spell the contract out in its manifest; a binary crate that forgets it (or
# lets the block drift) would ship with no installable binaries and only be noticed by a user
# reporting a broken `cargo binstall`. This check is run in CI (see the `just validate-binstall`
# recipe) and covered by a Pester suite (BinstallValidation.Tests.ps1) so it can be exercised
# locally against fixtures rather than only in CI.

Set-StrictMode -Version Latest

# The exact binstall contract the release workflow publishes. Keep in lockstep with
# .github/workflows/release.yml (`zip: all`, archive root) and docs/release-automation.md.
$script:RequiredBinstall = [ordered]@{
    'pkg-url' = '{ repo }/releases/download/{ name }-v{ version }/{ name }-v{ version }-{ target }.zip'
    'bin-dir' = '{ bin }{ binary-ext }'
    'pkg-fmt' = 'zip'
}

function Get-PublishableBinaryPackage {
    # Full `cargo metadata` package objects for crates that are publishable to a registry AND own
    # a `bin` target. Unlike the release module's Get-PublishableBinaryCrate (which projects to
    # {Name, Version}), this keeps the whole object so its `metadata.binstall` can be inspected.
    # In `cargo metadata` the `publish` field is null (any registry), an empty list (never
    # publish), or a non-empty registry list, so "publishable" is null-or-non-empty. Runs the
    # real `cargo metadata` (offline with --no-deps); tests point it at a fixture via -ManifestPath.
    [CmdletBinding()]
    param(
        [string] $ManifestPath
    )

    $cargoArgs = @('metadata', '--no-deps', '--format-version', '1')
    if ($ManifestPath) { $cargoArgs += @('--manifest-path', $ManifestPath) }

    $metadata = & cargo @cargoArgs | ConvertFrom-Json
    $metadata.packages |
        Where-Object { ($null -eq $_.publish) -or ($_.publish.Count -gt 0) } |
        Where-Object { $_.targets | Where-Object { $_.kind -contains 'bin' } } |
        Sort-Object -Property name -Unique
}

function Test-BinstallMetadata {
    # Validates a single `cargo metadata` package's `[package.metadata.binstall]` against the
    # release workflow's asset-naming contract. Returns the list of human-readable problems for
    # the package; an empty list means the package is compliant.
    [Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSUseSingularNouns', '',
        Justification = '"Metadata" is a mass noun (the cargo term), not a plural of "metadatum".')]
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][object] $Package
    )

    $problems = [System.Collections.Generic.List[string]]::new()

    # StrictMode makes accessing an absent property throw, so every hop is guarded explicitly.
    $binstall = $null
    if (($Package.PSObject.Properties.Name -contains 'metadata') -and ($null -ne $Package.metadata) -and
        ($Package.metadata.PSObject.Properties.Name -contains 'binstall')) {
        $binstall = $Package.metadata.binstall
    }

    if ($null -eq $binstall) {
        $problems.Add('missing [package.metadata.binstall] section')
        return $problems.ToArray()
    }

    foreach ($key in $script:RequiredBinstall.Keys) {
        $expected = $script:RequiredBinstall[$key]
        if ($binstall.PSObject.Properties.Name -notcontains $key) {
            $problems.Add("missing '$key' (expected '$expected')")
            continue
        }
        $actual = $binstall.$key
        if ($actual -ne $expected) {
            $problems.Add("'$key' is '$actual' but must be '$expected'")
        }
    }

    $problems.ToArray()
}

function Invoke-BinstallValidation {
    # Validates the binstall metadata of every publishable binary crate in the workspace, printing
    # a line per crate. Throws (so the `just validate-binstall` recipe exits non-zero and CI fails)
    # with every problem listed if any crate is non-compliant. Tests point it at a fixture via
    # -ManifestPath.
    [CmdletBinding()]
    param(
        [string] $ManifestPath
    )

    $packages = @(Get-PublishableBinaryPackage -ManifestPath $ManifestPath)
    $failures = [System.Collections.Generic.List[string]]::new()

    foreach ($pkg in $packages) {
        $problems = @(Test-BinstallMetadata -Package $pkg)
        if ($problems.Count -eq 0) {
            Write-Host "  OK    $($pkg.name)"
        } else {
            Write-Host "  FAIL  $($pkg.name)"
            foreach ($problem in $problems) { $failures.Add("$($pkg.name): $problem") }
        }
    }

    if ($failures.Count -gt 0) {
        throw "binstall metadata validation failed:`n" + ($failures -join "`n")
    }

    $noun = if ($packages.Count -eq 1) { 'crate' } else { 'crates' }
    Write-Host "All $($packages.Count) publishable binary $noun declare valid binstall metadata."
}

Export-ModuleMember -Function `
    Get-PublishableBinaryPackage, `
    Test-BinstallMetadata, `
    Invoke-BinstallValidation

#requires -Version 7

# Helpers for the `just validate-versions` / `just semver-checks` recipes.
#
# Package classification is `cargo release-plan`'s job; this module only reads `report.json`
# and selects the subset `cargo-semver-checks` should run on in CI. That subset is the
# packages this pull request releases (status `releasing` or `unreleased-changes`), minus
# `doc(hidden)` `_impl` crates, which have no consumer-visible surface, and minus packages
# with no own released-content change (group members dragged along with nothing of their
# own). Pure so the filter is Pester-tested without spawning the Rust tool.

Set-StrictMode -Version Latest

function Get-SemverCheckPackage {
    # Returns package names from a `cargo release-plan report` JSON file that the CI
    # `semver-checks` job should pass to `cargo semver-checks --all-features`. Order follows
    # the report. Unknown or empty reports yield an empty list rather than throwing, so a
    # pull request that releases nothing is a skip, not a broken job.
    [CmdletBinding()]
    [OutputType([string[]])]
    param(
        [Parameter(Mandatory)][string] $ReportPath
    )

    if (-not (Test-Path -LiteralPath $ReportPath)) {
        throw "release-plan report not found at '$ReportPath'."
    }

    $json = Get-Content -LiteralPath $ReportPath -Raw | ConvertFrom-Json
    if ($null -eq $json) { return @() }
    if (-not ($json.PSObject.Properties.Name -contains 'packages')) { return @() }
    if ($null -eq $json.packages) { return @() }

    $selected = foreach ($package in @($json.packages)) {
        if ($null -eq $package) { continue }

        $name = ''
        if ($package.PSObject.Properties.Name -contains 'name' -and $null -ne $package.name) {
            $name = [string] $package.name
        }
        if ([string]::IsNullOrWhiteSpace($name)) { continue }

        $status = ''
        if ($package.PSObject.Properties.Name -contains 'status' -and $null -ne $package.status) {
            $status = [string] $package.status
        }
        if ($status -ne 'unreleased-changes' -and $status -ne 'releasing') { continue }

        # `_impl` crates are `doc(hidden)` and have no consumer-visible surface, so rustdoc
        # for cargo-semver-checks would not inform the increment floor.
        if ($name.EndsWith('_impl', [System.StringComparison]::Ordinal)) { continue }

        # Group members dragged along with no change of their own have an empty `changed`
        # list (their status stays `released` until a version bump, and after a bump a
        # brand-new package also has none). Either way there is no API surface to compare.
        $changedCount = 0
        if ($package.PSObject.Properties.Name -contains 'changed' -and $null -ne $package.changed) {
            $changedCount = @($package.changed).Count
        }
        if ($changedCount -eq 0) { continue }

        $name
    }
    return @($selected)
}

Export-ModuleMember -Function Get-SemverCheckPackage

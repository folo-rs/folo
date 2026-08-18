#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Pester suite for ExternalTypes.psm1.
#
# Get-ExternalTypesManifest walks the filesystem, so tests build a `packages/<pkg>/Cargo.toml`
# fixture under TestDrive and assert the filter behaviour: empty filter selects every package,
# a whitespace-separated filter restricts to named packages (collapsing repeats and arbitrary
# spacing), directories without a Cargo.toml are ignored, and a filter naming a non-existent
# package drops it instead of emitting a bad path.

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'ExternalTypes.psm1') -Force
}

Describe 'Get-ExternalTypesManifest' {
    BeforeEach {
        $script:Root = Join-Path $TestDrive 'packages'

        # Three real packages, each with a Cargo.toml.
        foreach ($name in @('alpha', 'beta', 'gamma')) {
            $dir = Join-Path $script:Root $name
            New-Item -ItemType Directory -Path $dir -Force | Out-Null
            Set-Content -Path (Join-Path $dir 'Cargo.toml') -Value '[package]'
        }

        # A directory that is not a package (no Cargo.toml) must be ignored.
        New-Item -ItemType Directory -Path (Join-Path $script:Root 'noise') -Force | Out-Null
    }

    It 'selects every package with a Cargo.toml when the filter is empty, sorted' {
        $manifests = @(Get-ExternalTypesManifest -PackagesRoot $script:Root)
        $names = $manifests | ForEach-Object { Split-Path (Split-Path $_ -Parent) -Leaf }
        ($names -join ',') | Should -Be 'alpha,beta,gamma'
    }

    It 'ignores directories without a Cargo.toml' {
        $manifests = @(Get-ExternalTypesManifest -PackagesRoot $script:Root)
        ($manifests | Where-Object { $_ -like '*noise*' }) | Should -BeNullOrEmpty
    }

    It 'restricts to a space-separated package allow-list, sorted' {
        $manifests = @(Get-ExternalTypesManifest -PackagesRoot $script:Root -PackageFilter 'gamma alpha')
        $names = $manifests | ForEach-Object { Split-Path (Split-Path $_ -Parent) -Leaf }
        ($names -join ',') | Should -Be 'alpha,gamma'
    }

    It 'collapses arbitrary whitespace and de-duplicates repeated names' {
        $manifests = @(Get-ExternalTypesManifest -PackagesRoot $script:Root -PackageFilter "gamma`t alpha   gamma")
        $names = $manifests | ForEach-Object { Split-Path (Split-Path $_ -Parent) -Leaf }
        ($names -join ',') | Should -Be 'alpha,gamma'
    }

    It 'drops a filtered package whose manifest does not exist' {
        $manifests = @(Get-ExternalTypesManifest -PackagesRoot $script:Root -PackageFilter 'alpha does_not_exist')
        $names = $manifests | ForEach-Object { Split-Path (Split-Path $_ -Parent) -Leaf }
        ($names -join ',') | Should -Be 'alpha'
    }

    It 'returns a manifest path that points at the package Cargo.toml' {
        $manifests = @(Get-ExternalTypesManifest -PackagesRoot $script:Root -PackageFilter 'beta')
        $manifests.Count | Should -Be 1
        (Test-Path -LiteralPath $manifests[0]) | Should -BeTrue
        (Split-Path $manifests[0] -Leaf) | Should -Be 'Cargo.toml'
    }

    It 'returns nothing when the filter names only unknown packages' {
        $manifests = @(Get-ExternalTypesManifest -PackagesRoot $script:Root -PackageFilter 'nope')
        $manifests | Should -BeNullOrEmpty
    }
}

#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Pester suite for ReleasePlan.psm1. Get-SemverCheckPackage is the filter the CI
# `semver-checks` job uses to turn `report.json` into a `cargo semver-checks -p …` list, so
# the include/exclude rules are asserted against realistic report snippets here.

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'ReleasePlan.psm1') -Force
}

Describe 'Get-SemverCheckPackage' {
    It 'includes unreleased-changes and releasing packages that have their own changes' {
        $path = Join-Path $TestDrive 'report.json'
        @{
            schema_version = 1
            packages       = @(
                @{ name = 'events'; status = 'unreleased-changes'; changed = @(@{ path = 'src/lib.rs' }) }
                @{ name = 'nm'; status = 'releasing'; changed = @(@{ path = 'src/lib.rs' }) }
            )
        } | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $path -Encoding utf8
        Get-SemverCheckPackage -ReportPath $path | Should -Be @('events', 'nm')
    }

    It 'excludes released packages' {
        $path = Join-Path $TestDrive 'released.json'
        @{
            packages = @(
                @{ name = 'events'; status = 'released'; changed = @() }
            )
        } | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $path -Encoding utf8
        Get-SemverCheckPackage -ReportPath $path | Should -BeNullOrEmpty
    }

    It 'excludes _impl crates even when they have unreleased changes' {
        $path = Join-Path $TestDrive 'impl.json'
        @{
            packages = @(
                @{ name = 'nm_impl'; status = 'unreleased-changes'; changed = @(@{ path = 'src/lib.rs' }) }
                @{ name = 'nm'; status = 'unreleased-changes'; changed = @(@{ path = 'src/lib.rs' }) }
            )
        } | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $path -Encoding utf8
        Get-SemverCheckPackage -ReportPath $path | Should -Be @('nm')
    }

    It 'excludes packages with no own changed entries' {
        $path = Join-Path $TestDrive 'empty-changed.json'
        @{
            packages = @(
                @{ name = 'nm'; status = 'releasing'; changed = @() }
            )
        } | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $path -Encoding utf8
        Get-SemverCheckPackage -ReportPath $path | Should -BeNullOrEmpty
    }

    It 'returns an empty list when packages is missing' {
        $path = Join-Path $TestDrive 'no-packages.json'
        '{"schema_version":1}' | Set-Content -LiteralPath $path -Encoding utf8
        Get-SemverCheckPackage -ReportPath $path | Should -BeNullOrEmpty
    }

    It 'throws when the report file does not exist' {
        { Get-SemverCheckPackage -ReportPath (Join-Path $TestDrive 'missing.json') } |
            Should -Throw '*release-plan report not found*'
    }

    It 'still selects a single package when ConvertTo-Json collapses the arrays' {
        $path = Join-Path $TestDrive 'single.json'
        @{
            packages = @(
                @{ name = 'events'; status = 'unreleased-changes'; changed = @(@{ path = 'src/lib.rs' }) }
            )
        } | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $path -Encoding utf8
        $result = @(Get-SemverCheckPackage -ReportPath $path)
        $result.Count | Should -Be 1
        $result[0] | Should -Be 'events'
    }
}

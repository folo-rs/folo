#Requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Checks that generated matrix entries have unique identities and independent state without
# duplicating the configured check catalog. Deep validation consumes this plan before setup.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll { Import-Module (Join-Path $PSScriptRoot 'ScheduledPlan.psm1') -Force }

Describe 'Fresh deep check catalog' {
    It 'provides nonempty unique matrix identities' {
        $checks = @(Get-ScheduledCheck)
        $checks.Count | Should -BeGreaterThan 0
        @($checks.id | Sort-Object -Unique).Count | Should -Be $checks.Count
        foreach ($check in $checks) {
            $check.id | Should -Not -BeNullOrEmpty
            $check.recipe | Should -Not -BeNullOrEmpty
            $check.platform | Should -Not -BeNullOrEmpty
        }
    }

    It 'returns independent declarations on each invocation' {
        $check = @(Get-ScheduledCheck)[0]
        $recipe = $check.recipe
        $packages = $check.packages
        $check.recipe = 'fixture'
        $check.packages = @('fixture')
        $fresh = @(Get-ScheduledCheck)[0]
        $fresh.recipe | Should -Be $recipe
        $fresh.packages | Should -Be $packages
    }
}

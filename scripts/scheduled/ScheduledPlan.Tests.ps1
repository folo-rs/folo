#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Protects the complete nightly check scope without executing expensive tools.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll { Import-Module (Join-Path $PSScriptRoot 'ScheduledPlan.psm1') -Force }

Describe 'Fresh deep check catalog' {
    It 'preserves every platform, mutation shard and many-seed family' {
        $checks = @(Get-ScheduledCheck)
        $checks.Count | Should -Be 26
        @($checks.id | Sort-Object -Unique).Count | Should -Be $checks.Count
        @($checks.recipe | Sort-Object -Unique) | Should -Be @('careful', 'miri', 'miri-harder', 'mutants')
        @($checks | Where-Object recipe -EQ 'miri').platform | Should -Be @(
            'ubuntu-latest', 'windows-latest', 'ubuntu-24.04-arm', 'windows-11-arm')
        foreach ($platform in @('ubuntu-latest', 'windows-latest')) {
            @($checks | Where-Object { $_.recipe -eq 'mutants' -and $_.platform -eq $platform }).shard |
                Should -Be @(1..8 | ForEach-Object { "$_/8" })
            @($checks | Where-Object { $_.recipe -eq 'careful' -and $_.platform -eq $platform }).Count | Should -Be 1
        }
        foreach ($package in @('events_once', 'events', 'awaiter_set', 'nm_impl')) {
            $selected = @($checks | Where-Object { $_.recipe -eq 'miri-harder' -and $_.packages -contains $package })
            $selected.Count | Should -Be 1
            $selected.shard | Should -Be '1/1'
            $selected.id | Should -Be "miri-harder-$package-1"
            $selected.platform | Should -Be 'ubuntu-latest'
        }
    }

    It 'uses workspace scope except for the declared many-seed families' {
        $checks = @(Get-ScheduledCheck | Where-Object recipe -NE 'miri-harder')
        foreach ($check in $checks) { $check.packages | Should -BeNullOrEmpty }
    }

    It 'returns independent plain declarations without coordination metadata' {
        $check = @(Get-ScheduledCheck | Where-Object id -EQ 'miri-harder-events-1')[0]
        @($check.Keys | Sort-Object) | Should -Be @('id', 'packages', 'platform', 'recipe', 'shard')
        $check.recipe | Should -Be 'miri-harder'
        $check.packages = @('fixture')
        @(Get-ScheduledCheck | Where-Object id -EQ 'miri-harder-events-1')[0].packages | Should -Be @('events')
    }
}

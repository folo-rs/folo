#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Pester suite for CallgrindBench.psm1. Get-CallgrindBenchTarget walks the filesystem, so each test
# builds a small `packages/<pkg>/benches/` fixture under Pester's TestDrive and asserts which
# (package, bench) pairs it discovers under the various filter combinations.

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'CallgrindBench.psm1') -Force
}

Describe 'Get-CallgrindBenchTarget' {
    BeforeEach {
        $script:Root = Join-Path $TestDrive 'packages'

        # alpha: one Callgrind bench + one plain (non-_cg) bench that must be ignored.
        New-Item -ItemType Directory -Path (Join-Path $script:Root 'alpha' 'benches') -Force | Out-Null
        Set-Content -Path (Join-Path $script:Root 'alpha' 'benches' 'alpha_cg.rs') -Value '// cg'
        Set-Content -Path (Join-Path $script:Root 'alpha' 'benches' 'alpha.rs') -Value '// plain'

        # beta: two Callgrind benches.
        New-Item -ItemType Directory -Path (Join-Path $script:Root 'beta' 'benches') -Force | Out-Null
        Set-Content -Path (Join-Path $script:Root 'beta' 'benches' 'beta_one_cg.rs') -Value '// cg'
        Set-Content -Path (Join-Path $script:Root 'beta' 'benches' 'beta_two_cg.rs') -Value '// cg'

        # gamma: has a benches dir but no Callgrind benches.
        New-Item -ItemType Directory -Path (Join-Path $script:Root 'gamma' 'benches') -Force | Out-Null
        Set-Content -Path (Join-Path $script:Root 'gamma' 'benches' 'gamma.rs') -Value '// plain'

        # delta: no benches dir at all (must be skipped without error).
        New-Item -ItemType Directory -Path (Join-Path $script:Root 'delta') -Force | Out-Null
    }

    It 'discovers every *_cg.rs bench across all packages when no filters are given' {
        $targets = @(Get-CallgrindBenchTarget -PackagesRoot $script:Root)
        $pairs = $targets | ForEach-Object { "$($_.Package)::$($_.Bench)" }
        ($pairs -join ',') | Should -Be 'alpha::alpha_cg,beta::beta_one_cg,beta::beta_two_cg'
    }

    It 'ignores plain (non-_cg) bench files' {
        $targets = @(Get-CallgrindBenchTarget -PackagesRoot $script:Root)
        ($targets | Where-Object { $_.Bench -eq 'alpha' }) | Should -BeNullOrEmpty
    }

    It 'restricts to a space-separated package allow-list' {
        $targets = @(Get-CallgrindBenchTarget -PackagesRoot $script:Root -PackageFilter 'beta')
        $pairs = $targets | ForEach-Object { "$($_.Package)::$($_.Bench)" }
        ($pairs -join ',') | Should -Be 'beta::beta_one_cg,beta::beta_two_cg'
    }

    It 'supports multiple space-separated packages' {
        $targets = @(Get-CallgrindBenchTarget -PackagesRoot $script:Root -PackageFilter 'alpha beta')
        $targets.Count | Should -Be 3
    }

    It 'restricts to a single target name' {
        $targets = @(Get-CallgrindBenchTarget -PackagesRoot $script:Root -TargetFilter 'beta_two_cg')
        $targets.Count | Should -Be 1
        $targets[0].Package | Should -Be 'beta'
        $targets[0].Bench | Should -Be 'beta_two_cg'
    }

    It 'returns nothing for a package without Callgrind benches' {
        $targets = @(Get-CallgrindBenchTarget -PackagesRoot $script:Root -PackageFilter 'gamma')
        $targets | Should -BeNullOrEmpty
    }

    It 'skips a package that has no benches directory without error' {
        { Get-CallgrindBenchTarget -PackagesRoot $script:Root -PackageFilter 'delta' } | Should -Not -Throw
        $targets = @(Get-CallgrindBenchTarget -PackagesRoot $script:Root -PackageFilter 'delta')
        $targets | Should -BeNullOrEmpty
    }

    It 'returns nothing when the target filter matches no bench' {
        $targets = @(Get-CallgrindBenchTarget -PackagesRoot $script:Root -TargetFilter 'does_not_exist_cg')
        $targets | Should -BeNullOrEmpty
    }
}

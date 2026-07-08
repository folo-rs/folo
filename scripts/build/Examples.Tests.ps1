#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Pester suite for Examples.psm1.
#
# Get-ExampleTarget walks the filesystem, so tests build a `packages/<pkg>/examples/` fixture under
# TestDrive covering both example shapes plus the mod.rs and skip-list exclusions. Invoke-ExampleRun
# takes an injected scriptblock in place of cargo, so its timeout/exit-code/output classification is
# tested with fast fake commands (one short real timeout to exercise the watchdog).

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'Examples.psm1') -Force
}

Describe 'Get-ExampleTarget' {
    BeforeEach {
        $script:Root = Join-Path $TestDrive 'packages'

        # alpha: two .rs examples, a mod.rs to ignore, and a subdir example.
        $alpha = Join-Path $script:Root 'alpha' 'examples'
        New-Item -ItemType Directory -Path $alpha -Force | Out-Null
        Set-Content -Path (Join-Path $script:Root 'alpha' 'Cargo.toml') -Value '[package]'
        Set-Content -Path (Join-Path $alpha 'a_one.rs') -Value '// ex'
        Set-Content -Path (Join-Path $alpha 'a_two.rs') -Value '// ex'
        Set-Content -Path (Join-Path $alpha 'mod.rs') -Value '// shared'
        New-Item -ItemType Directory -Path (Join-Path $alpha 'a_dir') -Force | Out-Null
        Set-Content -Path (Join-Path $alpha 'a_dir' 'main.rs') -Value '// ex'

        # beta: one example plus an excluded-by-design example.
        $beta = Join-Path $script:Root 'beta' 'examples'
        New-Item -ItemType Directory -Path $beta -Force | Out-Null
        Set-Content -Path (Join-Path $script:Root 'beta' 'Cargo.toml') -Value '[package]'
        Set-Content -Path (Join-Path $beta 'b_one.rs') -Value '// ex'
        Set-Content -Path (Join-Path $beta 'nm_otel_console.rs') -Value '// infinite loop'

        # gamma: a package with no examples directory (must be skipped).
        New-Item -ItemType Directory -Path (Join-Path $script:Root 'gamma') -Force | Out-Null
        Set-Content -Path (Join-Path $script:Root 'gamma' 'Cargo.toml') -Value '[package]'

        # noise: a directory that is not a package (no Cargo.toml) must be ignored.
        New-Item -ItemType Directory -Path (Join-Path $script:Root 'noise') -Force | Out-Null
    }

    It 'discovers both .rs-file and subdirectory examples across all packages' {
        $targets = @(Get-ExampleTarget -PackagesRoot $script:Root)
        $pairs = $targets | ForEach-Object { "$($_.Package)::$($_.Example)" }
        ($pairs -join ',') | Should -Be 'alpha::a_dir,alpha::a_one,alpha::a_two,beta::b_one'
    }

    It 'ignores mod.rs' {
        $targets = @(Get-ExampleTarget -PackagesRoot $script:Root -PackageFilter 'alpha')
        ($targets | Where-Object { $_.Example -eq 'mod' }) | Should -BeNullOrEmpty
    }

    It 'drops examples on the default skip-list' {
        $targets = @(Get-ExampleTarget -PackagesRoot $script:Root -PackageFilter 'beta')
        ($targets | Where-Object { $_.Example -eq 'nm_otel_console' }) | Should -BeNullOrEmpty
        ($targets | ForEach-Object { $_.Example }) | Should -Be 'b_one'
    }

    It 'honours a caller-supplied exclusion list' {
        $targets = @(Get-ExampleTarget -PackagesRoot $script:Root -PackageFilter 'alpha' -ExcludedExample @('a_two'))
        ($targets | ForEach-Object { $_.Example }) -join ',' | Should -Be 'a_dir,a_one'
    }

    It 'restricts to a space-separated package allow-list' {
        $targets = @(Get-ExampleTarget -PackagesRoot $script:Root -PackageFilter 'beta')
        ($targets | ForEach-Object { $_.Package } | Sort-Object -Unique) | Should -Be 'beta'
    }

    It 'skips packages without an examples directory' {
        $targets = @(Get-ExampleTarget -PackagesRoot $script:Root -PackageFilter 'gamma')
        $targets | Should -BeNullOrEmpty
    }
}

Describe 'Invoke-ExampleRun' {
    It 'reports Success and captures output when the command exits 0' {
        $command = { Write-Output 'line one'; Write-Output 'line two'; return 0 }
        $result = Invoke-ExampleRun -Package 'pkg' -Example 'ex' -Command $command -TimeoutSeconds 30
        $result.Status | Should -Be 'Success'
        $result.ExitCode | Should -Be 0
        $result.Output | Should -Be "line one`nline two"
    }

    It 'reports Success with empty output when the command emits only an exit code' {
        $command = { return 0 }
        $result = Invoke-ExampleRun -Package 'pkg' -Example 'ex' -Command $command -TimeoutSeconds 30
        $result.Status | Should -Be 'Success'
        $result.Output | Should -Be ''
    }

    It 'reports Failed and preserves the non-zero exit code and output' {
        $command = { Write-Output 'boom'; return 7 }
        $result = Invoke-ExampleRun -Package 'pkg' -Example 'ex' -Command $command -TimeoutSeconds 30
        $result.Status | Should -Be 'Failed'
        $result.ExitCode | Should -Be 7
        $result.Output | Should -Be 'boom'
    }

    It 'reports Timeout when the command outlasts the watchdog' {
        $command = { Start-Sleep -Seconds 30; return 0 }
        $result = Invoke-ExampleRun -Package 'pkg' -Example 'ex' -Command $command -TimeoutSeconds 1
        $result.Status | Should -Be 'Timeout'
    }
}

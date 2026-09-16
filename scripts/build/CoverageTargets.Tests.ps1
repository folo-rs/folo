#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }
# Protects coverage-measure's Cargo target selection with real nextest discovery on dependency-free
# packages. Unit, integration and example tests must remain selected for binary-only, library-only
# and mixed scopes; benchmark canaries must never compile. Also executes passing tests with the real
# nextest configuration to verify retained stdout/stderr. No coverage engine or hosted API is mocked
# into producing successful measurements. Ref: ../../docs/build-and-tooling.md#coverage-target-selection.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'

BeforeAll {
    $root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
    # Expand the actual recipe without running its workspace builds. Consume its arguments
    # in the fixture so broken recipe wiring cannot be hidden by test-supplied configuration.
    $recipe = (just --justfile (Join-Path $root 'justfile') --dry-run coverage-measure 2>&1) -join "`n"
    $LASTEXITCODE | Should -Be 0
    $parseErrors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseInput($recipe, [ref] $null, [ref] $parseErrors)
    $parseErrors | Should -BeNullOrEmpty
    $measurement = @($ast.FindAll({
                param($node)
                $node -is [System.Management.Automation.Language.CommandAst] -and
                    $node.GetCommandName() -eq 'cargo' -and $node.Extent.Text -match ' llvm-cov nextest '
            }, $true))
    $measurement.Count | Should -Be 1
    $elements = $measurement[0].CommandElements
    $script:selectors = @($elements.Extent.Text |
        Where-Object { $_ -match '^--(?:lib|bins|tests|examples|benches|all-targets)$' })
    $script:configurationArguments = @(
        for ($index = 0; $index -lt $elements.Count; $index++) {
            if ($elements[$index].Extent.Text -eq '--tool-config-file') {
                $elements[$index].Extent.Text
                $elements[($index + 1)].SafeGetValue()
            }
        }
    )
    $script:workspace = Join-Path $TestDrive 'coverage-targets'
    $null = New-Item -ItemType Directory -Path $workspace
    $null = New-Item -ItemType Directory -Path (Join-Path $workspace '.config')
    # The fixture has no repository-specific package overrides. Its report path is
    # ordinary runner setup; the coverage tool config supplies the behavior under test.
    @'
[profile.default.junit]
path = "junit.xml"
'@ | Set-Content -LiteralPath (Join-Path $workspace '.config\nextest.toml')
    @'
[workspace]
members = ["binary_only", "library_only", "mixed"]
resolver = "3"
'@ | Set-Content -LiteralPath (Join-Path $workspace 'Cargo.toml')
    foreach ($name in @('binary_only', 'library_only', 'mixed')) {
        $directory = Join-Path $workspace $name
        foreach ($folder in @('src', 'tests', 'examples', 'benches')) {
            $null = New-Item -ItemType Directory -Path (Join-Path $directory $folder) -Force
        }
        @"
[package]
name = "$name"
version = "0.0.0"
edition = "2024"
publish = false
"@ | Set-Content -LiteralPath (Join-Path $directory 'Cargo.toml')
        if ($name -ne 'binary_only') {
            '#[test] fn library_case() {}' |
                Set-Content -LiteralPath (Join-Path $directory 'src\lib.rs')
        }
        if ($name -ne 'library_only') {
            'fn main() {}',
                '#[test] fn binary_case() { println!("stdout canary"); eprintln!("stderr canary"); }' |
                Set-Content -LiteralPath (Join-Path $directory 'src\main.rs')
        }
        '#[test] fn integration_case() {}' |
            Set-Content -LiteralPath (Join-Path $directory 'tests\integration.rs')
        'fn main() {}', '#[test] fn example_case() {}' |
            Set-Content -LiteralPath (Join-Path $directory 'examples\example.rs')
        'compile_error!("benchmark targets must not be selected for coverage");' |
            Set-Content -LiteralPath (Join-Path $directory 'benches\benchmark.rs')
    }
}

Describe 'Coverage Cargo target selection' {
    It 'uses test and example selection without requiring a library or selecting benchmarks' {
        $selectors | Should -Be @('--tests', '--examples')
    }

    It 'discovers every intended test in <Packages>' -TestCases @(
        @{ Packages = @('binary_only') }
        @{ Packages = @('library_only') }
        @{ Packages = @('mixed') }
        @{ Packages = @('binary_only', 'library_only', 'mixed') }
    ) {
        param($Packages)
        $packageArguments = @($Packages | ForEach-Object { '-p'; $_ })
        $json = cargo nextest list --manifest-path (Join-Path $workspace 'Cargo.toml') `
            --target-dir (Join-Path $workspace 'target') --offline --all-features `
            --message-format json @selectors @packageArguments
        $LASTEXITCODE | Should -Be 0
        $inventory = ($json -join "`n") | ConvertFrom-Json -AsHashtable
        $actual = @(
            foreach ($suite in $inventory['rust-suites'].Values) {
                foreach ($testName in $suite.testcases.Keys) {
                    "$($suite['package-name'])::$testName"
                }
            }
        )
        $expected = @(
            foreach ($name in $Packages) {
                if ($name -ne 'binary_only') { "${name}::library_case" }
                if ($name -ne 'library_only') { "${name}::binary_case" }
                "${name}::integration_case"
                "${name}::example_case"
            }
        )
        @($actual | Sort-Object) | Should -Be @($expected | Sort-Object)
    }

    It 'retains successful-test output only with coverage configuration=<UseCoverageConfiguration>' -TestCases @(
        @{ UseCoverageConfiguration = $true }
        @{ UseCoverageConfiguration = $false }
    ) {
        param($UseCoverageConfiguration)
        $runnerConfiguration = if ($UseCoverageConfiguration) { $configurationArguments } else { @() }
        cargo nextest run --manifest-path (Join-Path $workspace 'Cargo.toml') `
            @runnerConfiguration --target-dir (Join-Path $workspace 'target') `
            --offline --all-features @selectors -p binary_only
        $LASTEXITCODE | Should -Be 0
        [xml] $report = Get-Content -LiteralPath (Join-Path $workspace 'target\nextest\default\junit.xml') -Raw
        $test = @($report.testsuites.testsuite.testcase | Where-Object name -EQ 'binary_case')
        $test.Count | Should -Be 1
        if ($UseCoverageConfiguration) {
            $test[0].'system-out' | Should -Match 'stdout canary'
            $test[0].'system-err' | Should -Match 'stderr canary'
        }
        else {
            $test[0].InnerXml | Should -Not -Match 'stdout canary|stderr canary'
        }
    }
}

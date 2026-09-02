#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Pester suite for ReleasePlan.psm1. Get-SemverCheckPackage is the filter the CI
# `semver-checks` job uses to turn `report.json` into a `cargo semver-checks -p ...` list, so
# the include/exclude rules are asserted against realistic report snippets here.

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'ReleasePlan.psm1') -Force
}

Describe 'Get-ReleasePlanCargoArgument' {
    It 'forwards --base when a release baseline is set' {
        $argument = Get-ReleasePlanCargoArgument -Command @('check', '--format', 'github') -Base 'abc123'
        $argument | Should -Contain '--base'
        $argument | Should -Contain 'abc123'
    }

    It 'omits --base when the release baseline is empty' {
        $argument = Get-ReleasePlanCargoArgument -Command @('check', '--format', 'github') -Base ''
        $argument | Should -Not -Contain '--base'
    }
}

Describe 'Get-SemverCheckPackage' {
    It 'includes needs-increment and pending-release packages that have their own changes' {
        $path = Join-Path $TestDrive 'report.json'
        @{
            schema_version = 1
            packages       = @(
                @{ name = 'events'; status = 'needs-increment'; changed = @(@{ path = 'src/lib.rs' }) }
                @{ name = 'nm'; status = 'pending-release'; changed = @(@{ path = 'src/lib.rs' }) }
            )
        } | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $path -Encoding utf8
        Get-SemverCheckPackage -ReportPath $path | Should -Be @('events', 'nm')
    }

    It 'excludes unchanged packages' {
        $path = Join-Path $TestDrive 'unchanged.json'
        @{
            schema_version = 1
            packages       = @(
                @{ name = 'events'; status = 'unchanged'; changed = @() }
            )
        } | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $path -Encoding utf8
        Get-SemverCheckPackage -ReportPath $path | Should -BeNullOrEmpty
    }

    It 'selects the public shell when a grouped _impl member has released-content changes' {
        $path = Join-Path $TestDrive 'impl.json'
        @{
            schema_version = 1
            packages       = @(
                @{ name = 'nm_impl'; status = 'needs-increment'; group = 'nm'; changed = @(@{ path = 'src/lib.rs' }) }
                @{ name = 'nm'; status = 'unchanged'; group = 'nm'; changed = @() }
            )
            groups         = @{
                nm = @{ members = @('nm', 'nm_impl') }
            }
        } | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $path -Encoding utf8
        Get-SemverCheckPackage -ReportPath $path | Should -Be @('nm')
    }

    It 'does not select an _impl crate as a comparison target' {
        $path = Join-Path $TestDrive 'impl-only.json'
        @{
            schema_version = 1
            packages       = @(
                @{ name = 'nm_impl'; status = 'needs-increment'; changed = @(@{ path = 'src/lib.rs' }) }
            )
        } | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $path -Encoding utf8
        Get-SemverCheckPackage -ReportPath $path | Should -BeNullOrEmpty
    }

    It 'excludes packages with no own changed entries and no _impl sibling change' {
        $path = Join-Path $TestDrive 'empty-changed.json'
        @{
            schema_version = 1
            packages       = @(
                @{ name = 'nm'; status = 'pending-release'; changed = @() }
            )
        } | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $path -Encoding utf8
        Get-SemverCheckPackage -ReportPath $path | Should -BeNullOrEmpty
    }

    It 'throws when packages is missing' {
        $path = Join-Path $TestDrive 'no-packages.json'
        '{"schema_version":1}' | Set-Content -LiteralPath $path -Encoding utf8
        { Get-SemverCheckPackage -ReportPath $path } |
            Should -Throw '*missing packages*'
    }

    It 'throws when schema_version is missing' {
        $path = Join-Path $TestDrive 'no-schema.json'
        '{"packages":[]}' | Set-Content -LiteralPath $path -Encoding utf8
        { Get-SemverCheckPackage -ReportPath $path } |
            Should -Throw '*missing schema_version*'
    }

    It 'joins an empty selected set to the empty string written as released=' {
        $path = Join-Path $TestDrive 'all-unchanged.json'
        @{
            schema_version = 1
            packages       = @(
                @{ name = 'events'; status = 'unchanged'; changed = @() }
            )
        } | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $path -Encoding utf8
        $released = @(Get-SemverCheckPackage -ReportPath $path)
        $released.Count | Should -Be 0
        ($released -join ' ') | Should -BeExactly ''
    }

    It 'throws when the report file does not exist' {
        { Get-SemverCheckPackage -ReportPath (Join-Path $TestDrive 'missing.json') } |
            Should -Throw '*release-plan report not found*'
    }

    It 'still selects a single package when ConvertTo-Json collapses the arrays' {
        $path = Join-Path $TestDrive 'single.json'
        @{
            schema_version = 1
            packages       = @(
                @{ name = 'events'; status = 'needs-increment'; changed = @(@{ path = 'src/lib.rs' }) }
            )
        } | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $path -Encoding utf8
        $result = @(Get-SemverCheckPackage -ReportPath $path)
        $result.Count | Should -Be 1
        $result[0] | Should -Be 'events'
    }
}

Describe 'Complete-SemverChecksCollect' {
    It 'accepts a clean comparison' {
        { Complete-SemverChecksCollect -ExitCode 0 -LogPath 'semver-checks.log' } | Should -Not -Throw
    }

    It 'accepts the documented finding-exit as an increment floor' {
        { Complete-SemverChecksCollect -ExitCode 100 -LogPath 'semver-checks.log' } | Should -Not -Throw
    }

    It 'throws on a tool error' {
        { Complete-SemverChecksCollect -ExitCode 101 -LogPath 'semver-checks.log' } |
            Should -Throw '*exit 101*'
    }
}

Describe 'Invoke-ValidateVersions' {
    It 'emits released= when the report selects nothing, then runs check' {
        $output = Join-Path $TestDrive 'github-output'
        New-Item -ItemType File -Path $output | Out-Null
        $script:calls = [System.Collections.Generic.List[object]]::new()
        $cargo = {
            param([string[]] $Argument)
            $script:calls.Add(@($Argument))
            if ($Argument -contains 'report') {
                $index = [array]::IndexOf($Argument, '--out-dir')
                $dir = $Argument[$index + 1]
                @{
                    schema_version = 1
                    packages       = @(
                        @{ name = 'events'; status = 'unchanged'; changed = @() }
                    )
                } | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $dir 'report.json') -Encoding utf8
            }
        }
        Invoke-ValidateVersions -GitHubOutputPath $output -Base 'abc' -Cargo $cargo
        @(Get-Content -LiteralPath $output) | Should -Be @('released=')
        $script:calls.Count | Should -Be 2
        $script:calls[0] | Should -Contain 'report'
        $script:calls[1] | Should -Contain 'check'
        $script:calls[1] | Should -Contain 'abc'
    }
}

#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Protects the local and CI validation gates: injected Cargo output is authoritative, failed
# commands cannot emit targets, and compatibility execution retains its exit and environment rules.
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'ReleasePlan.psm1') -Force
    $script:previousBase = $env:RELEASE_PLAN_BASE
    $env:RELEASE_PLAN_BASE = 'baseline-must-not-reach-artifact-commands'
    $global:LASTEXITCODE = 0
}

AfterAll {
    $env:RELEASE_PLAN_BASE = $script:previousBase
}

Describe 'CI compatibility target selection' {
    It 'preserves the semver-targets array for <Json>' -TestCases @(
        @{ Json = '[]'; ExpectedCount = 0 },
        @{ Json = '["alpha"]'; ExpectedCount = 1 },
        @{ Json = '["alpha","beta"]'; ExpectedCount = 2 }
    ) {
        param($Json, $ExpectedCount)
        InModuleScope ReleasePlan -Parameters @{ Json = $Json; ExpectedCount = $ExpectedCount } {
            param($Json, $ExpectedCount)
            $fixtureJson = $Json
            $actual = Get-AffectedSemverCheckTarget -ReportPath 'absent report.json' -Cargo {
                param($Argument)
                $Argument | Should -Be @(
                    'run', '-p', 'cargo-release-plan', '--locked', '--',
                    'semver-targets', '--report', 'absent report.json', '--verbose'
                )
                $global:LASTEXITCODE = 0
                $fixtureJson
            }
            ($actual -is [array]) | Should -BeTrue
            $actual.Count | Should -Be $ExpectedCount
            ConvertTo-Json -InputObject $actual -Compress | Should -BeExactly $Json
        }
    }

    It 'rejects failed JSON commands before parsing their stdout' {
        InModuleScope ReleasePlan {
            Mock ConvertFrom-Json { throw 'parser must not run' }
            {
                Get-AffectedSemverCheckTarget -ReportPath 'report.json' -Cargo {
                    $global:LASTEXITCODE = 23
                    'invalid json'
                }
            } | Should -Throw
            Should -Invoke ConvertFrom-Json -Times 0
        }
    }

    It 'rejects invalid JSON after a successful subprocess' {
        InModuleScope ReleasePlan {
            {
                Get-AffectedSemverCheckTarget -ReportPath 'report.json' -Cargo {
                    $global:LASTEXITCODE = 0
                    'invalid json'
                }
            } | Should -Throw
        }
    }
}

Describe 'Compatibility validation gates' {
    It 'invokes the canary once and propagates failure=<Fails>' -ForEach @(
        @{ Fails = $false }, @{ Fails = $true }
    ) {
        $script:count = 0
        $action = {
            Invoke-VerifySemverCheck -Cargo {
                param($Argument)
                $script:count++
                $Argument | Should -Be @('semver-checks', '--baseline-rev', 'HEAD', '-p', 'folo_utils')
                $global:LASTEXITCODE = [int] $Fails
            }
        }
        if ($Fails) { $action | Should -Throw } else { & $action }
        $script:count | Should -Be 1
    }

    It 'does not invoke semver-checks for an empty Just package selection' {
        Invoke-SemverCheck -Package '  ' -Cargo { throw 'must not execute' }
    }

    It 'uses repeated package arguments and propagates the semver-checks exit' {
        {
            Invoke-SemverCheck -Package 'alpha beta' -Cargo {
                param($Argument)
                $Argument | Should -Be @('semver-checks', '--all-features', '-p', 'alpha', '-p', 'beta')
                $global:LASTEXITCODE = 100
            }
        } | Should -Throw
    }
}

Describe 'Semver target directory lifecycle' {
    It 'keeps paths stable per workspace and distinct across workspaces' {
        InModuleScope ReleasePlan -Parameters @{ Root = $TestDrive } {
            param($Root)
            $previous = $env:CARGO_TARGET_DIR
            try {
                $env:CARGO_TARGET_DIR = $null
                $cacheRoot = Join-Path $Root 'cache'
                $first = Get-SemverCheckTargetDirectory -WorkspaceRoot $Root -TempRoot $cacheRoot
                $first | Should -Be (
                    Get-SemverCheckTargetDirectory -WorkspaceRoot $Root -TempRoot $cacheRoot
                )
                $other = Get-SemverCheckTargetDirectory `
                    -WorkspaceRoot (Join-Path $Root 'other') -TempRoot $cacheRoot
                $other | Should -Not -Be $first
                Split-Path -Parent $first | Should -Be $cacheRoot
                $env:CARGO_TARGET_DIR = $Root
                Get-SemverCheckTargetDirectory -WorkspaceRoot $Root -TempRoot $cacheRoot |
                    Should -Be $Root
            } finally {
                $env:CARGO_TARGET_DIR = $previous
            }
        }
    }

    It 'restores an absent or configured target after failure: <Configured>' -ForEach @(
        @{ Configured = $false }, @{ Configured = $true }
    ) {
        InModuleScope ReleasePlan -Parameters @{ Root = $TestDrive; Configured = $Configured } {
            param($Root, $Configured)
            $previous = $env:CARGO_TARGET_DIR
            $selected = if ($Configured) { Join-Path $Root 'caller' } else { $null }
            try {
                $env:CARGO_TARGET_DIR = $selected
                {
                    Invoke-SemverCheckCargo -Argument @('semver-checks') `
                        -TargetDirectory $Root -Cargo {
                            $env:CARGO_TARGET_DIR | Should -Be $Root
                            throw [IO.IOException]::new('directory canary')
                        }
                } | Should -Throw
                [Environment]::GetEnvironmentVariable('CARGO_TARGET_DIR', 'Process') |
                    Should -Be $selected
            } finally {
                $env:CARGO_TARGET_DIR = $previous
            }
        }
    }

    It 'does not override the target when explicitly disabled' {
        InModuleScope ReleasePlan {
            $previous = $env:CARGO_TARGET_DIR
            Invoke-WithSemverCheckTargetDirectory -TargetDirectory $null -Action {
                $env:CARGO_TARGET_DIR | Should -Be $previous
            }
        }
    }
}

Describe 'CI version validation output' {
    It 'runs only the locked check without GitHub output' {
        $script:seen = $null
        Invoke-ValidateVersions -GitHubOutputPath '' -Base 'base-ref' -Cargo {
            param($Argument)
            $script:seen = $Argument
            $global:LASTEXITCODE = 0
        }
        $script:seen | Should -Be @(
            'run', '-p', 'cargo-release-plan', '--locked', '--',
            'check', '--format', 'github', '--base', 'base-ref'
        )
    }

    It 'writes <Expected> before a failing check and removes its report directory' -ForEach @(
        @{ Json = '[]'; Expected = 'semver_targets=' },
        @{ Json = '["alpha"]'; Expected = 'semver_targets=alpha' },
        @{ Json = '["alpha","beta"]'; Expected = 'semver_targets=alpha beta' }
    ) {
        $githubOutput = Join-Path $TestDrive 'github-output'
        Remove-Item -LiteralPath $githubOutput -Force -ErrorAction SilentlyContinue
        $previousOutput = $env:GITHUB_OUTPUT
        $script:reportDirectory = $null
        Push-Location $TestDrive
        try {
            {
                Invoke-ValidateVersions -GitHubOutputPath $githubOutput -Base base-ref -Cargo {
                    param($Argument)
                    $global:LASTEXITCODE = 0
                    switch ($Argument[5]) {
                        'report' { $script:reportDirectory = $Argument[7] }
                        'semver-targets' {
                            $Argument | Should -Not -Contain '--base'
                            $Json
                        }
                        'check' {
                            Get-Content $githubOutput | Should -Be $Expected
                            $global:LASTEXITCODE = 2
                        }
                        default { throw 'unexpected Cargo operation' }
                    }
                }
            } | Should -Throw
            Get-Content $githubOutput | Should -Be $Expected
            Test-Path $script:reportDirectory | Should -BeFalse
            $env:GITHUB_OUTPUT | Should -Be $previousOutput
        } finally {
            Pop-Location
        }
    }

    It 'cleans up after <Failure> fails without emitting targets or running check' -ForEach @(
        @{ Failure = 'report' }, @{ Failure = 'semver-targets' }
    ) {
        $githubOutput = Join-Path $TestDrive 'failed-github-output'
        Remove-Item -LiteralPath $githubOutput -Force -ErrorAction SilentlyContinue
        $script:reportDirectory = $null
        $script:commands = [Collections.Generic.List[string]]::new()
        $previousOutput = $env:GITHUB_OUTPUT
        Push-Location $TestDrive
        try {
            {
                Invoke-ValidateVersions -GitHubOutputPath $githubOutput -Base 'base-ref' -Cargo {
                    param($Argument)
                    $command = $Argument[5]
                    $script:commands.Add($command)
                    $global:LASTEXITCODE = 0
                    if ($command -eq 'report') {
                        $script:reportDirectory = $Argument[7]
                    }
                    if ($command -eq $Failure) {
                        $global:LASTEXITCODE = 7
                        'unusable output'
                    }
                }
            } | Should -Throw
            $script:commands | Should -Not -Contain 'check'
            Test-Path $script:reportDirectory | Should -BeFalse
            Test-Path $githubOutput | Should -BeFalse
            $env:GITHUB_OUTPUT | Should -Be $previousOutput
        } finally {
            Pop-Location
        }
    }
}

#requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Protects the local and CI validation gates: injected Cargo output is authoritative, failed
# commands cannot emit targets, and compatibility execution preserves exit codes and environment rules.
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'ReleasePlan.psm1') -Force
    $script:previousBase = $env:RELEASE_PLAN_BASE
    $script:previousHistory = $env:RELEASE_PLAN_HISTORY
    $script:previousTarget = $env:RELEASE_PLAN_MERGE_TARGET
    $script:previousActions = $env:GITHUB_ACTIONS
    $env:RELEASE_PLAN_BASE = $null
    $env:RELEASE_PLAN_HISTORY = $null
    $env:RELEASE_PLAN_MERGE_TARGET = $null
    $env:GITHUB_ACTIONS = 'true'
    $global:LASTEXITCODE = 0

    function Get-ContextFixture {
        param([AllowNull()][object] $MergeTarget = $null)

        # Full immutable identities distinguish actual history, target and the tested checkout.
        return [ordered]@{
            schema_version = 2
            repository = 'fixture/release'
            release_branch = 'releases'
            release_history = 'a' * 40
            merge_target = $MergeTarget
            head = 'c' * 40
            workspace_manifest = 'Cargo.toml'
            config_path = '.cargo/release_plan.toml'
            concurrency_group = 'release-fixture'
        }
    }

    function Get-ContextJson {
        param([AllowNull()][object] $MergeTarget = $null)

        ConvertTo-Json -InputObject (Get-ContextFixture -MergeTarget $MergeTarget) -Compress
    }
}

AfterAll {
    $env:RELEASE_PLAN_BASE = $script:previousBase
    $env:RELEASE_PLAN_HISTORY = $script:previousHistory
    $env:RELEASE_PLAN_MERGE_TARGET = $script:previousTarget
    $env:GITHUB_ACTIONS = $script:previousActions
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

Describe 'Release context handoff' {
    It 'lets hosted context own configured history when no explicit history or target is supplied' {
        $script:seen = [Collections.Generic.List[object]]::new()
        $result = Invoke-ValidateVersions -GitHubOutputPath '' -Cargo {
            param($Argument)
            $script:seen.Add($Argument)
            $global:LASTEXITCODE = 0
            if ($Argument[5] -eq 'release-context') { Get-ContextJson }
            else { 'check result' }
        }
        $result | Should -Be 'check result'
        $script:seen.Count | Should -Be 2
        $script:seen[0] | Should -Be @('run', '-p', 'cargo-release-plan', '--locked', '--', 'release-context')
        $script:seen[1] | Should -Be @(
            'run', '-p', 'cargo-release-plan', '--locked', '--',
            'check', '--format', 'github', '--release-history', ('a' * 40)
        )
    }

    It 'uses cached Folo release history for an ordinary local invocation' {
        $previousActions = $env:GITHUB_ACTIONS
        $script:seen = [Collections.Generic.List[object]]::new()
        try {
            $env:GITHUB_ACTIONS = $null
            Invoke-ValidateVersions -GitHubOutputPath '' -Cargo {
                param($Argument)
                $script:seen.Add($Argument)
                $global:LASTEXITCODE = 0
                if ($Argument[5] -eq 'release-context') { Get-ContextJson }
            }
        } finally { $env:GITHUB_ACTIONS = $previousActions }
        $script:seen[0] | Should -Be @(
            'run', '-p', 'cargo-release-plan', '--locked', '--', 'release-context',
            '--release-history', 'origin/main'
        )
        $script:seen[1] | Should -Be @(
            'run', '-p', 'cargo-release-plan', '--locked', '--', 'check', '--format', 'github',
            '--release-history', ('a' * 40)
        )
    }

    It 'does not replace an explicit local target with cached release history' {
        $previousActions = $env:GITHUB_ACTIONS
        try {
            $env:GITHUB_ACTIONS = $null
            Invoke-ValidateVersions -GitHubOutputPath '' -MergeTarget ('b' * 40) -Cargo {
                param($Argument)
                $global:LASTEXITCODE = 0
                if ($Argument[5] -eq 'release-context') {
                    $Argument | Should -Be @(
                        'run', '-p', 'cargo-release-plan', '--locked', '--', 'release-context',
                        '--merge-target', ('b' * 40)
                    )
                    Get-ContextJson -MergeTarget ('b' * 40)
                }
            }
        } finally { $env:GITHUB_ACTIONS = $previousActions }
    }

    It 'surfaces unavailable local history without retrying with a network default' {
        $previousActions = $env:GITHUB_ACTIONS
        $script:calls = 0
        try {
            $env:GITHUB_ACTIONS = $null
            {
                Invoke-ValidateVersions -GitHubOutputPath '' -Cargo {
                    param($Argument)
                    $script:calls++
                    $Argument | Should -Contain 'origin/main'
                    $global:LASTEXITCODE = 7
                }
            } | Should -Throw
        } finally { $env:GITHUB_ACTIONS = $previousActions }
        $script:calls | Should -Be 1
    }

    It 'passes an explicit history override through the canonical argument for <Case>' -ForEach @(
        @{ Case = 'canonical history'; History = 'a' * 40; Base = '' }
        @{ Case = 'legacy alias'; History = ''; Base = 'a' * 40 }
        @{ Case = 'matching canonical and legacy inputs'; History = 'a' * 40; Base = 'a' * 40 }
    ) {
        $script:seen = [Collections.Generic.List[object]]::new()
        Invoke-ValidateVersions -GitHubOutputPath '' -History $History -Base $Base -Cargo {
            param($Argument)
            $script:seen.Add($Argument)
            $global:LASTEXITCODE = 0
            if ($Argument[5] -eq 'release-context') { Get-ContextJson }
        }
        $script:seen[0] | Should -Be @(
            'run', '-p', 'cargo-release-plan', '--locked', '--', 'release-context',
            '--release-history', ('a' * 40)
        )
        $script:seen[1] | Should -Be @(
            'run', '-p', 'cargo-release-plan', '--locked', '--', 'check', '--format', 'github',
            '--release-history', ('a' * 40)
        )
    }

    It 'rejects conflicting history aliases before invoking the tool' {
        {
            Invoke-ValidateVersions -GitHubOutputPath '' -History ('a' * 40) -Base ('b' * 40) `
                -Cargo { throw 'must not invoke' }
        } | Should -Throw '*must not select different release histories*'
    }

    It 'consumes canonical and legacy environment inputs with the same semantics' -ForEach @(
        @{ Variable = 'RELEASE_PLAN_HISTORY' }
        @{ Variable = 'RELEASE_PLAN_BASE' }
    ) {
        $previous = [Environment]::GetEnvironmentVariable($Variable, 'Process')
        $previousTarget = $env:RELEASE_PLAN_MERGE_TARGET
        try {
            Set-Item -LiteralPath "Env:$Variable" -Value ('a' * 40)
            $env:RELEASE_PLAN_MERGE_TARGET = 'b' * 40
            Invoke-ValidateVersions -GitHubOutputPath '' -Cargo {
                param($Argument)
                $global:LASTEXITCODE = 0
                $Argument | Should -Contain '--release-history'
                $Argument | Should -Contain ('a' * 40)
                $Argument | Should -Contain '--merge-target'
                $Argument | Should -Contain ('b' * 40)
                $Argument | Should -Not -Contain '--base'
                if ($Argument[5] -eq 'release-context') { Get-ContextJson -MergeTarget ('b' * 40) }
            }
        } finally {
            Set-Item -LiteralPath "Env:$Variable" -Value $previous
            $env:RELEASE_PLAN_MERGE_TARGET = $previousTarget
        }
    }

    It 'forwards the resolved pair unchanged when target normalization is <Case>' -ForEach @(
        @{ Case = 'an anticipated unmerged parent'; Normalized = 'b' * 40 }
        @{ Case = 'an already-released target'; Normalized = $null }
    ) {
        $script:seen = [Collections.Generic.List[object]]::new()
        $output = Join-Path $TestDrive "pair-output-$Case"
        Push-Location $TestDrive
        try {
            Invoke-ValidateVersions -GitHubOutputPath $output -MergeTarget ('b' * 40) -Cargo {
                param($Argument)
                $script:seen.Add($Argument)
                $global:LASTEXITCODE = 0
                switch ($Argument[5]) {
                    'release-context' { Get-ContextJson -MergeTarget $Normalized }
                    'semver-targets' { '["alpha"]' }
                }
            }
        } finally { Pop-Location }
        $script:seen.Count | Should -Be 4
        $script:seen[0] | Should -Be @(
            'run', '-p', 'cargo-release-plan', '--locked', '--', 'release-context',
            '--merge-target', ('b' * 40)
        )
        $pair = @('--release-history', ('a' * 40))
        if ($null -ne $Normalized) { $pair += @('--merge-target', $Normalized) }
        $script:seen[1] | Should -Be (@(
            'run', '-p', 'cargo-release-plan', '--locked', '--',
            'report', '--out-dir', $script:seen[1][7]
        ) + $pair)
        $script:seen[3] | Should -Be (@(
            'run', '-p', 'cargo-release-plan', '--locked', '--',
            'check', '--format', 'github'
        ) + $pair)
        $script:seen[2] | Should -Not -Contain '--release-history'
        $script:seen[2] | Should -Not -Contain '--merge-target'
        Get-Content -LiteralPath $output | Should -Be 'semver_targets=alpha'
    }

    It 'propagates rejected source/context acquisition before producing a report or CI target' {
        $script:commands = [Collections.Generic.List[string]]::new()
        $output = Join-Path $TestDrive 'rejected-source'
        {
            Invoke-ValidateVersions -GitHubOutputPath $output -MergeTarget 'invalid-source' -Cargo {
                param($Argument)
                $script:commands.Add($Argument[5])
                $Argument | Should -Contain 'invalid-source'
                $global:LASTEXITCODE = 7
                Get-ContextJson
            }
        } | Should -Throw
        $script:commands | Should -Be @('release-context')
        Test-Path -LiteralPath $output | Should -BeFalse
    }

    It 'rejects an invalid context handoff: <Case>' -ForEach @(
        @{ Case = 'malformed JSON' }
        @{ Case = 'null' }
        @{ Case = 'array' }
        @{ Case = 'old schema' }
        @{ Case = 'string schema' }
        @{ Case = 'missing history' }
        @{ Case = 'symbolic history' }
        @{ Case = 'missing target' }
        @{ Case = 'empty target' }
        @{ Case = 'numeric target' }
        @{ Case = 'symbolic target' }
    ) {
        $context = Get-ContextFixture
        switch ($Case) {
            'old schema' { $context.schema_version = 1 }
            'string schema' { $context.schema_version = '2' }
            'missing history' { $context.Remove('release_history') }
            'symbolic history' { $context.release_history = 'origin/releases' }
            'missing target' { $context.Remove('merge_target') }
            'empty target' { $context.merge_target = '' }
            'numeric target' { $context.merge_target = 1 }
            'symbolic target' { $context.merge_target = 'unmerged-parent' }
        }
        $contextJson = switch ($Case) {
            'malformed JSON' { 'not JSON' }
            'null' { 'null' }
            'array' { '[]' }
            default { ConvertTo-Json -InputObject $context -Compress }
        }
        $script:commands = [Collections.Generic.List[string]]::new()
        $output = Join-Path $TestDrive 'invalid-context'
        {
            Invoke-ValidateVersions -GitHubOutputPath $output -Cargo {
                param($Argument)
                $script:commands.Add($Argument[5])
                $global:LASTEXITCODE = 0
                $contextJson
            }
        } | Should -Throw
        $script:commands | Should -Be @('release-context')
        Test-Path -LiteralPath $output | Should -BeFalse
    }
}

Describe 'CI version validation output' {
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
                Invoke-ValidateVersions -GitHubOutputPath $githubOutput -MergeTarget ('b' * 40) -Cargo {
                    param($Argument)
                    $global:LASTEXITCODE = 0
                    switch ($Argument[5]) {
                        'release-context' { Get-ContextJson -MergeTarget ('b' * 40) }
                        'report' { $script:reportDirectory = $Argument[7] }
                        'semver-targets' {
                            $Argument | Should -Not -Contain '--base'
                            $Argument | Should -Not -Contain '--release-history'
                            $Argument | Should -Not -Contain '--merge-target'
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
                Invoke-ValidateVersions -GitHubOutputPath $githubOutput -Cargo {
                    param($Argument)
                    $command = $Argument[5]
                    $script:commands.Add($command)
                    $global:LASTEXITCODE = 0
                    if ($command -eq 'release-context') { Get-ContextJson }
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

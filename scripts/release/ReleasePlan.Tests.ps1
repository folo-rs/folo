#requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Exercises local and queue adapters through the same tool boundary. Rust owns history
# acquisition and target normalization; these tests verify forwarding and failure propagation.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'ReleasePlan.psm1') -Force
    $script:history = 'a' * 40
    $script:target = 'b' * 40
}

Describe 'Release context forwarding' {
    BeforeEach {
        $script:saved = @{}
        foreach ($name in @('GITHUB_ACTIONS', 'RELEASE_PLAN_HISTORY', 'RELEASE_PLAN_BASE', 'RELEASE_PLAN_MERGE_TARGET')) {
            $script:saved[$name] = [Environment]::GetEnvironmentVariable($name)
            [Environment]::SetEnvironmentVariable($name, $null)
        }
        $env:GITHUB_ACTIONS = 'true'
        $script:seen = [Collections.Generic.List[object]]::new()
    }

    AfterEach {
        foreach ($name in $script:saved.Keys) {
            [Environment]::SetEnvironmentVariable($name, $script:saved[$name])
        }
    }

    It 'uses cached history locally and configured discovery when hosted: <Hosted>' -ForEach @(
        @{ Hosted = $false }, @{ Hosted = $true }
    ) {
        $env:GITHUB_ACTIONS = if ($Hosted) { 'true' } else { $null }
        $result = Invoke-ReleaseValidation -Tool {
            param([string[]] $Argument)
            $script:seen.Add($Argument)
            $global:LASTEXITCODE = 0
            if ($Argument[0] -eq 'release-context') {
                @{ schema_version = 2; release_history = $script:history; merge_target = $null } | ConvertTo-Json
            } else { 'checked' }
        }
        $result | Should -Be 'checked'
        $expected = @('release-context', '--config', '.cargo/release_plan.toml')
        if (-not $Hosted) { $expected += @('--release-history', 'origin/main') }
        $script:seen[0] | Should -Be $expected
        $script:seen[1] | Should -Be @('check', '--format', 'github', '--verbose',
            '--config', '.cargo/release_plan.toml', '--release-history', $script:history)
    }

    It 'forwards normalized context and keeps queue readiness narrow: <Normalize>' -ForEach @(
        @{ Normalize = $false }, @{ Normalize = $true }
    ) {
        $env:GITHUB_ACTIONS = $null
        $script:normalized = if ($Normalize) { $null } else { $script:target }
        Invoke-ReleaseValidation -MergeTarget 'parent with spaces' -VersionReadinessOnly -Tool {
            param([string[]] $Argument)
            $script:seen.Add($Argument)
            $global:LASTEXITCODE = 0
            if ($Argument[0] -eq 'release-context') {
                @{ schema_version = 2; release_history = $script:history; merge_target = $script:normalized } | ConvertTo-Json
            }
        }
        $script:seen.Count | Should -Be 2
        $script:seen[0] | Should -Be @('release-context', '--config', '.cargo/release_plan.toml',
            '--merge-target', 'parent with spaces')
        $expected = @('check', '--format', 'github', '--verbose', '--release-history', $script:history)
        if (-not $Normalize) { $expected += @('--merge-target', $script:target) }
        $script:seen[1] | Should -Be $expected
        $script:seen[1] | Should -Not -Contain '--config'
    }

    It 'accepts canonical and legacy history inputs without conflating the target: <Case>' -ForEach @(
        @{ Case = 'canonical'; History = 'release branch'; Base = '' },
        @{ Case = 'alias'; History = ''; Base = 'release branch' },
        @{ Case = 'matching'; History = 'release branch'; Base = 'release branch' }
    ) {
        Invoke-ReleaseValidation -History $History -Base $Base -MergeTarget 'parent' -Tool {
            param([string[]] $Argument)
            $script:seen.Add($Argument)
            $global:LASTEXITCODE = 0
            if ($Argument[0] -eq 'release-context') {
                @{ schema_version = 2; release_history = $script:history; merge_target = $script:target } | ConvertTo-Json
            }
        }
        $script:seen[0] | Should -Be @('release-context', '--config', '.cargo/release_plan.toml',
            '--release-history', 'release branch', '--merge-target', 'parent')
        $script:seen[1] | Should -Contain $script:target
    }

    It 'consumes environment history and target inputs: <Variable>' -ForEach @(
        @{ Variable = 'RELEASE_PLAN_HISTORY' }, @{ Variable = 'RELEASE_PLAN_BASE' }
    ) {
        [Environment]::SetEnvironmentVariable($Variable, 'history-ref')
        $env:RELEASE_PLAN_MERGE_TARGET = 'target-ref'
        Invoke-ReleaseValidation -Tool {
            param([string[]] $Argument)
            $script:seen.Add($Argument)
            $global:LASTEXITCODE = 0
            if ($Argument[0] -eq 'release-context') {
                @{ schema_version = 2; release_history = $script:history; merge_target = $script:target } | ConvertTo-Json
            }
        }
        $script:seen[0] | Should -Contain 'history-ref'
        $script:seen[0] | Should -Contain 'target-ref'
        $script:seen[1] | Should -Contain $script:history
        $script:seen[1] | Should -Contain $script:target
    }

    It 'rejects conflicting history aliases before execution' {
        {
            Invoke-ReleaseValidation -History 'first' -Base 'second' -Tool {
                $script:seen.Add('unexpected invocation')
                throw 'must not execute'
            }
        } | Should -Throw
        $script:seen.Count | Should -Be 0
    }

    It 'propagates a failed <Phase> without fallback or successful partial output' -ForEach @(
        @{ Phase = 'release-context'; Count = 1 }, @{ Phase = 'check'; Count = 2 }
    ) {
        $script:failingPhase = $Phase
        {
            Invoke-ReleaseValidation -Tool {
                param([string[]] $Argument)
                $script:seen.Add($Argument)
                if ($Argument[0] -eq $script:failingPhase) {
                    $global:LASTEXITCODE = 1
                    'partial output'
                } else {
                    $global:LASTEXITCODE = 0
                    @{ schema_version = 2; release_history = $script:history; merge_target = $null } | ConvertTo-Json
                }
            }
        } | Should -Throw
        $script:seen.Count | Should -Be $Count
    }

    It 'rejects invalid context before running check: <Case>' -ForEach @(
        @{ Case = 'malformed JSON'; Json = '{' },
        @{ Case = 'null'; Json = 'null' },
        @{ Case = 'array'; Json = '[]' },
        @{ Case = 'singleton array'; Json = '[{"schema_version":2,"release_history":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","merge_target":null}]' },
        @{ Case = 'missing fields'; Json = '{}' },
        @{ Case = 'old schema'; Json = '{"schema_version":1,"release_history":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","merge_target":null}' },
        @{ Case = 'string schema'; Json = '{"schema_version":"2","release_history":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","merge_target":null}' },
        @{ Case = 'short history'; Json = '{"schema_version":2,"release_history":"main","merge_target":null}' },
        @{ Case = 'numeric history'; Json = '{"schema_version":2,"release_history":42,"merge_target":null}' },
        @{ Case = 'empty target'; Json = '{"schema_version":2,"release_history":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","merge_target":""}' },
        @{ Case = 'noncommit target'; Json = '{"schema_version":2,"release_history":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","merge_target":"parent"}' }
    ) {
        $script:contextJson = $Json
        {
            Invoke-ReleaseValidation -Tool {
                param([string[]] $Argument)
                $script:seen.Add($Argument)
                $global:LASTEXITCODE = 0
                $script:contextJson
            }
        } | Should -Throw
        $script:seen.Count | Should -Be 1
    }
}

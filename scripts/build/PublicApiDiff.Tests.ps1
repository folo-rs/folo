#Requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Exercises producer selection, output classification and failure aggregation without Cargo or Git.
# Ref: ../../.github/workflows/implementation.md#public-api-diff-publication.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot '..\workflow\PublicApiDiff.psm1') -Force
    Import-Module (Join-Path $PSScriptRoot '..\workflow\PublicApiComment.psm1') -Force
    $script:emptyDiff = @'
Removed items from the public API
=================================
(none)

Changed items in the public API
==============================
(none)

Added items to the public API
=============================
(none)
'@
}

Describe 'Public API producer' {
    It 'binds delta Git change detection to the same merge base as its snapshot' {
        $config = "[git]`nremote_branch = `"origin/main`"`n"
        $sha = 'b' * 40
        $result = Get-PublicApiDeltaConfig -Config $config -MergeBaseSha $sha
        $result | Should -Match ([regex]::Escape("remote_branch = `"$sha`""))
        { Get-PublicApiDeltaConfig -Config '[git]' -MergeBaseSha $sha } | Should -Throw
    }

    It 'selects only affected publishable workspace libraries' {
        $metadata = @{
            workspace_members = @('a', 'b', 'c', 'd', 'f', 'g')
            packages = @(
                @{ id = 'a'; name = 'lib-a'; publish = $null; targets = @(@{ kind = @('lib') }) }
                @{ id = 'b'; name = 'private'; publish = @(); targets = @(@{ kind = @('lib') }) }
                @{ id = 'c'; name = 'binary'; publish = $null; targets = @(@{ kind = @('bin') }) }
                @{ id = 'd'; name = 'macro'; publish = @('crates-io'); targets = @(@{ kind = @('proc-macro') }) }
                @{ id = 'e'; name = 'external'; publish = $null; targets = @(@{ kind = @('lib') }) }
                @{ id = 'f'; name = 'new-target'; publish = $null; targets = @(@{ kind = @('lib') }, @{ kind = @('bin') }) }
                @{ id = 'g'; name = 'removed-target'; publish = $null; targets = @(@{ kind = @('bin') }) }
            )
        }
        $baseline = @{
            workspace_members = @('c', 'f-old', 'g', 'deleted')
            packages = @(
                @{ id = 'c'; name = 'binary'; publish = $null; targets = @(@{ kind = @('lib') }) }
                @{ id = 'f-old'; name = 'new-target'; publish = $null; targets = @(@{ kind = @('bin') }) }
                @{ id = 'g'; name = 'removed-target'; publish = $null; targets = @(@{ kind = @('lib') }, @{ kind = @('bin') }) }
                @{ id = 'deleted'; name = 'deleted'; publish = @('crates-io'); targets = @(@{ kind = @('lib') }) }
            )
        }
        @(Select-PublicApiPackage -Affected @('lib-a', 'private', 'binary', 'macro', 'external',
                    'new-target', 'removed-target', 'deleted') -Metadata $metadata `
                -BaselineMetadata $baseline) |
            Should -Be @('lib-a', 'binary', 'macro', 'new-target', 'removed-target', 'deleted')
        @(Select-PublicApiPackage -Affected @() -Metadata $metadata -BaselineMetadata $baseline).Count |
            Should -Be 0
    }

    It 'classifies empty, changed, failed and malformed output' {
        (ConvertTo-PublicApiPackageReport -Name sample -Output $emptyDiff -ExitCode 0).status | Should -Be 'unchanged'
        $changed = $emptyDiff.Replace('(none)', '+pub struct Sample;', 1)
        $report = ConvertTo-PublicApiPackageReport -Name sample -Output $changed -ExitCode 0
        $report.status | Should -Be 'changed'
        $report.output | Should -BeExactly $changed
        (ConvertTo-PublicApiPackageReport -Name sample -Output $changed -ExitCode 1).status | Should -Be 'error'
        (ConvertTo-PublicApiPackageReport -Name sample -Output 'unexpected' -ExitCode 0).status | Should -Be 'error'
        (ConvertTo-PublicApiPackageReport -Name sample -Output ('x' * (2MB + 1)) -ExitCode 0).status | Should -Be 'error'
    }

    It 'represents a newly introduced library API as additions' {
        $output = ConvertTo-PublicApiOneSidedDiff -ApiOutput "pub struct NewType;`npub fn create();" `
            -Direction Added
        $output | Should -Match '(?m)^\+pub struct NewType;$'
        $output | Should -Match '(?m)^\+pub fn create\(\);$'
        (ConvertTo-PublicApiPackageReport -Name sample -Output $output -ExitCode 0).status |
            Should -Be 'changed'
        $empty = ConvertTo-PublicApiOneSidedDiff -ApiOutput '' -Direction Added
        (ConvertTo-PublicApiPackageReport -Name sample -Output $empty -ExitCode 0).status |
            Should -Be 'unchanged'
    }

    It 'represents a removed library API as removals' {
        $output = ConvertTo-PublicApiOneSidedDiff -ApiOutput 'pub struct RemovedType;' -Direction Removed
        $output | Should -Match '(?m)^-pub struct RemovedType;$'
        (ConvertTo-PublicApiPackageReport -Name sample -Output $output -ExitCode 0).status |
            Should -Be 'changed'
    }

    It 'retains all package results after a generation failure' {
        $report = Get-PublicApiReport -Repository example/repo -PullRequest 2 -HeadSha ('a' * 40) `
            -BaseSha ('c' * 40) -MergeBaseSha ('b' * 40) -RunId 3 -RunAttempt 1 `
            -Package @('bad', 'good') -Generate {
            param($name)
            if ($name -eq 'bad') { throw 'tool failed' }
            return @{ Output = $emptyDiff; ErrorOutput = ''; ExitCode = 0 }
        }
        @($report.packages.status) | Should -Be @('error', 'unchanged')
        $json = $report | ConvertTo-Json -Depth 20 | ConvertFrom-Json -AsHashtable
        $json.packages[0].name | Should -Be 'bad'
        $json.base_sha | Should -Be ('c' * 40)
        $json.merge_base_sha | Should -Be ('b' * 40)
    }

    It 'produces an artifact accepted by the trusted publisher' {
        $report = Get-PublicApiReport -Repository example/repo -PullRequest 2 -HeadSha ('a' * 40) `
            -BaseSha ('c' * 40) -MergeBaseSha ('b' * 40) -RunId 3 -RunAttempt 1 `
            -Package @('sample') -Generate {
            return @{ Output = $emptyDiff; ErrorOutput = ''; ExitCode = 0 }
        }
        $path = Join-Path $TestDrive 'public-api-diff.json'
        $report | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath $path -Encoding utf8
        $validated = Read-PublicApiArtifact -Path $path -Repository example/repo -PullRequest 2 `
            -RunId 3 -RunAttempt 1 -HeadSha ('a' * 40)
        $validated.packages[0].status | Should -Be 'unchanged'
    }
}

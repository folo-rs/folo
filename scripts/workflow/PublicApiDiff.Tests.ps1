#Requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Exercises producer selection, output classification and failure aggregation without Cargo or Git.
# Ref: ../../.github/workflows/implementation.md#public-api-diff-publication.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'PublicApiDiff.psm1') -Force
    Import-Module (Join-Path $PSScriptRoot 'PublicApiComment.psm1') -Force
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
    It 'selects only affected publishable workspace libraries' {
        $metadata = @{
            workspace_members = @('a', 'b', 'c', 'd')
            packages = @(
                @{ id = 'a'; name = 'lib-a'; publish = $null; targets = @(@{ kind = @('lib') }) }
                @{ id = 'b'; name = 'private'; publish = @(); targets = @(@{ kind = @('lib') }) }
                @{ id = 'c'; name = 'binary'; publish = $null; targets = @(@{ kind = @('bin') }) }
                @{ id = 'd'; name = 'macro'; publish = @('crates-io'); targets = @(@{ kind = @('proc-macro') }) }
                @{ id = 'e'; name = 'external'; publish = $null; targets = @(@{ kind = @('lib') }) }
            )
        }
        @(Select-PublicApiPackage -Affected @('lib-a', 'private', 'binary', 'macro', 'external') `
                -Metadata $metadata) | Should -Be @('lib-a', 'macro')
        @(Select-PublicApiPackage -Affected @() -Metadata $metadata).Count | Should -Be 0
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

    It 'retains all package results after a generation failure' {
        $report = Get-PublicApiReport -Repository example/repo -PullRequest 2 -HeadSha ('a' * 40) `
            -MergeBaseSha ('b' * 40) -RunId 3 -RunAttempt 1 -Package @('bad', 'good') -Generate {
            param($name)
            if ($name -eq 'bad') { throw 'tool failed' }
            return @{ Output = $emptyDiff; ErrorOutput = ''; ExitCode = 0 }
        }
        @($report.packages.status) | Should -Be @('error', 'unchanged')
        $json = $report | ConvertTo-Json -Depth 20 | ConvertFrom-Json -AsHashtable
        $json.packages[0].name | Should -Be 'bad'
        $json.merge_base_sha | Should -Be ('b' * 40)
    }

    It 'produces an artifact accepted by the trusted publisher' {
        $report = Get-PublicApiReport -Repository example/repo -PullRequest 2 -HeadSha ('a' * 40) `
            -MergeBaseSha ('b' * 40) -RunId 3 -RunAttempt 1 -Package @('sample') -Generate {
            return @{ Output = $emptyDiff; ErrorOutput = ''; ExitCode = 0 }
        }
        $path = Join-Path $TestDrive 'public-api-diff.json'
        $report | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath $path -Encoding utf8
        $validated = Read-PublicApiArtifact -Path $path -Repository example/repo -PullRequest 2 `
            -RunId 3 -RunAttempt 1 -HeadSha ('a' * 40)
        $validated.packages[0].status | Should -Be 'unchanged'
    }
}

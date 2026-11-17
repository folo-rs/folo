#Requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Exercise runner-capacity classification with in-memory GitHub job/annotation evidence.
# API acquisition stays outside these policy tests.
BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'MacOsCapacity.psm1') -Force

    function Get-CapacityJobFixture {
        param([string] $Name, [string] $Conclusion = 'success', [int] $Attempt = 1)
        @{
            name = $Name; status = 'completed'; conclusion = $Conclusion; run_attempt = $Attempt
            labels = @('macos-latest'); runner_id = 0; runner_name = ''; steps = @()
            annotations = @(@{
                    annotation_level = 'failure'
                    message = 'The job was not acquired by Runner of type hosted even after multiple attempts'
                })
        }
    }

    function Get-CanaryJobFixture {
        foreach ($name in @(
                'configuration', 'storage', 'exercise / prepare', 'exercise / analyze',
                'exercise / cbh-collect:reusable-workflow-canary:ubuntu-latest',
                'exercise / cbh-collect:reusable-workflow-canary:windows-latest',
                'exercise / cbh-collect:reusable-workflow-canary:macos-latest',
                'backfill / prepare', 'backfill / backfill (ubuntu-latest)',
                'backfill / backfill (windows-latest)', 'backfill / backfill (macos-latest)',
                'rolling-backfill / prepare', 'rolling-backfill / backfill (ubuntu-latest)',
                'no-eligible-backfill / prepare', 'verify-rolling-backfill', 'verify-no-work',
                'verify-backfill', 'verify')) {
            Get-CapacityJobFixture $name
        }
        foreach ($name in @('exercise / preflight', 'exercise / empty-scope',
                'exercise / failed', 'no-eligible-backfill / backfill')) {
            Get-CapacityJobFixture $name skipped
        }
    }
}

Describe 'macOS runner-capacity exceptions' {
    BeforeEach {
        $script:matrix = @(
            Get-CapacityJobFixture 'clippy-dev (ubuntu-latest)'
            Get-CapacityJobFixture 'clippy-dev (windows-latest)'
            Get-CapacityJobFixture 'clippy-dev (macos-latest)' cancelled
        )
    }

    It 'allows only the matrix whose macOS leg never acquired a runner' {
        Get-OptionalMacOsDependency -Policy queue -Job $matrix | Should -Be 'clippy-dev'
        $matrix[2].conclusion = 'failure'
        Get-OptionalMacOsDependency -Policy queue -Job $matrix | Should -BeNullOrEmpty
    }

    It 'rejects incomplete or contradictory capacity evidence: <Case>' -ForEach @(
        @{ Case = 'executed step'; Field = 'steps'; Value = @(@{ conclusion = 'cancelled' }) },
        @{ Case = 'allocated runner'; Field = 'runner_id'; Value = 123 },
        @{ Case = 'named runner'; Field = 'runner_name'; Value = 'Hosted Agent' },
        @{ Case = 'different runner'; Field = 'labels'; Value = @('ubuntu-latest') },
        @{ Case = 'unfinished job'; Field = 'status'; Value = 'queued' },
        @{ Case = 'absent annotations'; Field = 'annotations'; Value = @() },
        @{ Case = 'manual cancellation'; Field = 'annotations'; Value = @(@{
                    annotation_level = 'failure'; message = 'The operation was canceled.'
                }) },
        @{ Case = 'notice without failure'; Field = 'annotations'; Value = @(@{
                    annotation_level = 'notice'
                    message = 'Due to capacity constraints, jobs targeting macOS arm64 runners may experience longer queue times.'
                }) },
        @{ Case = 'mixed failures'; Field = 'annotations'; Value = @(
                @{ annotation_level = 'failure'
                    message = 'The job was not acquired by Runner of type hosted even after multiple attempts' },
                @{ annotation_level = 'failure'; message = 'An independent failure' }
            ) }
    ) {
        $matrix[2][$Field] = $Value
        Get-OptionalMacOsDependency -Policy queue -Job $matrix | Should -BeNullOrEmpty
    }

    It 'accepts the real accompanying capacity notices but rejects unknown annotations' {
        $matrix[2].annotations += @{
            annotation_level = 'notice'
            message = 'Due to capacity constraints, jobs targeting macOS arm64 runners may experience longer queue times.'
        }
        Get-OptionalMacOsDependency -Policy queue -Job $matrix | Should -Be 'clippy-dev'
        $matrix[2].annotations += @{ annotation_level = 'warning'; message = 'Unrelated warning' }
        Get-OptionalMacOsDependency -Policy queue -Job $matrix | Should -BeNullOrEmpty
    }

    It 'does not excuse a non-mac failure or an absent matrix leg' {
        $matrix[0].conclusion = 'failure'
        Get-OptionalMacOsDependency -Policy queue -Job $matrix | Should -BeNullOrEmpty
        Get-OptionalMacOsDependency -Policy queue -Job @($matrix[1], $matrix[2]) |
            Should -BeNullOrEmpty
    }

    It 'retains unretried results and lets a newer actual failure supersede capacity' {
        $matrix += Get-CapacityJobFixture 'clippy-dev (macos-latest)' failure 2
        Get-OptionalMacOsDependency -Policy queue -Job $matrix | Should -BeNullOrEmpty
        $matrix[3].conclusion = 'cancelled'
        Get-OptionalMacOsDependency -Policy queue -Job $matrix | Should -Be 'clippy-dev'
    }

    It 'rejects ambiguous executions within one attempt' {
        $matrix += Get-CapacityJobFixture 'clippy-dev (macos-latest)' cancelled
        { Get-OptionalMacOsDependency -Policy queue -Job $matrix } | Should -Throw
    }
}

Describe 'caller canary capacity dependencies' {
    BeforeEach {
        $script:canary = @(Get-CanaryJobFixture)
        ($canary | Where-Object name -CEQ 'exercise / cbh-collect:reusable-workflow-canary:macos-latest').conclusion = 'cancelled'
        ($canary | Where-Object name -CEQ 'exercise / analyze').conclusion = 'skipped'
        ($canary | Where-Object name -CEQ 'verify').conclusion = 'skipped'
    }

    Context 'authenticated workflow evidence' {
            BeforeEach {
                $script:head = 'a' * 40
                $script:run = @{
                    id = 42; repository = @{ full_name = 'folo-rs/folo' }
                    head_sha = $head; run_attempt = 2; check_suite_id = 19
                }
                $script:pages = @(@{ jobs = @(
                            @{
                                name = 'standard / clippy-dev (macos-latest)'
                                status = 'completed'; conclusion = 'cancelled'; run_attempt = 2
                                labels = @('macos-latest'); steps = @(); runner_id = 0; runner_name = ''
                                check_run_url = 'https://api.github.com/repos/folo-rs/folo/check-runs/123'
                            },
                            @{ name = 'standard / required-checks'; status = 'in_progress'; run_attempt = 2 }
                        ) })
                $script:check = @{
                    name = 'standard / clippy-dev (macos-latest)'; head_sha = $head
                    check_suite = @{ id = 19 }; app = @{ id = 15368; slug = 'github-actions' }
                    status = 'completed'; conclusion = 'cancelled'; output = @{ annotations_count = 1 }
                }
                Mock gh -ModuleName MacOsCapacity {
                    $global:LASTEXITCODE = 0
                    $endpoint = [string] $args[-1]
                    switch -Regex ($endpoint) {
                        '/jobs\?' { ConvertTo-Json -InputObject $script:pages -Depth 10 }
                        '/annotations\?' {
                            ConvertTo-Json -InputObject @(@(@{
                                        annotation_level = 'failure'
                                        message = 'The job was not acquired by Runner of type hosted even after multiple attempts'
                                    })) -Depth 5
                        }
                        '/check-runs/123$' { ConvertTo-Json -InputObject $script:check -Depth 5 }
                        '/runs/42$' { ConvertTo-Json -InputObject $script:run -Depth 5 }
                        default { throw "Unexpected test API request: $endpoint" }
                    }
                }
            }

            It 'retains caller scope and reads the matching check annotations' {
                $jobs = @(Get-RunnerCapacityJob -Repository folo-rs/folo -RunId 42 -RunAttempt 2 `
                        -HeadSha $head -GateName required-checks)
                $jobs.Count | Should -Be 1
                $jobs[0].name | Should -Be 'clippy-dev (macos-latest)'
                $jobs[0].annotations.Count | Should -Be 1
            }

            It 'rejects a stale head before granting any exception' {
                $run.head_sha = 'b' * 40
                { Get-RunnerCapacityJob -Repository folo-rs/folo -RunId 42 -RunAttempt 2 `
                        -HeadSha $head -GateName required-checks } | Should -Throw
            }

            It 'rejects mismatched check ownership and incomplete annotations' -ForEach @('owner', 'count') {
                if ($_ -ceq 'owner') { $check.app.id = 1 }
                else { $check.output.annotations_count = 2 }
                { Get-RunnerCapacityJob -Repository folo-rs/folo -RunId 42 -RunAttempt 2 `
                        -HeadSha $head -GateName required-checks } | Should -Throw
        }
    }

    It 'allows only the proven unavailable collection and its skipped verifier' {
        @(Get-OptionalMacOsDependency -Policy canary -Job $canary) |
            Should -Be @('exercise', 'verify')
    }

    It 'keeps actual analysis or verification failures blocking' -ForEach @(
        'exercise / analyze', 'verify', 'verify-backfill', 'verify-no-work'
    ) {
        ($canary | Where-Object name -CEQ $_).conclusion = 'failure'
        Get-OptionalMacOsDependency -Policy canary -Job $canary | Should -BeNullOrEmpty
    }

    It 'rejects an unrelated skipped verifier even with a capacity cancellation' {
        ($canary | Where-Object name -CEQ 'verify-backfill').conclusion = 'skipped'
        Get-OptionalMacOsDependency -Policy canary -Job $canary | Should -BeNullOrEmpty
    }

    It 'requires all non-mac collections and complete known caller evidence' {
        ($canary | Where-Object name -CEQ 'exercise / cbh-collect:reusable-workflow-canary:windows-latest').conclusion = 'cancelled'
        Get-OptionalMacOsDependency -Policy canary -Job $canary | Should -BeNullOrEmpty
        $canary = @($canary | Where-Object name -CNE 'storage')
        Get-OptionalMacOsDependency -Policy canary -Job $canary | Should -BeNullOrEmpty
    }

    It 'permits backfill verifier skips only after its own confirmed capacity loss' {
        ($canary | Where-Object name -CEQ 'backfill / backfill (macos-latest)').conclusion = 'cancelled'
        ($canary | Where-Object name -CEQ 'verify-backfill').conclusion = 'skipped'
        @(Get-OptionalMacOsDependency -Policy canary -Job $canary) |
            Should -Be @('exercise', 'verify', 'backfill', 'verify-backfill')
    }

    It 'requires the successful internal result when classifying the outer reusable call' {
        $outer = @($canary | ForEach-Object {
                $copy = $_.Clone(); $copy.name = "benchmark-canary / $($copy.name)"; $copy
            })
        Get-OptionalMacOsDependency -Policy standard -Job $outer | Should -BeNullOrEmpty
        $outer += Get-CapacityJobFixture 'benchmark-canary / result'
        Get-OptionalMacOsDependency -Policy standard -Job $outer | Should -Be 'benchmark-canary'
    }

    It 'accepts an unexpanded no-work matrix only with successful preparation and verification' {
        $canary = @($canary | Where-Object name -CNE 'no-eligible-backfill / backfill')
        @(Get-OptionalMacOsDependency -Policy canary -Job $canary) |
            Should -Be @('exercise', 'verify')
        $canary = @($canary | Where-Object name -CNE 'verify-no-work')
        Get-OptionalMacOsDependency -Policy canary -Job $canary | Should -BeNullOrEmpty
    }
}

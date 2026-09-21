#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Proves Folo's shared collection policy, stability flags and exported backfill window
# selection used by the thin reusable-workflow callers without a workflow run.
#
# The nightly backfill's rolling date window is proven the same way: `git` is isolated behind the
# module's Invoke-GitCapture boundary and mocked here in the module's scope, so the window resolution
# (including the quiet-window fallback and the nothing-eligible exit) is exercised against
# canned `rev-list` output rather than a real repository, whose history would change under the
# suite. Policy isolation keeps one caller from changing the settings supplied to another.

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'BenchHistoryCollect.psm1') -Force

    # Fixture policy tests configuration consumption, not the repository's current choices.
    # Unsorted exclusions expose reordering or truncation; distinct windows expose swapped inputs.
    $script:OriginalConfiguration = InModuleScope BenchHistoryCollect {
        $original = @{
            ExcludedPackages = $script:ExcludedPackages
            BackfillQuarantine = $script:BackfillQuarantine
            BackfillHorizon = $script:BackfillHorizon
        }
        $script:ExcludedPackages = @('excluded-z', 'excluded-a', 'excluded-m')
        $script:BackfillQuarantine = '2 days ago'
        $script:BackfillHorizon = '7 days ago'
        $original
    }

    # The canned `git rev-list` output the mocked window queries return: the range end is the newest
    # first-parent commit outside the quarantine and the range start the oldest one inside the
    # configured horizon, with one more commit between them. Full 40-character object ids, as git prints
    # them.
    $script:WindowEnd = 'a' * 40
    $script:WindowStart = 'c' * 40
}

AfterAll {
    InModuleScope BenchHistoryCollect -Parameters @{ Configuration = $script:OriginalConfiguration } {
        param($Configuration)
        $script:ExcludedPackages = $Configuration.ExcludedPackages
        $script:BackfillQuarantine = $Configuration.BackfillQuarantine
        $script:BackfillHorizon = $Configuration.BackfillHorizon
    }
}

Describe 'Get-BenchHistoryRustFlag' {
    It 'replaces alignment spellings and preserves unrelated flags' -ForEach @(
        @{ Existing = '-Copt-level=2 -Cllvm-args=-align-all-functions=3 -g' }
        @{ Existing = '-Copt-level=2 -C llvm-args=-align-all-functions=3 -g' }
        @{ Existing = '-Copt-level=2 --codegen=llvm-args=-align-all-functions=3 -g' }
        @{ Existing = '-Copt-level=2 --codegen llvm-args=-align-all-functions=3 -g' }
    ) {
        Get-BenchHistoryRustFlag -Existing $Existing -Stability '-Cllvm-args=-align-all-functions=6' |
            Should -Be '-Copt-level=2 -g -Cllvm-args=-align-all-functions=6'
    }

    It 'supports an initially empty flag set' {
        Get-BenchHistoryRustFlag -Existing '' -Stability 'configured-stability' | Should -Be 'configured-stability'
    }
}

Describe 'Get-BenchHistoryCollectionPolicy' {
    It 'returns isolated policy values for reusable callers' {
        $policy = Get-BenchHistoryCollectionPolicy
        $policy.ExcludedPackages | Should -Be @('excluded-z', 'excluded-a', 'excluded-m')
        $bestOf = $policy.BestOf
        $allFeatures = $policy.AllFeatures
        $policy.ExcludedPackages[0] = 'caller-modification'
        $policy.BestOf++
        $policy.AllFeatures = -not $policy.AllFeatures
        $fresh = Get-BenchHistoryCollectionPolicy
        $fresh.ExcludedPackages | Should -Be @('excluded-z', 'excluded-a', 'excluded-m')
        $fresh.BestOf | Should -Be $bestOf
        $fresh.AllFeatures | Should -Be $allFeatures
    }
}

Describe 'Get-BenchHistoryBackfillWindow' {
    Context 'the rolling date window (mocked git rev-list)' {
        BeforeEach {
            # The `-1` query resolves the range end (the newest first-parent commit outside the
            # quarantine); the other query lists the first-parent commits within the horizon of it,
            # newest first, so its LAST line is the range start.
            Mock git -ModuleName BenchHistoryCollect {
                $global:LASTEXITCODE = 0
                if ($args -contains '-1') {
                    'a' * 40
                } else {
                    @(('a' * 40), ('b' * 40), ('c' * 40))
                }
            }
        }

        It 'returns the inclusive endpoints for the reusable caller' {
            $result = Get-BenchHistoryBackfillWindow
            $result.From | Should -Be $script:WindowStart
            $result.To | Should -Be $script:WindowEnd
        }

        It 'quarantines the range end from the push-triggered collection' {
            Get-BenchHistoryBackfillWindow | Out-Null
            Should -Invoke git -ModuleName BenchHistoryCollect -Times 1 -Exactly -ParameterFilter {
                ($args -contains '-1') -and ($args -contains '--before=2 days ago') -and
                ($args -contains 'HEAD')
            }
        }

        It 'resolves the range start from the range end rather than from HEAD' {
            # `backfill` hard-errors unless the range start is a first-parent ancestor of the range
            # end, which resolving from the end is what guarantees.
            Get-BenchHistoryBackfillWindow | Out-Null
            Should -Invoke git -ModuleName BenchHistoryCollect -Times 1 -Exactly -ParameterFilter {
                ($args -contains '--since=7 days ago') -and ($args -contains ('a' * 40)) -and
                ($args -notcontains 'HEAD')
            }
        }

        It 'restricts every history query to the first-parent line' {
            Get-BenchHistoryBackfillWindow | Out-Null
            Should -Invoke git -ModuleName BenchHistoryCollect -Times 2 -Exactly -ParameterFilter {
                $args -contains '--first-parent'
            }
        }
    }

    Context 'a quiet window (the range end predates the horizon)' {
        BeforeEach {
            Mock git -ModuleName BenchHistoryCollect {
                $global:LASTEXITCODE = 0
                if ($args -contains '-1') { 'a' * 40 } else { @() }
            }
        }

        It 'collapses the window onto the single eligible commit' {
            $result = Get-BenchHistoryBackfillWindow
            $result.From | Should -Be $script:WindowEnd
            $result.To | Should -Be $script:WindowEnd
        }

        It 'also collapses an override older than the horizon onto that commit' {
            $result = Get-BenchHistoryBackfillWindow -ToCommitId 'abc1234'
            $result.From | Should -Be 'abc1234'
            $result.To | Should -Be 'abc1234'
        }
    }

    Context 'nothing eligible yet (every commit is inside the quarantine)' {
        BeforeEach {
            Mock git -ModuleName BenchHistoryCollect {
                $global:LASTEXITCODE = 0
                @()
            }
        }

        It 'returns no window and explains the no-op' {
            $result = @(Get-BenchHistoryBackfillWindow -Verbose 4>&1)
            $result | Where-Object { $_ -isnot [System.Management.Automation.VerboseRecord] } |
                Should -BeNullOrEmpty
            @($result | Where-Object { $_ -is [System.Management.Automation.VerboseRecord] }).Count |
                Should -BeGreaterThan 0
        }

        It 'does not query the window once the range end came back empty' {
            Get-BenchHistoryBackfillWindow | Out-Null
            Should -Invoke git -ModuleName BenchHistoryCollect -Times 1 -Exactly
        }
    }

    Context 'an operator-supplied range end' {
        BeforeEach {
            Mock git -ModuleName BenchHistoryCollect {
                $global:LASTEXITCODE = 0
                @(('d' * 40), ('e' * 40))
            }
        }

        It 'uses the given commit as the range end' {
            $result = Get-BenchHistoryBackfillWindow -ToCommitId 'abc1234'
            $result.From | Should -Be ('e' * 40)
            $result.To | Should -Be 'abc1234'
        }

        It 'bypasses the quarantine computation' {
            Get-BenchHistoryBackfillWindow -ToCommitId 'abc1234' | Out-Null
            Should -Invoke git -ModuleName BenchHistoryCollect -Times 0 -Exactly -ParameterFilter {
                $args -contains '-1'
            }
        }

        It 'still bounds the range start by the horizon' {
            Get-BenchHistoryBackfillWindow -ToCommitId 'abc1234' | Out-Null
            Should -Invoke git -ModuleName BenchHistoryCollect -Times 1 -Exactly -ParameterFilter {
                ($args -contains '--since=7 days ago') -and ($args -contains 'abc1234')
            }
        }

        It 'trims surrounding whitespace before use' {
            $result = Get-BenchHistoryBackfillWindow -ToCommitId '  abc1234  '
            $result.To | Should -Be 'abc1234'
        }

        It 'treats a blank id as no override' -ForEach @(
            @{ Override = $null }
            @{ Override = '' }
            @{ Override = '  ' }
        ) {
            Get-BenchHistoryBackfillWindow -ToCommitId $Override | Out-Null
            Should -Invoke git -ModuleName BenchHistoryCollect -Times 1 -Exactly -ParameterFilter {
                $args -contains '-1'
            }
        }

        It 'rejects invalid overrides before querying git' -ForEach @(
            @{ Override = 'HEAD~1' }
            @{ Override = '--all' }
            @{ Override = 'abc1234; invalid-command' }
            @{ Override = "abc1234`nabcdef0" }
        ) {
            { Get-BenchHistoryBackfillWindow -ToCommitId $Override } | Should -Throw
            Should -Invoke git -ModuleName BenchHistoryCollect -Times 0 -Exactly
        }
    }

    Context 'a failing git query' {
        BeforeEach {
            Mock git -ModuleName BenchHistoryCollect {
                $global:LASTEXITCODE = 1
                'fatal: bad revision'
            }
        }

        It 'fails loudly instead of backfilling an unresolved range' {
            { Get-BenchHistoryBackfillWindow } | Should -Throw
        }

        It 'rejects an unresolved override instead of treating it as a quiet window' {
            { Get-BenchHistoryBackfillWindow -ToCommitId 'abc1234' } | Should -Throw
        }
    }

    Context 'git output validation' {
        It 'ignores blank lines and trims the resolved endpoints' {
            Mock git -ModuleName BenchHistoryCollect {
                $global:LASTEXITCODE = 0
                if ($args -contains '-1') {
                    @('', " $('a' * 40) ", ' ')
                } else {
                    @('', ('a' * 40), " $('c' * 40) ", ' ')
                }
            }
            $result = Get-BenchHistoryBackfillWindow
            $result.From | Should -Be $script:WindowStart
            $result.To | Should -Be $script:WindowEnd
        }

        It 'rejects a malformed range end' {
            Mock git -ModuleName BenchHistoryCollect {
                $global:LASTEXITCODE = 0
                if ($args -contains '-1') { 'not-a-commit' } else { @() }
            }
            { Get-BenchHistoryBackfillWindow } | Should -Throw
        }

        It 'rejects a malformed range start' {
            Mock git -ModuleName BenchHistoryCollect {
                $global:LASTEXITCODE = 0
                if ($args -contains '-1') { 'a' * 40 } else { 'not-a-commit' }
            }
            { Get-BenchHistoryBackfillWindow } | Should -Throw
        }
    }
}

Describe 'Exclusion configuration' {
    It 'honors <Name> exclusions in the policy shared by all callers' -ForEach @(
        @{
            Name = 'empty'
            Exclusions = @()
        }
        @{
            Name = 'replacement'
            Exclusions = @('alternate')
        }
    ) {
        InModuleScope BenchHistoryCollect -Parameters @{
            Exclusions = $Exclusions
        } {
            param($Exclusions)
            $previous = $script:ExcludedPackages
            try {
                $script:ExcludedPackages = $Exclusions
                (Get-BenchHistoryCollectionPolicy).ExcludedPackages | Should -Be $Exclusions
            } finally {
                $script:ExcludedPackages = $previous
            }
        }
    }
}

#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Proves Folo's shared collection policy, stability flags and retained backfill argument
# selection without a workflow run.
#
# The nightly backfill's rolling date window is proven the same way: `git` is isolated behind the
# module's Invoke-GitCapture boundary and mocked here in the module's scope, so the window resolution
# (including the quiet-window fallback and the nothing-eligible exit) is exercised against
# canned `rev-list` output rather than a real repository, whose history would change under the
# suite. The scope-identity case is what keeps a backfilled point measured exactly like a pushed
# one.

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

    # Collection and backfill must use the same scope, feature and noise-reduction policy.
    $script:Scope = @(
        '--workspace',
        '--exclude', 'excluded-z',
        '--exclude', 'excluded-a',
        '--exclude', 'excluded-m',
        '--all-features',
        '--best-of', '3',
        '--verbose'
    )

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
        $policy.ExcludedPackages[0] = 'caller-modification'
        $policy.BestOf = 99
        $fresh = Get-BenchHistoryCollectionPolicy
        $fresh.ExcludedPackages | Should -Be @('excluded-z', 'excluded-a', 'excluded-m')
        $fresh.BestOf | Should -Not -Be $policy.BestOf
    }
}

Describe 'Get-BenchHistoryBackfillCommand' {
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

        It 'backfills the whole window with skip-existing and walks past failing commits' {
            $result = Get-BenchHistoryBackfillCommand
            $result | Should -Be (@('backfill', $script:WindowStart, $script:WindowEnd) +
                $script:Scope + @('--ignore-errors'))
        }

        It 'never overwrites an already-stored point' {
            $result = Get-BenchHistoryBackfillCommand
            $result | Should -Not -Contain '--overwrite'
        }

        It 'measures with the policy supplied to the reusable callers' {
            $policy = Get-BenchHistoryCollectionPolicy
            $expected = @('--workspace')
            foreach ($excluded in $policy.ExcludedPackages) { $expected += @('--exclude', $excluded) }
            if ($policy.AllFeatures) { $expected += '--all-features' }
            $expected += @('--best-of', [string] $policy.BestOf, '--verbose')
            $backfill = Get-BenchHistoryBackfillCommand
            $backfillScope = $backfill[3..($backfill.Count - 2)]
            $backfillScope | Should -Be $expected
        }

        It 'quarantines the range end from the push-triggered collection' {
            Get-BenchHistoryBackfillCommand | Out-Null
            Should -Invoke git -ModuleName BenchHistoryCollect -Times 1 -Exactly -ParameterFilter {
                ($args -contains '-1') -and ($args -contains '--before=2 days ago') -and
                ($args -contains 'HEAD')
            }
        }

        It 'resolves the range start from the range end rather than from HEAD' {
            # `backfill` hard-errors unless the range start is a first-parent ancestor of the range
            # end, which resolving from the end is what guarantees.
            Get-BenchHistoryBackfillCommand | Out-Null
            Should -Invoke git -ModuleName BenchHistoryCollect -Times 1 -Exactly -ParameterFilter {
                ($args -contains '--since=7 days ago') -and ($args -contains ('a' * 40)) -and
                ($args -notcontains 'HEAD')
            }
        }

        It 'restricts every history query to the first-parent line' {
            Get-BenchHistoryBackfillCommand | Out-Null
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
            $result = Get-BenchHistoryBackfillCommand
            $result | Should -Be (@('backfill', $script:WindowEnd, $script:WindowEnd) +
                $script:Scope + @('--ignore-errors'))
        }
    }

    Context 'nothing eligible yet (every commit is inside the quarantine)' {
        BeforeEach {
            Mock git -ModuleName BenchHistoryCollect {
                $global:LASTEXITCODE = 0
                @()
            }
        }

        It 'emits no command at all' {
            @(Get-BenchHistoryBackfillCommand).Count | Should -Be 0
        }

        It 'does not query the window once the range end came back empty' {
            Get-BenchHistoryBackfillCommand | Out-Null
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
            $result = Get-BenchHistoryBackfillCommand -ToCommitId 'abc1234'
            $result | Should -Be (@('backfill', ('e' * 40), 'abc1234') + $script:Scope +
                @('--ignore-errors'))
        }

        It 'bypasses the quarantine computation' {
            Get-BenchHistoryBackfillCommand -ToCommitId 'abc1234' | Out-Null
            Should -Invoke git -ModuleName BenchHistoryCollect -Times 0 -Exactly -ParameterFilter {
                $args -contains '-1'
            }
        }

        It 'still bounds the range start by the horizon' {
            Get-BenchHistoryBackfillCommand -ToCommitId 'abc1234' | Out-Null
            Should -Invoke git -ModuleName BenchHistoryCollect -Times 1 -Exactly -ParameterFilter {
                ($args -contains '--since=7 days ago') -and ($args -contains 'abc1234')
            }
        }

        It 'trims surrounding whitespace before use' {
            $result = Get-BenchHistoryBackfillCommand -ToCommitId '  abc1234  '
            $result[2] | Should -Be 'abc1234'
        }

        It 'treats an empty id as no override' {
            Get-BenchHistoryBackfillCommand -ToCommitId '' | Out-Null
            Should -Invoke git -ModuleName BenchHistoryCollect -Times 1 -Exactly -ParameterFilter {
                $args -contains '-1'
            }
        }

        It 'rejects a ref expression such as HEAD~1' {
            { Get-BenchHistoryBackfillCommand -ToCommitId 'HEAD~1' } | Should -Throw '*hex commit SHA*'
        }

        It 'rejects an id carrying shell metacharacters' {
            { Get-BenchHistoryBackfillCommand -ToCommitId 'abc1234; rm -rf /' } |
                Should -Throw '*hex commit SHA*'
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
            { Get-BenchHistoryBackfillCommand } | Should -Throw '*failed (exit 1)*'
        }
    }
}

Describe 'Exclusion configuration' {
    It 'honors <Name> exclusions in shared policy and backfill' -ForEach @(
        @{
            Name = 'empty'
            Exclusions = @()
            WorkspaceArguments = @('--workspace')
        }
        @{
            Name = 'replacement'
            Exclusions = @('alternate')
            WorkspaceArguments = @('--workspace', '--exclude', 'alternate')
        }
    ) {
        InModuleScope BenchHistoryCollect -Parameters @{
            Exclusions = $Exclusions
            WorkspaceArguments = $WorkspaceArguments
        } {
            param($Exclusions, $WorkspaceArguments)
            $previous = $script:ExcludedPackages
            try {
                $script:ExcludedPackages = $Exclusions
                (Get-BenchHistoryCollectionPolicy).ExcludedPackages | Should -Be $Exclusions
                Get-BenchHistoryScopeArgument | Should -Be (
                    $WorkspaceArguments + @('--all-features', '--best-of', '3', '--verbose'))
            } finally {
                $script:ExcludedPackages = $previous
            }
        }
    }
}

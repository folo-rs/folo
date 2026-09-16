#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Pester suite for BenchHistoryCollect.psm1. Proves the mode selection the bench-history `collect`
# step depends on - append vs. overwrite, and the untrusted-input validation - without a workflow
# run: each case asserts the exact argument vector the step would hand the tool.
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

    # Flags shared by both modes; asserted as a slice so a case only spells out what makes it
    # distinct (the subcommand, its positionals, and the append/overwrite tail).
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

Describe 'Get-BenchHistoryCollectCommand' {
    Context 'separate automation and measured checkouts' {
        It 'keeps local storage, measured repository and current configuration as distinct arguments' {
            $result = Get-BenchHistoryCollectCommand -Package @('measured') `
                -LocalStore "temporary store's data" -Repository 'measured checkout' -ConfigPath 'automation config.toml'
            $result | Should -Contain "--local=temporary store's data"
            $result[[Array]::IndexOf($result, '--repo') + 1] | Should -Be 'measured checkout'
            $result[[Array]::IndexOf($result, '--config') + 1] | Should -Be 'automation config.toml'
            $result | Should -Contain '--skip-existing'
        }

        It 'rejects an explicitly empty store instead of selecting cloud storage' {
            { Get-BenchHistoryCollectCommand -Package @('measured') -LocalStore '' } | Should -Throw
        }

        It 'rejects an explicitly empty measured repository' {
            { Get-BenchHistoryCollectCommand -Package @('measured') -Repository ' ' } | Should -Throw
        }
    }

    Context 'append mode (no recollect commit id)' {
        It 'collects the pushed commit in append mode for an empty id' {
            $result = Get-BenchHistoryCollectCommand -RecollectCommitId ''
            $result | Should -Be (@('collect') + $script:Scope + @('--skip-existing'))
        }


        It 'treats a null id as append mode' {
            $result = Get-BenchHistoryCollectCommand -RecollectCommitId $null
            $result | Should -Be (@('collect') + $script:Scope + @('--skip-existing'))
        }

        It 'treats a whitespace-only id as append mode' {
            $result = Get-BenchHistoryCollectCommand -RecollectCommitId "  `t "
            $result | Should -Be (@('collect') + $script:Scope + @('--skip-existing'))
        }

        It 'never overwrites in append mode' {
            $result = Get-BenchHistoryCollectCommand -RecollectCommitId ''
            $result | Should -Not -Contain '--overwrite'
            $result | Should -Not -Contain 'backfill'
        }
    }

    Context 'recollect mode (a commit id set)' {
        It 'overwrites a single historical commit via backfill for a full SHA' {
            $sha = '0123456789abcdef0123456789abcdef01234567'
            $result = Get-BenchHistoryCollectCommand -RecollectCommitId $sha
            $result | Should -Be (@('backfill', $sha, $sha) + $script:Scope + @('--overwrite'))
        }

        It 'accepts a short SHA and passes it as both range endpoints' {
            $result = Get-BenchHistoryCollectCommand -RecollectCommitId 'abc1234'
            $result | Should -Be (@('backfill', 'abc1234', 'abc1234') + $script:Scope + @('--overwrite'))
        }

        It 'trims surrounding whitespace before use' {
            $result = Get-BenchHistoryCollectCommand -RecollectCommitId '  abc1234  '
            $result | Should -Be (@('backfill', 'abc1234', 'abc1234') + $script:Scope + @('--overwrite'))
        }

        It 'never appends in recollect mode' {
            $result = Get-BenchHistoryCollectCommand -RecollectCommitId 'abc1234'
            $result | Should -Not -Contain '--skip-existing'
            $result | Should -Not -Contain 'collect'
        }
    }

    Context 'invalid commit ids' {
        It 'rejects a value shorter than 7 characters' {
            { Get-BenchHistoryCollectCommand -RecollectCommitId 'abc123' } | Should -Throw '*hex commit SHA*'
        }

        It 'rejects a value longer than 40 characters' {
            $tooLong = '0' * 41
            { Get-BenchHistoryCollectCommand -RecollectCommitId $tooLong } | Should -Throw '*hex commit SHA*'
        }

        It 'rejects non-hex characters' {
            { Get-BenchHistoryCollectCommand -RecollectCommitId 'abcdefg' } | Should -Throw '*hex commit SHA*'
        }

        It 'rejects a ref expression such as HEAD~1' {
            { Get-BenchHistoryCollectCommand -RecollectCommitId 'HEAD~1' } | Should -Throw '*hex commit SHA*'
        }

        It 'rejects an id carrying shell metacharacters' {
            { Get-BenchHistoryCollectCommand -RecollectCommitId "abc1234; rm -rf /" } | Should -Throw '*hex commit SHA*'
        }
    }

    Context 'package scoping (PR workflow)' {
        It 'collects only retained packages after the PR delta filter' {
            $packages = @(Select-BenchmarkablePackage -Package @(
                    'excluded-m', 'retained-z', 'excluded-z', 'retained-a', 'excluded-a'))
            $result = Get-BenchHistoryCollectCommand -Package $packages
            $result | Should -Be @(
                'collect',
                '--package', 'retained-z',
                '--package', 'retained-a',
                '--all-features',
                '--best-of', '3',
                '--verbose',
                '--skip-existing'
            )
        }

        It 'scopes to the given packages with repeated --package instead of --workspace' {
            $result = Get-BenchHistoryCollectCommand -RecollectCommitId '' -Package @('nm', 'many_cpus')
            $result | Should -Be @(
                'collect',
                '--package', 'nm',
                '--package', 'many_cpus',
                '--all-features',
                '--best-of', '3',
                '--verbose',
                '--skip-existing'
            )
        }

        It 'does not fall back to a whole-workspace scope when packages are given' {
            $result = Get-BenchHistoryCollectCommand -RecollectCommitId '' -Package @('nm')
            $result | Should -Not -Contain '--workspace'
            $result | Should -Not -Contain '--exclude'
        }

        It 'ignores blank entries in the package list' {
            $result = Get-BenchHistoryCollectCommand -RecollectCommitId '' -Package @('nm', '', '  ')
            $result | Should -Be @(
                'collect',
                '--package', 'nm',
                '--all-features',
                '--best-of', '3',
                '--verbose',
                '--skip-existing'
            )
        }

        It 'treats an all-blank package list as no scope (whole workspace)' {
            $result = Get-BenchHistoryCollectCommand -RecollectCommitId '' -Package @('', '  ')
            $result | Should -Be (@('collect') + $script:Scope + @('--skip-existing'))
        }

        It 'enables all features so Cargo runs required-feature benchmark targets' {
            $result = Get-BenchHistoryCollectCommand -Package 'nm_otel_impl'
            $result | Should -Contain '--all-features'
        }
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

        It 'measures with exactly the scope the push-to-main collect uses' {
            # A partially-scoped or lower-best-of commit would still count as recorded and never be
            # revisited, so the two builders must emit an identical scope slice. Both are compared
            # between their leading positionals and their distinct trailing flag.
            $collect = Get-BenchHistoryCollectCommand -RecollectCommitId ''
            $backfill = Get-BenchHistoryBackfillCommand
            $collectScope = $collect[1..($collect.Count - 2)]
            $backfillScope = $backfill[3..($backfill.Count - 2)]
            $backfillScope | Should -Be $collectScope
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

Describe 'Select-BenchmarkablePackage' {
    It 'drops configured packages wherever they occur' {
        Select-BenchmarkablePackage -Package @(
            'excluded-a', 'retained-z', 'excluded-m', 'retained-a', 'excluded-z') |
            Should -Be @('retained-z', 'retained-a')
    }

    It 'leaves no PR collection scope when only excluded packages changed' -ForEach @(
        @{ Packages = @('excluded-z') }
        @{ Packages = @('excluded-m') }
        @{ Packages = @('excluded-a', 'excluded-z', 'excluded-m') }
    ) {
        @(Select-BenchmarkablePackage -Package $Packages).Count | Should -Be 0
    }

    It 'returns an empty array for an empty input' {
        @(Select-BenchmarkablePackage -Package @()).Count | Should -Be 0
    }

    It 'preserves retained order and duplicates' {
        Select-BenchmarkablePackage -Package @(
            'retained-z', 'excluded-a', 'retained-a', 'retained-z') |
            Should -Be @('retained-z', 'retained-a', 'retained-z')
    }

    It 'matches excluded names exactly and case-sensitively' {
        Select-BenchmarkablePackage -Package @(
            'Excluded-z', 'excluded-z-extra', 'prefix-excluded-z', 'excluded-z', 'retained') |
            Should -BeExactly @('Excluded-z', 'excluded-z-extra', 'prefix-excluded-z', 'retained')
    }
}

Describe 'Exclusion configuration' {
    It 'honors <Name> exclusions in workspace and PR selection' -ForEach @(
        @{
            Name = 'empty'
            Exclusions = @()
            WorkspaceArguments = @('--workspace')
            Retained = @('alternate', 'excluded-z', 'retained')
        }
        @{
            Name = 'replacement'
            Exclusions = @('alternate')
            WorkspaceArguments = @('--workspace', '--exclude', 'alternate')
            Retained = @('excluded-z', 'retained')
        }
    ) {
        InModuleScope BenchHistoryCollect -Parameters @{
            Exclusions = $Exclusions
            WorkspaceArguments = $WorkspaceArguments
            Retained = $Retained
        } {
            param($Exclusions, $WorkspaceArguments, $Retained)
            $previous = $script:ExcludedPackages
            try {
                $script:ExcludedPackages = $Exclusions
                Get-BenchHistoryCollectCommand | Should -Be (
                    @('collect') + $WorkspaceArguments +
                    @('--all-features', '--best-of', '3', '--verbose', '--skip-existing'))
                Select-BenchmarkablePackage -Package @('alternate', 'excluded-z', 'retained') |
                    Should -Be $Retained
            } finally {
                $script:ExcludedPackages = $previous
            }
        }
    }
}

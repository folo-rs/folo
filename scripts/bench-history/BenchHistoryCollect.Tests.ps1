#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Pester suite for BenchHistoryCollect.psm1. Proves the mode selection the bench-history `collect`
# step depends on - append vs. overwrite, and the untrusted-input validation - without a workflow
# run: each case asserts the exact argument vector the step would hand the tool.

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'BenchHistoryCollect.psm1') -Force

    # Flags shared by both modes; asserted as a slice so a case only spells out what makes it
    # distinct (the subcommand, its positionals, and the append/overwrite tail).
    $script:Scope = @(
        '--workspace',
        '--exclude', 'benchmarks',
        '--best-of', '3',
        '--verbose'
    )
}

Describe 'Get-BenchHistoryCollectCommand' {
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
        It 'scopes to the given packages with repeated --package instead of --workspace' {
            $result = Get-BenchHistoryCollectCommand -RecollectCommitId '' -Package @('nm', 'many_cpus')
            $result | Should -Be @(
                'collect',
                '--package', 'nm',
                '--package', 'many_cpus',
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
                '--best-of', '3',
                '--verbose',
                '--skip-existing'
            )
        }

        It 'treats an all-blank package list as no scope (whole workspace)' {
            $result = Get-BenchHistoryCollectCommand -RecollectCommitId '' -Package @('', '  ')
            $result | Should -Be (@('collect') + $script:Scope + @('--skip-existing'))
        }
    }
}

Describe 'Select-BenchmarkablePackage' {
    It 'drops the excluded benchmarks package' {
        Select-BenchmarkablePackage -Package @('nm', 'benchmarks', 'many_cpus') |
            Should -Be @('nm', 'many_cpus')
    }

    It 'returns an empty array when only benchmarks changed' {
        @(Select-BenchmarkablePackage -Package @('benchmarks')).Count | Should -Be 0
    }

    It 'returns an empty array for an empty input' {
        @(Select-BenchmarkablePackage -Package @()).Count | Should -Be 0
    }

    It 'preserves order and leaves other packages untouched' {
        Select-BenchmarkablePackage -Package @('many_cpus', 'nm', 'events') |
            Should -Be @('many_cpus', 'nm', 'events')
    }

    It 'matches the excluded name case-sensitively' {
        Select-BenchmarkablePackage -Package @('Benchmarks', 'nm') |
            Should -Be @('Benchmarks', 'nm')
    }
}



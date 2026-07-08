#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Pester suite for Sharding.psm1. ConvertFrom-ShardSpec is pure, so every case is exercised
# directly: the happy path returns the 1-based index/count, and each malformed spec throws its
# specific, actionable message. These are the checks both `just mutants` and `just miri-harder`
# rely on, so a regression here would silently mis-shard a CI matrix.

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'Sharding.psm1') -Force
}

Describe 'ConvertFrom-ShardSpec' {
    Context 'valid specs' {
        It 'returns the 1-based index and count for a mid-range shard' {
            $result = ConvertFrom-ShardSpec -Spec '3/8'
            $result.Index | Should -Be 3
            $result.Count | Should -Be 8
        }

        It 'accepts the first shard' {
            $result = ConvertFrom-ShardSpec -Spec '1/8'
            $result.Index | Should -Be 1
            $result.Count | Should -Be 8
        }

        It 'accepts the last shard (index equal to count)' {
            $result = ConvertFrom-ShardSpec -Spec '8/8'
            $result.Index | Should -Be 8
            $result.Count | Should -Be 8
        }

        It 'accepts a single-shard spec' {
            $result = ConvertFrom-ShardSpec -Spec '1/1'
            $result.Index | Should -Be 1
            $result.Count | Should -Be 1
        }
    }

    Context 'malformed specs' {
        It 'rejects a spec with the wrong number of parts' {
            { ConvertFrom-ShardSpec -Spec '3' } | Should -Throw "*Expected format 'N/M'*"
            { ConvertFrom-ShardSpec -Spec '1/2/3' } | Should -Throw "*Expected format 'N/M'*"
        }

        It 'rejects an empty spec (the caller must handle "no sharding" first)' {
            # An empty string is not a valid shard spec; the mandatory parameter rejects it.
            { ConvertFrom-ShardSpec -Spec '' } | Should -Throw
        }

        It 'rejects non-integer components' {
            { ConvertFrom-ShardSpec -Spec 'a/8' } | Should -Throw '*must be integers*'
            { ConvertFrom-ShardSpec -Spec '3/x' } | Should -Throw '*must be integers*'
        }

        It 'rejects a non-positive shard count' {
            { ConvertFrom-ShardSpec -Spec '1/0' } | Should -Throw '*M must be a positive integer*'
            { ConvertFrom-ShardSpec -Spec '1/-2' } | Should -Throw '*M must be a positive integer*'
        }

        It 'rejects an index below 1' {
            { ConvertFrom-ShardSpec -Spec '0/8' } | Should -Throw '*1 <= N <= M*'
        }

        It 'rejects an index above the count' {
            { ConvertFrom-ShardSpec -Spec '9/8' } | Should -Throw '*1 <= N <= M*'
        }
    }
}

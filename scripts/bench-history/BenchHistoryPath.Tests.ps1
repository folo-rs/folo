#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Pester suite for BenchHistoryPath.psm1. Proves the directory preparation the benchmark-history
# automation recipes depend on - creating a scratch/report directory (and the machine-key file's
# parent) before writing into it - actually runs, without a workflow run. Because a justfile [script]
# block is invisible to both PSScriptAnalyzer and Pester, an invalid cmdlet invocation in this glue
# (New-Item -LiteralPath, which New-Item does not support) once reached CI unvalidated; exercising the
# extracted seam here executes the exact line that broke, so the same class of mistake fails loudly in
# `just test-scripts` instead of in a live run. Directories are real temp paths so creation, the
# idempotent already-exists case, nested-parent creation and the empty-path no-op are asserted, not
# faked.

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'BenchHistoryPath.psm1') -Force

    function Get-ScratchPath {
        Join-Path ([System.IO.Path]::GetTempPath()) ("bh-path-$([guid]::NewGuid().ToString('n'))")
    }
}

Describe 'New-BenchHistoryDirectory' {
    Context 'creating a directory' {
        It 'creates a missing directory' {
            $dir = Get-ScratchPath
            try {
                New-BenchHistoryDirectory -Path $dir
                Test-Path -LiteralPath $dir -PathType Container | Should -BeTrue
            } finally {
                Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
            }
        }

        It 'creates any missing intermediate parents' {
            $root = Get-ScratchPath
            $nested = Join-Path $root (Join-Path 'cache' 'objects')
            try {
                New-BenchHistoryDirectory -Path $nested
                Test-Path -LiteralPath $nested -PathType Container | Should -BeTrue
            } finally {
                Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
            }
        }

        It 'is a no-op when the directory already exists' {
            $dir = Get-ScratchPath
            try {
                New-BenchHistoryDirectory -Path $dir
                # A pre-existing file inside proves the second call did not recreate/clear the directory.
                $marker = Join-Path $dir 'marker.txt'
                Set-Content -LiteralPath $marker -Value 'kept' -Encoding utf8
                { New-BenchHistoryDirectory -Path $dir } | Should -Not -Throw
                Test-Path -LiteralPath $marker -PathType Leaf | Should -BeTrue
            } finally {
                Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
            }
        }
    }

    Context 'no directory component' {
        It 'does nothing for an empty path (a bare filename has no parent)' {
            { New-BenchHistoryDirectory -Path '' } | Should -Not -Throw
        }

        It 'does nothing for a null path' {
            { New-BenchHistoryDirectory -Path $null } | Should -Not -Throw
        }
    }
}

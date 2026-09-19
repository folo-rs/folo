#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Pester suite for BenchHistoryMachineKey.psm1. Proves the machine-key threading the bench-history
# `analyze` step depends on - reading reconciled collection fingerprints into the
# repeated `--machine-key` argument vector, with dedupe, ordering, validation and the empty-directory
# edge case a total collect failure produces - without a workflow run. Key files are real temp files
# so the on-disk read and the missing/empty guards are asserted, not faked.

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'BenchHistoryMachineKey.psm1') -Force

    # A fresh key directory per test, matching the companion's per-platform reconciliation output.
    function Get-KeyDirectory {
        $dir = Join-Path ([System.IO.Path]::GetTempPath()) ("bh-mk-$([guid]::NewGuid().ToString('n'))")
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
        return $dir
    }

    function Write-KeyFile {
        param(
            [Parameter(Mandatory)] [string] $Directory,
            [Parameter(Mandatory)] [string] $Name,
            [Parameter(Mandatory)] [AllowEmptyString()] [string] $Content
        )
        # Reconciliation keeps platform subdirectories; the argument builder scans them recursively.
        $sub = Join-Path $Directory $Name
        New-Item -ItemType Directory -Path $sub -Force | Out-Null
        Set-Content -LiteralPath (Join-Path $sub 'machine-key.txt') -Value $Content -Encoding utf8
    }
}

Describe 'Get-BenchHistoryAnalysisCommand' {
    It 'returns a clean-only argument vector with frozen topology, measured repository and cache' {
        $keys = Join-Path $TestDrive 'keys'
        Write-KeyFile -Directory $keys -Name 'first' -Content 'abcdef0123456789'
        Write-KeyFile -Directory $keys -Name 'second' -Content '0123456789abcdef'
        $report = Join-Path $TestDrive 'report output'
        $result = Get-BenchHistoryAnalysisCommand -KeyDirectory $keys -ReportDirectory $report `
            -Context ('a' * 40) -Base ('b' * 40) -Repository 'measured repo'
        , $result | Should -BeOfType [string[]]
        $result | Should -Be @(
            'analyze', '--engine', 'all', '--target-triple', 'all'
            '--machine-key', '0123456789abcdef', '--machine-key', 'abcdef0123456789'
            '--context', ('a' * 40), '--base', ('b' * 40), '--no-dirty', '--verbose'
            "--cache=$(Join-Path $report 'cache')"
            '--no-text', '--markdown', (Join-Path $report 'report.md')
            '--json', (Join-Path $report 'report.json')
            '--markdown-summary', (Join-Path $report 'summary.md')
            '--repo', 'measured repo'
        )
    }

    It 'uses the current checkout for ordinary history analysis' {
        $keys = Join-Path $TestDrive 'history keys'
        Write-KeyFile -Directory $keys -Name 'first' -Content 'abcdef0123456789'
        $result = Get-BenchHistoryAnalysisCommand -KeyDirectory $keys -ReportDirectory $TestDrive `
            -Context ('a' * 40) -Base ('a' * 40)
        $result | Should -Not -Contain '--repo'
        $result | Should -Contain '--no-dirty'
    }

    It 'fails instead of fabricating a report when collection supplied no keys' {
        { Get-BenchHistoryAnalysisCommand -KeyDirectory (Join-Path $TestDrive 'absent') `
            -ReportDirectory $TestDrive -Context ('a' * 40) -Base ('a' * 40) } | Should -Throw
        Test-Path -LiteralPath (Join-Path $TestDrive 'report.json') | Should -BeFalse
    }
}

Describe 'Get-MachineKeyArgument' {
    Context 'a normal multi-runner collection' {
        It 'builds one --machine-key per distinct fingerprint, sorted' {
            $dir = Get-KeyDirectory
            try {
                Write-KeyFile -Directory $dir -Name 'ubuntu-latest' -Content 'ffff0000ffff0000'
                Write-KeyFile -Directory $dir -Name 'windows-latest' -Content '00001111aaaa2222'
                $result = Get-MachineKeyArgument -KeyDirectory $dir
                $result | Should -Be @(
                    '--machine-key', '00001111aaaa2222',
                    '--machine-key', 'ffff0000ffff0000'
                )
            } finally {
                Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
            }

        }

        It 'collapses duplicate fingerprints from identically-specced runners' {
            $dir = Get-KeyDirectory
            try {
                Write-KeyFile -Directory $dir -Name 'ubuntu-latest' -Content 'abcdef0123456789'
                Write-KeyFile -Directory $dir -Name 'ubuntu-24.04-arm' -Content 'abcdef0123456789'
                $result = Get-MachineKeyArgument -KeyDirectory $dir
                $result | Should -Be @('--machine-key', 'abcdef0123456789')
            } finally {
                Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
            }
        }

        It 'trims surrounding whitespace and lowercases before use' {
            $dir = Get-KeyDirectory
            try {
                Write-KeyFile -Directory $dir -Name 'windows-11-arm' -Content "  ABCDEF0123456789`n"
                $result = Get-MachineKeyArgument -KeyDirectory $dir
                $result | Should -Be @('--machine-key', 'abcdef0123456789')
            } finally {
                Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
            }
        }

        It 'returns a string array even for a single key' {
            $dir = Get-KeyDirectory
            try {
                Write-KeyFile -Directory $dir -Name 'ubuntu-latest' -Content 'abcdef0123456789'
                $result = Get-MachineKeyArgument -KeyDirectory $dir
                , $result | Should -BeOfType [string[]]
                $result.Count | Should -Be 2
            } finally {
                Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
            }
        }

        It 'ignores stray non-key files the artifact download may leave alongside the keys' {
            $dir = Get-KeyDirectory
            try {
                Write-KeyFile -Directory $dir -Name 'ubuntu-latest' -Content 'abcdef0123456789'
                # Files the download can drop next to the real keys - metadata, a macOS `.DS_Store`,
                # an accidental readme. None is a machine-key.txt, so the scan must skip them all
                # rather than fail fingerprint validation and abort the whole analysis.
                Set-Content -LiteralPath (Join-Path $dir 'metadata.json') -Value '{ "not": "a key" }' -Encoding utf8
                Set-Content -LiteralPath (Join-Path $dir 'README.md') -Value 'not a fingerprint' -Encoding utf8
                Set-Content -LiteralPath (Join-Path $dir 'ubuntu-latest/.DS_Store') -Value 'junk' -Encoding utf8
                $result = Get-MachineKeyArgument -KeyDirectory $dir
                $result | Should -Be @('--machine-key', 'abcdef0123456789')
            } finally {
                Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
            }
        }
    }

    Context 'verbose diagnostics' {
        It 'uses the singular noun for exactly one key' {
            $dir = Get-KeyDirectory
            try {
                Write-KeyFile -Directory $dir -Name 'ubuntu-latest' -Content 'abcdef0123456789'
                $verbose = Get-MachineKeyArgument -KeyDirectory $dir -Verbose 4>&1 |
                    Where-Object { $_ -is [System.Management.Automation.VerboseRecord] }
                ($verbose -join "`n") | Should -Match 'Threading 1 machine key into analysis'
            } finally {
                Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
            }
        }

        It 'uses the plural noun for more than one key' {
            $dir = Get-KeyDirectory
            try {
                Write-KeyFile -Directory $dir -Name 'ubuntu-latest' -Content 'ffff0000ffff0000'
                Write-KeyFile -Directory $dir -Name 'windows-latest' -Content '00001111aaaa2222'
                $verbose = Get-MachineKeyArgument -KeyDirectory $dir -Verbose 4>&1 |
                    Where-Object { $_ -is [System.Management.Automation.VerboseRecord] }
                ($verbose -join "`n") | Should -Match 'Threading 2 machine keys into analysis'
            } finally {
                Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
            }
        }
    }

    Context 'zero collected keys (total collect failure)' {
        It 'returns an empty vector for a non-existent directory' {
            $missing = Join-Path ([System.IO.Path]::GetTempPath()) ([guid]::NewGuid().ToString('n'))
            @(Get-MachineKeyArgument -KeyDirectory $missing).Count | Should -Be 0
        }

        It 'returns an empty vector for an empty directory' {
            $dir = Get-KeyDirectory
            try {
                @(Get-MachineKeyArgument -KeyDirectory $dir).Count | Should -Be 0
            } finally {
                Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
            }
        }

        It 'returns an empty vector for a directory holding only stray non-key files' {
            $dir = Get-KeyDirectory
            try {
                # A total collect failure uploads no machine-key.txt; anything else in the tree is not
                # a key, so the recipe treats it as zero collected keys and the caller skips analysis.
                Set-Content -LiteralPath (Join-Path $dir 'metadata.json') -Value '{}' -Encoding utf8
                @(Get-MachineKeyArgument -KeyDirectory $dir).Count | Should -Be 0
            } finally {
                Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
            }
        }

        It 'returns an empty vector for a null or blank directory argument' {
            @(Get-MachineKeyArgument -KeyDirectory $null).Count | Should -Be 0
            @(Get-MachineKeyArgument -KeyDirectory '   ').Count | Should -Be 0
        }
    }

    Context 'corrupt uploads' {
        It 'throws on an empty key file' {
            $dir = Get-KeyDirectory
            try {
                Write-KeyFile -Directory $dir -Name 'ubuntu-latest' -Content ''
                { Get-MachineKeyArgument -KeyDirectory $dir } | Should -Throw '*is empty*'
            } finally {
                Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
            }
        }

        It 'throws on a non-hex fingerprint' {
            $dir = Get-KeyDirectory
            try {
                Write-KeyFile -Directory $dir -Name 'ubuntu-latest' -Content 'not-a-fingerprint'
                { Get-MachineKeyArgument -KeyDirectory $dir } | Should -Throw '*16-hex-character*'
            } finally {
                Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
            }
        }

        It 'throws on a fingerprint of the wrong length' {
            $dir = Get-KeyDirectory
            try {
                Write-KeyFile -Directory $dir -Name 'ubuntu-latest' -Content 'abcdef01'
                { Get-MachineKeyArgument -KeyDirectory $dir } | Should -Throw '*16-hex-character*'
            } finally {
                Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
            }
        }

        It 'throws on a file carrying shell metacharacters' {
            $dir = Get-KeyDirectory
            try {
                Write-KeyFile -Directory $dir -Name 'ubuntu-latest' -Content 'abc; rm -rf /'
                { Get-MachineKeyArgument -KeyDirectory $dir } | Should -Throw '*16-hex-character*'
            } finally {
                Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
            }
        }
    }
}

#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Pester suite for BenchHistoryMachineKey.psm1. Proves the machine-key threading the bench-history
# `analyze` step depends on - reading the collect matrix's uploaded fingerprint files into the
# repeated `--machine-key` argument vector, with dedupe, ordering, validation and the empty-directory
# edge case a total collect failure produces - without a workflow run. Key files are real temp files
# so the on-disk read and the missing/empty guards are asserted, not faked.

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'BenchHistoryMachineKey.psm1') -Force

    # A fresh, isolated key directory per test; helpers create the per-artifact files inside it the
    # same way `actions/download-artifact` would (one file per collect leg).
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
        # Each collect leg's artifact typically arrives in its own subdirectory (download without
        # merge), so nest the file to prove the recursive scan finds it.
        $sub = Join-Path $Directory $Name
        New-Item -ItemType Directory -Path $sub -Force | Out-Null
        Set-Content -LiteralPath (Join-Path $sub 'machine-key.txt') -Value $Content -Encoding utf8
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

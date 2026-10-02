#requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Exercises the real `just careful` Cargo runner with an owned native child. The runner must
# restore original compiler settings without changing argv, unrelated environment or failures.
# Real merged rustdoc examples also require separate processes, including with serial execution.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll {
    $script:runner = Join-Path $PSScriptRoot 'Invoke-CarefulTest.ps1'
    $script:probe = Join-Path $TestDrive $(if ($IsWindows) { 'probe.exe' } else { 'probe' })
    & rustc --edition=2024 -D warnings --crate-name careful_probe `
        (Join-Path $PSScriptRoot 'fixtures\careful\probe.rs') -o $probe

    $script:doctestSource = Join-Path $PSScriptRoot 'fixtures\careful\doctests.rs'
    $script:doctestLibrary = Join-Path $TestDrive 'libcareful_doctests.rlib'
    & rustc "+$env:RUST_NIGHTLY" --edition=2024 -D warnings --cfg careful `
        --crate-name careful_doctests --crate-type lib $doctestSource -o $doctestLibrary

    function Invoke-CarefulRunnerProbe {
        param(
            [hashtable] $Flags = @{
                RUSTFLAGS = $null
                CARGO_ENCODED_RUSTFLAGS = $null
                RUSTDOCFLAGS = $null
                CARGO_ENCODED_RUSTDOCFLAGS = $null
            },
            [string[]] $Arguments = @(),
            [int] $ExitCode = 0
        )
        $start = [Diagnostics.ProcessStartInfo]::new()
        $start.FileName = (Get-Command pwsh -CommandType Application | Select-Object -First 1).Source
        $start.UseShellExecute = $false
        $start.RedirectStandardOutput = $true
        $start.RedirectStandardError = $true
        foreach ($argument in @('-NoLogo', '-NoProfile', '-NonInteractive', '-File', $runner, $probe) + $Arguments) {
            $start.ArgumentList.Add($argument)
        }
        $start.Environment['FOLO_CAREFUL_BUILD_FLAGS'] = $Flags | ConvertTo-Json -Compress
        $start.Environment['RUSTFLAGS'] = '--cfg careful -Zextra-const-ub-checks --sysroot careful'
        $start.Environment['CARGO_ENCODED_RUSTFLAGS'] = '--cfg' + [char]0x1f + 'careful'
        $start.Environment['RUSTDOCFLAGS'] = '--sysroot careful'
        $start.Environment['CARGO_ENCODED_RUSTDOCFLAGS'] = '--sysroot' + [char]0x1f + 'careful'
        $start.Environment['CARGO_TARGET_DIR'] = 'caller-target'
        $start.Environment['CAREFUL_PROBE_EXIT'] = [string]$ExitCode
        $process = [Diagnostics.Process]::new()
        $process.StartInfo = $start
        try {
            $null = $process.Start()
            $stdout = $process.StandardOutput.ReadToEndAsync()
            $stderr = $process.StandardError.ReadToEndAsync()
            $process.WaitForExit()
            return @{
                Code = $process.ExitCode
                Output = $stdout.GetAwaiter().GetResult()
                Error = $stderr.GetAwaiter().GetResult()
            }
        } finally { $process.Dispose() }
    }
}

Describe 'Careful test runner' {
    It 'removes only instrumentation absent from the caller and preserves literal arguments' {
        $result = Invoke-CarefulRunnerProbe -Arguments @('test::name', '', 'a path', '*.rs', '--exact')
        $result.Code | Should -Be 0 -Because $result.Error
        $lines = $result.Output -split '\r?\n'
        $lines[0..4] | Should -Be @('argument=test::name', 'argument=', 'argument=a path', 'argument=*.rs', 'argument=--exact')
        $lines | Should -Contain 'RUSTFLAGS=None'
        $lines | Should -Contain 'CARGO_ENCODED_RUSTFLAGS=None'
        $lines | Should -Contain 'RUSTDOCFLAGS=None'
        $lines | Should -Contain 'CARGO_ENCODED_RUSTDOCFLAGS=None'
        $lines | Should -Contain 'CARGO_TARGET_DIR=Some("caller-target")'
        $lines | Should -Contain 'FOLO_CAREFUL_BUILD_FLAGS=None'
    }

    It 'restores original flags including encoded separators and returns the test failure status' {
        $result = Invoke-CarefulRunnerProbe -Flags @{
            RUSTFLAGS = '--cfg caller'
            CARGO_ENCODED_RUSTFLAGS = '--cfg' + [char]0x1f + 'caller="a b"'
            RUSTDOCFLAGS = '--cfg docs'
            CARGO_ENCODED_RUSTDOCFLAGS = '--cfg' + [char]0x1f + 'docs'
        } -ExitCode 17
        $result.Code | Should -Be 17
        $lines = $result.Output -split '\r?\n'
        $lines | Should -Contain 'RUSTFLAGS=Some("--cfg caller")'
        $lines | Should -Contain 'CARGO_ENCODED_RUSTFLAGS=Some("--cfg\u{1f}caller=\"a b\"")'
        $lines | Should -Contain 'RUSTDOCFLAGS=Some("--cfg docs")'
        $lines | Should -Contain 'CARGO_ENCODED_RUSTDOCFLAGS=Some("--cfg\u{1f}docs")'
    }

    It 'rejects incomplete captured state rather than silently clearing caller configuration' {
        $result = Invoke-CarefulRunnerProbe -Flags @{ RUSTFLAGS = $null }
        $result.Code | Should -Not -Be 0
        $result.Output | Should -BeNullOrEmpty
    }

    It 'isolates merged doctests with <Threads> test threads and restores nested-build flags' -ForEach @(
        @{ Threads = 1 }
        @{ Threads = 2 }
    ) {
        $start = [Diagnostics.ProcessStartInfo]::new()
        $start.FileName = (Get-Command rustdoc -CommandType Application | Select-Object -First 1).Source
        $start.UseShellExecute = $false
        $start.RedirectStandardOutput = $true
        $start.RedirectStandardError = $true
        foreach ($argument in @("+$env:RUST_NIGHTLY", '--test', '--edition=2024', '-Dwarnings',
                '--cfg=careful', '-Zunstable-options', '--merge-doctests=yes',
                "--extern=careful_doctests=$doctestLibrary", "--test-args=--test-threads=$Threads",
                '--test-runtool=pwsh', '--test-runtool-arg=-NoLogo', '--test-runtool-arg=-NoProfile',
                '--test-runtool-arg=-NonInteractive', '--test-runtool-arg=-File',
                "--test-runtool-arg=$runner", $doctestSource)) {
            $start.ArgumentList.Add($argument)
        }
        $flags = @{}
        foreach ($name in @('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTDOCFLAGS', 'CARGO_ENCODED_RUSTDOCFLAGS')) {
            $flags[$name] = $null
            $start.Environment[$name] = '--cfg=careful'
        }
        $start.Environment['FOLO_CAREFUL_BUILD_FLAGS'] = $flags | ConvertTo-Json -Compress
        $null = $start.Environment.Remove('RUSTDOC_DOCTEST_BIN_PATH')
        $null = $start.Environment.Remove('RUSTDOC_DOCTEST_RUN_NB_TEST')
        $process = [Diagnostics.Process]::new()
        $process.StartInfo = $start
        try {
            $null = $process.Start()
            $stdout = $process.StandardOutput.ReadToEndAsync()
            $stderr = $process.StandardError.ReadToEndAsync()
            $process.WaitForExit()
            $output = $stdout.GetAwaiter().GetResult() + $stderr.GetAwaiter().GetResult()
            $process.ExitCode | Should -Be 0 -Because $output
            $output | Should -Match '2 passed; 0 failed'
            $output | Should -Not -Match 'WARNING:'
        } finally { $process.Dispose() }
    }
}

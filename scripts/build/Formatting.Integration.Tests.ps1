#requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Exercises the real format/check recipes and installed tools in a dependency-free workspace.
# The target inventory exceeds Windows' aggregate argument limit while each package fits.
# This suite runs through just test-scripts on Windows and Linux without changing product files.
# Ref: docs/build-and-tooling.md#workspace-formatting.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'

BeforeAll {
    $repository = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
    $script:workspace = Join-Path $TestDrive 'copilot-worktrees\formatting workspace with spaces'
    $recipes = Join-Path $workspace 'justfiles'
    $null = New-Item -ItemType Directory -Path $recipes -Force
    foreach ($file in @('justfile', 'constants.env', 'rust-toolchain.toml', 'unstable-rustfmt.toml')) {
        Copy-Item -LiteralPath (Join-Path $repository $file) -Destination $workspace
    }
    Get-ChildItem -LiteralPath (Join-Path $repository 'justfiles') -Filter '*.just' |
        Copy-Item -Destination $recipes
    @'
[workspace]
members = ["first", "last"]
default-members = ["first"]
resolver = "3"
'@ | Set-Content -LiteralPath (Join-Path $workspace 'Cargo.toml')

    # CreateProcessW limits the command line including its terminator. Derive the fixture size
    # from its shortest target path rather than relying on a particular temporary-directory length.
    # https://learn.microsoft.com/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessw
    $script:windowsArgumentLimit = 32767
    $names = @('first', 'last')
    $shortestTarget = Join-Path $workspace 'last\examples\e0.rs'
    $exampleCount = [int][Math]::Ceiling($windowsArgumentLimit / ($names.Count * $shortestTarget.Length))
    $script:sources = @{}
    $script:unformatted = "use std::fmt::Debug;`nuse std::fmt::Display;`nfn main( ){ }`n"
    foreach ($name in $names) {
        $directory = Join-Path $workspace $name
        foreach ($folder in @('src', 'tests', 'examples', 'benches')) {
            $null = New-Item -ItemType Directory -Path (Join-Path $directory $folder) -Force
        }
        @"
[package]
name = "$name"
version = "0.0.0"
edition = "2024"
publish = false

[features]
extra = []

[[example]]
name = "example"
required-features = ["extra"]
"@ | Set-Content -LiteralPath (Join-Path $directory 'Cargo.toml')
        $paths = @('src\lib.rs', 'src\main.rs', 'build.rs', 'tests\integration.rs',
            'examples\example.rs', 'benches\benchmark.rs', 'src\unix.rs', 'src\windows.rs')
        $paths += @(0..($exampleCount - 1) | ForEach-Object { "examples\e$_.rs" })
        foreach ($path in $paths) {
            $source = Join-Path $directory $path
            $content = $unformatted
            if ($path -eq 'src\lib.rs') {
                $content += "#[cfg(unix)] mod unix;`n#[cfg(windows)] mod windows;`n"
            }
            $sources[$source] = $content
        }
    }

    function Invoke-FormattingFixture {
        # Capture real process outcomes, including expected failures, without installing tools
        # or resolving network dependencies. Running inside a member exercises Just's root lookup.
        [CmdletBinding()]
        [OutputType([hashtable])]
        param(
            [Parameter(Mandatory)][string[]] $Arguments,
            [string] $Executable = 'just'
        )

        $start = [Diagnostics.ProcessStartInfo]::new()
        $start.FileName = (Get-Command $Executable -CommandType Application | Select-Object -First 1).Source
        $start.WorkingDirectory = Join-Path $workspace 'first\src'
        $start.UseShellExecute = $false
        $start.RedirectStandardOutput = $true
        $start.RedirectStandardError = $true
        $start.Environment['RUSTUP_AUTO_INSTALL'] = '0'
        $start.Environment['CARGO_NET_OFFLINE'] = 'true'
        foreach ($argument in $Arguments) { $start.ArgumentList.Add($argument) }
        $process = [Diagnostics.Process]::new()
        $process.StartInfo = $start
        try {
            $null = $process.Start()
            $stdout = $process.StandardOutput.ReadToEndAsync()
            $stderr = $process.StandardError.ReadToEndAsync()
            $process.WaitForExit()
            return @{
                ExitCode = $process.ExitCode
                Output = $stdout.GetAwaiter().GetResult() + $stderr.GetAwaiter().GetResult()
            }
        } finally {
            $process.Dispose()
        }
    }
}

Describe 'Workspace formatting recipes' {
    BeforeEach {
        foreach ($source in $sources.Keys) {
            Set-Content -LiteralPath $source -Value $sources[$source] -NoNewline
        }
    }

    It 'checks without writing and formats every target in package-sized batches from a subdirectory' {
        $check = Invoke-FormattingFixture -Arguments @('package=first', 'format-check')
        $check.ExitCode | Should -Not -Be 0 -Because $check.Output
        foreach ($source in $sources.Keys) {
            Get-Content -LiteralPath $source -Raw | Should -BeExactly $sources[$source]
        }

        $format = Invoke-FormattingFixture -Arguments @('package=first', 'format')
        $format.ExitCode | Should -Be 0 -Because $format.Output
        foreach ($source in $sources.Keys) {
            $content = Get-Content -LiteralPath $source -Raw
            # Import merging also proves that the shared nightly-only configuration took effect.
            $content | Should -Match 'use std::fmt::\{Debug, Display\};'
            $content | Should -Match 'fn main\(\) \{\}'
        }
        $check = Invoke-FormattingFixture -Arguments @('package=first', 'format-check')
        $check.ExitCode | Should -Be 0 -Because $check.Output

        $metadata = Invoke-FormattingFixture -Executable 'cargo' `
            -Arguments @('metadata', '--no-deps', '--offline', '--format-version', '1')
        $metadata.ExitCode | Should -Be 0 -Because $metadata.Output
        $inventory = $metadata.Output | ConvertFrom-Json
        $pathCharacters = ($inventory.packages.targets.src_path |
            ForEach-Object { $_.Length } | Measure-Object -Sum).Sum
        $pathCharacters | Should -BeGreaterThan $windowsArgumentLimit
        foreach ($member in $inventory.packages) {
            ($member.targets.src_path | ForEach-Object { $_.Length } | Measure-Object -Sum).Sum |
                Should -BeLessThan $windowsArgumentLimit
        }
        if ($IsWindows) {
            # With sources already formatted, the oversized control must fail at process launch,
            # not because of a formatting difference. The OS error number identifies that failure.
            $control = Invoke-FormattingFixture -Executable 'cargo' -Arguments @(
                "+$env:RUST_NIGHTLY", 'fmt', '--all', '--check', '--', '--config-path',
                (Join-Path $workspace 'unstable-rustfmt.toml')
            )
            $control.ExitCode | Should -Not -Be 0
            $control.Output | Should -Match 'os error 206'
        }
    }

    It 'returns a formatting error while still formatting later workspace members' {
        $invalidSource = Join-Path $workspace 'first\src\lib.rs'
        Set-Content -LiteralPath $invalidSource -Value 'fn main('
        $format = Invoke-FormattingFixture -Arguments @('format')
        $format.ExitCode | Should -Not -Be 0 -Because $format.Output
        Get-Content -LiteralPath (Join-Path $workspace 'last\src\lib.rs') -Raw |
            Should -Match 'use std::fmt::\{Debug, Display\};'
    }
}

#Requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Exercises the workflow companion builder against synthetic Cargo artifacts and archive calls.
# Temporary output parents use the real directory helper; no compiler or archiver is executed.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'

BeforeAll {
    $script:Builder = Join-Path $PSScriptRoot 'build-companion.ps1'
}

Describe 'Workflow companion archive' {
    BeforeEach {
        $script:Binary = Join-Path $TestDrive 'custom target' 'cargo-bench-history-github'
        $script:Archive = Join-Path $TestDrive 'reports' 'companion.tar.gz'
        $script:State = @{ Binary = $script:Binary; CargoArgs = @(); TarArgs = @() }
        $state = $script:State
        Mock cargo ({
            $state.CargoArgs = @($args)
            @{
                reason = 'compiler-artifact'
                target = @{ name = 'cargo-bench-history-github' }
                executable = $state.Binary
            } | ConvertTo-Json -Compress
        }.GetNewClosure())
        Mock tar ({ $state.TarArgs = @($args) }.GetNewClosure())
    }

    It 'archives the executable Cargo reports using the ordinary target-directory policy' {
        $binary = & $script:Builder -ArchivePath $script:Archive

        $binary | Should -Be $script:Binary
        $script:State.CargoArgs | Should -Contain '--locked'
        $script:State.CargoArgs | Should -Not -Contain '--target-dir'
        $script:State.CargoArgs[[array]::IndexOf($script:State.CargoArgs, '--target') + 1] |
            Should -Be 'x86_64-unknown-linux-gnu'
        $script:State.TarArgs | Should -Be @(
            '-czf', $script:Archive, '-C', (Split-Path -Parent $script:Binary),
            'cargo-bench-history-github'
        )
        Test-Path -LiteralPath (Split-Path -Parent $script:Archive) -PathType Container |
            Should -BeTrue
    }

    It 'does not archive when Cargo emits no matching executable' {
        Mock cargo { '{"reason":"build-finished","success":true}' }

        { & $script:Builder -ArchivePath $script:Archive } | Should -Throw
        Should -Invoke tar -Times 0 -Exactly
    }

    It 'propagates a build failure without archiving' {
        Mock cargo { throw [InvalidOperationException]::new('build-canary') }

        { & $script:Builder -ArchivePath $script:Archive } |
            Should -Throw -ExceptionType ([InvalidOperationException])
        Should -Invoke tar -Times 0 -Exactly
    }

    It 'propagates an archive failure without returning an executable' {
        Mock tar { throw [IO.IOException]::new('archive-canary') }

        { & $script:Builder -ArchivePath $script:Archive } |
            Should -Throw -ExceptionType ([IO.IOException])
    }
}

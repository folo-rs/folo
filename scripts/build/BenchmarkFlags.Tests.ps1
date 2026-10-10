#Requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Protects local benchmark-profile selection without invoking Cargo or changing the environment.
# The fixtures vary Rust's option spellings and encoded boundaries, not checked-in policy text.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'BenchmarkFlags.psm1') -Force
    $script:Separator = [string][char]0x1f
}

Describe 'Get-BenchmarkEncodedRustFlagSet' {
    It 'applies a profile with no ambient arguments' {
        Get-BenchmarkEncodedRustFlagSet -StabilityFlags '--cfg=profile' |
            Should -BeExactly '--cfg=profile'
    }

    It 'replaces competing alignment options in <Form> form for <Option>' -ForEach @(
        foreach ($option in @('functions', 'nofallthru-blocks', 'blocks')) {
            foreach ($form in @('-C', '-C ', '--codegen=', '--codegen ')) {
                @{ Option = $option; Form = $form }
            }
        }
    ) {
        # The values are representative: profile selection must not depend on their magnitude.
        $result = Get-BenchmarkEncodedRustFlagSet `
            -RustFlags "-Copt-level=2 ${Form}llvm-args=-align-all-${Option}=3 --cfg=caller" `
            -StabilityFlags '-Cllvm-args=-align-all-functions=6 -Cllvm-args=-align-all-nofallthru-blocks=6'
        $result | Should -BeExactly (@(
            '-Copt-level=2', '--cfg=caller',
            '-Cllvm-args=-align-all-functions=6', '-Cllvm-args=-align-all-nofallthru-blocks=6'
        ) -join $script:Separator)
    }

    It 'removes every inherited alignment policy for the best profile' {
        $result = Get-BenchmarkEncodedRustFlagSet -RustFlags (
            '-Cllvm-args=-align-all-functions=3 --codegen llvm-args=-align-all-nofallthru-blocks=4 ' +
            '-C llvm-args=-align-all-blocks=5 -Ctarget-cpu=native'
        )
        $result | Should -BeExactly '-Ctarget-cpu=native'
    }

    It 'honors encoded precedence and preserves a whitespace-containing argument' {
        $encoded = @('--cfg', 'caller="a b"', '-Cllvm-args=-align-all-functions=3') -join $script:Separator
        $result = Get-BenchmarkEncodedRustFlagSet -RustFlags '--cfg=ignored' -EncodedRustFlags $encoded `
            -StabilityFlags '-Cllvm-args=-align-all-functions=6'
        $result | Should -BeExactly (
            @('--cfg', 'caller="a b"', '-Cllvm-args=-align-all-functions=6') -join $script:Separator
        )
    }

    It 'keeps an explicitly empty encoded variable authoritative' {
        Get-BenchmarkEncodedRustFlagSet -RustFlags '--cfg=ignored' -EncodedRustFlags '' |
            Should -BeExactly ''
    }

    It 'preserves unrelated options inside a combined LLVM argument' {
        $encoded = @('-C', 'llvm-args=-align-all-functions=3 -inline-threshold=42 -align-all-nofallthru-blocks=4') -join $script:Separator
        Get-BenchmarkEncodedRustFlagSet -EncodedRustFlags $encoded |
            Should -BeExactly (@('-C', 'llvm-args=-inline-threshold=42') -join $script:Separator)
    }

    It 'keeps unrelated encoded empty arguments for the compiler to interpret' {
        $encoded = @('--cfg', '', '--cfg=caller') -join $script:Separator
        Get-BenchmarkEncodedRustFlagSet -EncodedRustFlags $encoded | Should -BeExactly $encoded
    }

    It 'leaves malformed and unknown alignment options visible' {
        $flags = '-Cllvm-args=-align-all-functions=bad -Cllvm-args=-align-all-blocks=6junk -Cllvm-args=-align-loops=64'
        Get-BenchmarkEncodedRustFlagSet -RustFlags $flags |
            Should -BeExactly (($flags -split ' ') -join $script:Separator)
    }

    It 'does not consume a dangling codegen option' {
        Get-BenchmarkEncodedRustFlagSet -RustFlags '--cfg=caller -C' |
            Should -BeExactly (@('--cfg=caller', '-C') -join $script:Separator)
    }

    It 'rejects an unrepresentable plain-flag separator' {
        { Get-BenchmarkEncodedRustFlagSet -RustFlags "--cfg=one${script:Separator}--cfg=two" } |
            Should -Throw '*encoded argument separator*'
        { Get-BenchmarkEncodedRustFlagSet -StabilityFlags "--cfg=one${script:Separator}--cfg=two" } |
            Should -Throw '*encoded argument separator*'
    }
}

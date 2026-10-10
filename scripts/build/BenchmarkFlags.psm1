#Requires -Version 7.6

# Selects compiler flags for the local Just benchmark profiles before Cargo starts.
# PowerShell owns this boundary because compiling a Rust helper would itself consume the
# flags being selected. CI callers use the companion's child-process environment instead.
# Ref: docs/benchmarks.md, "Compiler layout policy".
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

function Get-BenchmarkEncodedRustFlagSet {
    # Keeps Cargo's effective argument boundaries, removes competing alignment policies,
    # and optionally applies the workspace's normal measurement policy.
    [CmdletBinding()]
    [OutputType([string])]
    param(
        [AllowEmptyString()][string] $RustFlags = '',
        # An untyped parameter preserves the distinction between an absent and empty variable.
        [AllowNull()] $EncodedRustFlags = $null,
        [AllowEmptyString()][string] $StabilityFlags = ''
    )

    $separator = [string][char]0x1f
    if ($null -ne $EncodedRustFlags -and $EncodedRustFlags -isnot [string]) {
        throw 'CARGO_ENCODED_RUSTFLAGS must be a string or absent.'
    }
    if ($StabilityFlags.Contains($separator) -or
        ($null -eq $EncodedRustFlags -and $RustFlags.Contains($separator))) {
        throw 'Plain Rust flags must not contain the encoded argument separator.'
    }

    [string[]] $arguments = @()
    if ($null -ne $EncodedRustFlags) {
        if ($EncodedRustFlags.Length -ne 0) {
            $arguments = $EncodedRustFlags -split $separator
        }
    } else {
        $arguments = @($RustFlags -split '\s+' | Where-Object { $_ -cne '' })
    }
    $kept = [System.Collections.Generic.List[string]]::new()

    for ($index = 0; $index -lt $arguments.Count; $index++) {
        $argument = $arguments[$index]
        $separate = $false
        $prefix = ''
        $llvmArguments = $null

        if ($argument -cin @('-C', '--codegen') -and $index + 1 -lt $arguments.Count -and
            $arguments[$index + 1] -cmatch '^llvm-args=(.*)$') {
            $llvmArguments = $Matches[1]
            $separate = $true
            $index++
        } elseif ($argument -cmatch '^(-C|--codegen=)llvm-args=(.*)$') {
            $prefix = $Matches[1]
            $llvmArguments = $Matches[2]
        }

        if ($null -eq $llvmArguments) {
            $kept.Add($argument)
            continue
        }

        # Keep unknown or malformed options for rustc to diagnose rather than hiding errors.
        $remaining = ($llvmArguments -creplace `
            '(^|\s)-align-all-(?:functions|nofallthru-blocks|blocks)=\d+(?=\s|$)', '').Trim()
        if ($remaining.Length -eq 0) {
            continue
        }
        if ($separate) {
            $kept.Add($argument)
            $kept.Add("llvm-args=$remaining")
        } else {
            $kept.Add("${prefix}llvm-args=$remaining")
        }
    }

    foreach ($argument in @($StabilityFlags -split '\s+' | Where-Object { $_ -cne '' })) {
        $kept.Add($argument)
    }
    return $kept.ToArray() -join $separator
}

Export-ModuleMember -Function Get-BenchmarkEncodedRustFlagSet

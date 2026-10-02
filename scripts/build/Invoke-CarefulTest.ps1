#requires -Version 7.6
# Cargo's runner for `just careful`: run the instrumented test executable with the caller's
# original compiler flags, so nested source-selected Cargo builds do not inherit careful's
# nightly sysroot/instrumentation. PowerShell is already the recipe runtime and can restore
# the child environment without building another Rust helper with the instrumentation.
# Ref: docs/build-and-tooling.md, "Careful test execution".
param(
    [Parameter(Mandatory, Position = 0)]
    [string] $Executable,
    [Parameter(ValueFromRemainingArguments)]
    [string[]] $TestArguments = @()
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

if (-not $env:FOLO_CAREFUL_BUILD_FLAGS) {
    throw 'The careful test runner requires the caller environment captured by just careful.'
}
$flags = $env:FOLO_CAREFUL_BUILD_FLAGS | ConvertFrom-Json -AsHashtable
$start = [Diagnostics.ProcessStartInfo]::new()
$start.FileName = $Executable
$start.WorkingDirectory = (Get-Location).ProviderPath
$start.UseShellExecute = $false
# Rustdoc omits this dispatch variable for custom runners. Its merged harness needs it to
# spawn each example separately; ordinary test executables ignore it. See "Careful test
# execution" in docs/build-and-tooling.md for the pinned rustdoc protocol.
$start.Environment['RUSTDOC_DOCTEST_BIN_PATH'] = $Executable
foreach ($argument in $TestArguments) { $start.ArgumentList.Add($argument) }
foreach ($name in @('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTDOCFLAGS', 'CARGO_ENCODED_RUSTDOCFLAGS')) {
    if (-not $flags.ContainsKey($name)) { throw "Missing captured compiler environment: $name" }
    if ($null -eq $flags[$name]) {
        $null = $start.Environment.Remove($name)
    } else {
        $start.Environment[$name] = $flags[$name]
    }
}
$null = $start.Environment.Remove('FOLO_CAREFUL_BUILD_FLAGS')
$process = [Diagnostics.Process]::new()
$process.StartInfo = $start
try {
    if (-not $process.Start()) { throw 'Could not start the careful test executable.' }
    $process.WaitForExit()
    exit $process.ExitCode
} finally { $process.Dispose() }

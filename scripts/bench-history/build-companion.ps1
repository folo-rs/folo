#requires -Version 7.6

# Builds the source companion used by benchmark workflow preparation. The archive carries
# executable permissions to the Ubuntu posting jobs, which need no Rust or Azure setup.
# Ref: .github/workflows/implementation.md, "Benchmark workflow artifacts".
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string] $ArchivePath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
Import-Module (Join-Path $PSScriptRoot 'BenchHistoryPath.psm1') -Force
Import-Module (Join-Path $PSScriptRoot '..' 'build' 'CargoExecutable.psm1') -Force

# Posting jobs are explicitly x86_64 Ubuntu. Pinning the target, not the toolchain version,
# keeps an ambient Cargo cross-compilation setting from changing the archive layout.
$target = 'x86_64-unknown-linux-gnu'
# Use Cargo's ordinary target directory so the standard environment cache owns the build.
$messages = @(cargo build --locked --package cargo-bench-history-github --bin cargo-bench-history-github --target $target --message-format=json-render-diagnostics)
$binary = Resolve-CargoExecutable -CargoMessage $messages -TargetName 'cargo-bench-history-github'
New-BenchHistoryDirectory -Path (Split-Path -Parent $ArchivePath)
tar -czf $ArchivePath -C (Split-Path -Parent $binary) (Split-Path -Leaf $binary)
$binary

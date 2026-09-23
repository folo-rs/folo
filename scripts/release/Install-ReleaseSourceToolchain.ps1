#requires -Version 7

# Controller-owned rustup adapter called by release-binaries inside the pinned source worktree.
# The active manifest supplies channel/components; the standard installer owns bounded retries.
# Ref: packages/release-binaries/docs/implementation.md#planning-and-execution.
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSReviewUnusedParameter', 'Target',
    Justification = 'Consumed by the retry closure passed to Invoke-WithRetry.')]
param([Parameter(Mandatory)][string] $Target)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

Import-Module (Join-Path $PSScriptRoot '..' 'setup' 'RustToolchain.psm1') -Force
Import-Module (Join-Path $PSScriptRoot '..' 'utility' 'Retry.psm1') -Force

$channel = Get-PinnedRustChannel
Write-Host "Installing source toolchain '$channel' from $((Get-Location).ProviderPath)."
Install-RustupToolchain -InstallProfile minimal
Invoke-WithRetry -Attempt 4 -DelaySeconds 5 -BackoffMultiplier 2 -MaxDelaySeconds 30 -Action {
    rustup target add $Target --toolchain $channel
}

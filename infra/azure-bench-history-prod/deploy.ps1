#Requires -Version 7.6

<#
.SYNOPSIS
    Deploys the Folo production benchmark history stack.
.DESCRIPTION
    Supplies Folo-specific names to the source-built cargo-bench-history setup-azure
    command, exercising its CLI, prerequisite checks and embedded deployment bundle.
    See README.md for privileges, storage preservation and configuration handoff.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string] $SubscriptionId,
    [string] $ResourceGroup = 'folohistory',
    [string] $Location = 'swedencentral',
    [string] $StorageAccountName = 'folohistory',
    [string] $ManagedIdentityName = 'id-folo-bench-history-prod',
    [string] $HistoryContainerName = 'bench-history',
    [string] $GithubOrg = 'folo-rs',
    [string] $GithubRepo = 'folo',
    [string] $HistoryBranch = 'main',
    [string] $LocalPrincipalId = '',
    [ValidateSet('', 'User', 'Group')][string] $LocalPrincipalType = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'

$cargoArgs = @(
    'run', '--manifest-path', (Join-Path $PSScriptRoot '..' '..' 'Cargo.toml'),
    '--package', 'cargo-bench-history', '--bin', 'cargo-bench-history', '--locked', '--',
    'setup-azure',
    '--subscription-id', $SubscriptionId,
    '--resource-group', $ResourceGroup,
    '--location', $Location,
    '--storage-account', $StorageAccountName,
    '--managed-identity', $ManagedIdentityName,
    '--container', $HistoryContainerName,
    '--github-owner', $GithubOrg,
    '--github-repository', $GithubRepo,
    '--history-branch', $HistoryBranch,
    '--verbose'
)
if (-not [string]::IsNullOrEmpty($LocalPrincipalId)) {
    $cargoArgs += @('--local-principal-id', $LocalPrincipalId)
}
if (-not [string]::IsNullOrEmpty($LocalPrincipalType)) {
    $cargoArgs += @('--local-principal-type', $LocalPrincipalType.ToLowerInvariant())
}
cargo @cargoArgs

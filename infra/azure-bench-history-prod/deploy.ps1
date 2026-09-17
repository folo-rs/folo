#Requires -Version 7.6

<#
.SYNOPSIS
    Deploys the Folo production benchmark history stack.
.DESCRIPTION
    Supplies Folo-specific names to the canonical standalone deployment driver
    embedded by cargo-bench-history setup-azure. It requires no compiled Rust.
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

$parameters = @{
    SubscriptionId = $SubscriptionId
    ResourceGroup = $ResourceGroup
    Location = $Location
    StorageAccountName = $StorageAccountName
    ManagedIdentityName = $ManagedIdentityName
    HistoryContainerName = $HistoryContainerName
    GithubOrg = $GithubOrg
    GithubRepo = $GithubRepo
    HistoryBranch = $HistoryBranch
    LocalPrincipalId = $LocalPrincipalId
    LocalPrincipalType = $LocalPrincipalType
}
& (Join-Path $PSScriptRoot '..' '..' 'packages' 'cargo-bench-history' 'src' 'azure_bundle' 'deploy.ps1') @parameters -Verbose

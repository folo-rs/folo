#Requires -Version 7.6

<#
.SYNOPSIS
    Deploys the Folo Azure resources used by real-Azure storage tests.
.DESCRIPTION
    Supplies test-specific parameters to the source-built cargo-bench-history
    setup-azure command. The command owns prerequisite checks and the shared
    deployment bundle; this wrapper performs no separate Azure operations.
    Existing storage settings and data are preserved. See README.md for the
    test account's isolation and non-secret constants.env handoff.
.PARAMETER SubscriptionId
    Required ID of an existing subscription, for example
    00000000-0000-0000-0000-000000000001.
.PARAMETER ResourceGroup
    Optional group to create or reuse; defaults to rg-folo-bench-history.
.PARAMETER Location
    Optional region for deployed resources, for example westeurope.
    Defaults to swedencentral; match existing resources on updates.
.PARAMETER StorageAccountName
    Required globally unique account name, for example folovalidate.
    Created if absent; existing account properties and data are preserved.
.PARAMETER ManagedIdentityName
    Optional identity to create or reuse; defaults to id-folo-bench-history-ci.
    Keep this separate from the production history identity.
.PARAMETER HistoryContainerName
    Optional named container, created if absent; defaults to bench-history.
    Real-Azure tests use their own isolated containers rather than this container.
.PARAMETER GithubOrg
    Optional existing GitHub owner; defaults to folo-rs.
.PARAMETER GithubRepo
    Optional existing repository name without owner; defaults to folo.
.PARAMETER HistoryBranch
    Optional literal GitHub branch permitted to federate; defaults to main.
.PARAMETER CustomPrincipalId
    Optional existing Entra user/group object ID in the subscription's tenant.
    Supply CustomPrincipalType too. Grants additional account-scoped blob access.
.PARAMETER CustomPrincipalType
    Required with CustomPrincipalId: User or Group.
.PARAMETER CurrentUser
    Optional switch granting the Azure CLI signed-in user additional access.
    Conflicts with either custom principal flag.
.EXAMPLE
    ./deploy.ps1 -SubscriptionId 00000000-0000-0000-0000-000000000001 `
        -StorageAccountName folovalidate -CurrentUser
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string] $SubscriptionId,
    [string] $ResourceGroup = 'rg-folo-bench-history',
    [string] $Location = 'swedencentral',
    [Parameter(Mandatory)][string] $StorageAccountName,
    [string] $ManagedIdentityName = 'id-folo-bench-history-ci',
    [string] $HistoryContainerName = 'bench-history',
    [string] $GithubOrg = 'folo-rs',
    [string] $GithubRepo = 'folo',
    [string] $HistoryBranch = 'main',
    [string] $CustomPrincipalId = '',
    [ValidateSet('', 'User', 'Group')][string] $CustomPrincipalType = '',
    [switch] $CurrentUser
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
if (-not [string]::IsNullOrEmpty($CustomPrincipalId)) {
    $cargoArgs += @('--custom-principal-id', $CustomPrincipalId)
}
if (-not [string]::IsNullOrEmpty($CustomPrincipalType)) {
    $cargoArgs += @('--custom-principal-type', $CustomPrincipalType.ToLowerInvariant())
}
if ($CurrentUser) {
    $cargoArgs += '--current-user'
}
cargo @cargoArgs

Write-Output 'For this test deployment, record the printed account as BENCH_HISTORY_TEST_AZURE_ACCOUNT and client ID as AZURE_TEST_CLIENT_ID in constants.env.'
Write-Output 'Record the printed tenant and subscription IDs as AZURE_TENANT_ID and AZURE_SUBSCRIPTION_ID. Do not replace the production storage configuration in .cargo/bench_history.toml with this test account.'

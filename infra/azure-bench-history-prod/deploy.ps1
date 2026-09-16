#Requires -Version 7.6

<#
.SYNOPSIS
    Deploys (or updates) the Azure resources that hold the nightly
    cargo-bench-history benchmark history.

.DESCRIPTION
    Maintainer entry point for reader-first production provisioning. Bicep defines
    the resources; ProductionIdentityDeployment.psm1 preserves existing storage
    and writer PR trust and performs only explicitly requested retirement.
    PowerShell is the Azure CLI boundary and needs no prepared Rust toolchain.

    Requires installed Azure CLI and Bicep, plus an authenticated session allowed
    to create the resources and assign roles. No tools are automatically installed.

.PARAMETER SubscriptionId
    Target subscription id.

.PARAMETER ResourceGroup
    Resource group to create/use. Defaults to 'folohistory'.

.PARAMETER Location
    Azure region. Defaults to 'swedencentral'.

.PARAMETER StorageAccountName
    Globally-unique Storage account name (3-24 lowercase alphanumerics). Defaults to
    'folohistory'.

.PARAMETER ManagedIdentityName
    Name of the user-assigned managed identity used by the nightly workflow. Defaults
    to 'id-folo-bench-history-prod'.

.PARAMETER ReaderManagedIdentityName
    Dedicated reader identity name. Defaults to ManagedIdentityName plus '-reader'.

.PARAMETER HistoryContainerName
    Container receiving the reader role. Defaults to 'bench-history', matching
    .cargo/bench_history.toml. A missing container is provisioned before the grant.

.PARAMETER RetireWriterPullRequestTrust
    Explicitly deletes only the writer's github-pull-request federated credential
    after a successful deployment. Use only after legacy PR-writing runs drain
    and replacement workflows use the reader. Main/backfill writer access remains.
    Ordinary repeat deployments preserve this credential's absence.

.PARAMETER LocalPrincipalId
    Object id of a local developer principal (user or group) to grant data access.
    Omit to grant CI access only. Tip: your own user id is
    `az ad signed-in-user show --query id -o tsv`.

.PARAMETER LocalPrincipalType
    'User' (default) or 'Group', matching LocalPrincipalId.

.EXAMPLE
    .\deploy.ps1 -SubscriptionId 00000000-0000-0000-0000-000000000000 `
        -LocalPrincipalId (az ad signed-in-user show --query id -o tsv)
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string] $SubscriptionId,

    [string] $ResourceGroup = 'folohistory',

    [string] $Location = 'swedencentral',

    [ValidatePattern('^[a-z0-9]{3,24}$', Options = 'None')]
    [string] $StorageAccountName = 'folohistory',

    [string] $ManagedIdentityName = 'id-folo-bench-history-prod',

    [string] $ReaderManagedIdentityName = '',

    [string] $HistoryContainerName = 'bench-history',

    [string] $GithubOrg = 'folo-rs',

    [string] $GithubRepo = 'folo',

    [string] $LocalPrincipalId = '',

    [ValidateSet('User', 'Group')]
    [string] $LocalPrincipalType = 'User',

    [switch] $RetireWriterPullRequestTrust
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'

Import-Module (Join-Path $PSScriptRoot '..' '..' 'scripts' 'bench-history' 'ProductionIdentityDeployment.psm1') -Force
$outputs = Invoke-ProductionIdentityDeployment @PSBoundParameters

Write-Host ''
Write-Host 'Deployment complete.' -ForegroundColor Green
Write-Host ''
Write-Host 'Record these non-secret identifiers in repository configuration.' -ForegroundColor Cyan
Write-Host 'Reader provisioning alone does not activate replacement PR workflows.' -ForegroundColor Cyan
Write-Host "  .cargo/bench_history.toml -> [storage.azure] account = `"$($outputs.storageAccountName.value)`""
Write-Host "  .cargo/bench_history.toml -> [storage.azure] container = `"$($outputs.historyContainerName.value)`""
Write-Host "  AZURE_PROD_CLIENT_ID=$($outputs.managedIdentityClientId.value)"
Write-Host "  AZURE_PROD_READER_CLIENT_ID=$($outputs.readerManagedIdentityClientId.value)"
Write-Host "  AZURE_TENANT_ID=$($outputs.tenantId.value)"
Write-Host "  AZURE_SUBSCRIPTION_ID=$($outputs.subscriptionId.value)"
Write-Host ''
Write-Host "Blob endpoint: $($outputs.blobEndpoint.value)"

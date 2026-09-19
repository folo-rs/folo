#Requires -Version 7.6

<#
.SYNOPSIS
    Deploys (or updates) the Azure resources backing the cargo-bench-history
    real-Azure storage tests.

.DESCRIPTION
    Idempotently provisions a resource group, an Entra-only Storage account, a
    user-assigned managed identity with GitHub OIDC federated credentials, and the
    `Storage Blob Data Contributor` role assignments needed by CI and (optionally) an
    additional user or group. Re-running it converges to the same state, so the
    paired `teardown.ps1` + this script let you delete and re-create everything at
    will.

    Uses the canonical bundle's prerequisite and current-user checks, but keeps
    the throwaway test storage lifecycle and CI identity separate from production.
    Requires Azure CLI, installed Bicep and an authenticated session with rights
    to create resources and role assignments. See README.md for that boundary.

.PARAMETER SubscriptionId
    Required ID of an existing subscription, for example
    00000000-0000-0000-0000-000000000001.

.PARAMETER ResourceGroup
    Optional resource group to create/reuse, for example rg-team-tests.
    Defaults to rg-folo-bench-history.

.PARAMETER Location
    Optional Azure region name for deployed resources, for example westeurope.
    Defaults to swedencentral; match existing resources on updates.

.PARAMETER StorageAccountName
    Required globally unique account name (3-24 lowercase alphanumerics), for
    example teamhistorytests. Created or updated with the test storage policy.

.PARAMETER ManagedIdentityName
    Optional identity name, for example id-team-history-ci. Created or updated
    in the selected resource group; defaults to id-folo-bench-history-ci.

.PARAMETER GithubOrg
    Optional existing GitHub owner, for example example-org. Defaults to folo-rs.
    Selects the OIDC repository subject; does not create GitHub resources.

.PARAMETER GithubRepo
    Optional existing repository name without owner, for example project.
    Defaults to folo; selects OIDC trust without modifying GitHub.

.PARAMETER CustomPrincipalId
    Optional existing Entra user/group object ID in the subscription's tenant,
    for example 00000000-0000-0000-0000-000000000002. Supply CustomPrincipalType too.
    Grants account-scoped blob contributor access, without creating a principal.

.PARAMETER CustomPrincipalType
    Required with CustomPrincipalId: User or Group. Conflicts with CurrentUser.

.PARAMETER CurrentUser
    Optional switch resolving the Azure CLI signed-in user in the explicit
    subscription's tenant before any Azure changes. Grants additional access;
    requires that subscription to be active in Azure CLI and conflicts with either
    custom principal flag.

.EXAMPLE
    ./deploy.ps1 -SubscriptionId 00000000-0000-0000-0000-000000000000 `
        -StorageAccountName stfolobenchhist `
        -CurrentUser
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string] $SubscriptionId,

    [string] $ResourceGroup = 'rg-folo-bench-history',

    [string] $Location = 'swedencentral',

    [Parameter(Mandatory)]
    [ValidatePattern('^[a-z0-9]{3,24}$')]
    [string] $StorageAccountName,

    [string] $ManagedIdentityName = 'id-folo-bench-history-ci',

    [string] $GithubOrg = 'folo-rs',

    [string] $GithubRepo = 'folo',

    [string] $CustomPrincipalId = '',

    [ValidateSet('', 'User', 'Group')]
    [string] $CustomPrincipalType = '',

    [switch] $CurrentUser
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path

if ($CurrentUser -and ($CustomPrincipalId -or $CustomPrincipalType)) {
    throw 'CurrentUser cannot be combined with CustomPrincipalId or CustomPrincipalType.'
}
if ([string]::IsNullOrEmpty($CustomPrincipalId) -ne [string]::IsNullOrEmpty($CustomPrincipalType)) {
    throw 'CustomPrincipalId and CustomPrincipalType must be supplied together.'
}
Import-Module (Join-Path $PSScriptRoot '..' '..' 'packages' 'cargo-bench-history' 'src' 'azure_bundle' 'ProductionIdentityDeployment.psm1') -Force
$context = Get-AzureDeploymentContext -SubscriptionId $SubscriptionId -CurrentUser:$CurrentUser
if ($CurrentUser) {
    $CustomPrincipalId = $context.CurrentUserPrincipalId
    $CustomPrincipalType = 'User'
}

Write-Verbose "Ensuring resource group '$ResourceGroup' in '$Location' only after shared tooling, subscription and optional user checks succeeded."
az group create --subscription $SubscriptionId --name $ResourceGroup --location $Location --output none

Write-Verbose 'Exporting parameters for main.bicepparam (readEnvironmentVariable).'
$env:AZURE_STORAGE_ACCOUNT_NAME = $StorageAccountName
$env:AZURE_LOCATION = $Location
$env:AZURE_MANAGED_IDENTITY_NAME = $ManagedIdentityName
$env:GITHUB_ORG = $GithubOrg
$env:GITHUB_REPO = $GithubRepo
$env:AZURE_CUSTOM_PRINCIPAL_ID = $CustomPrincipalId
$env:AZURE_CUSTOM_PRINCIPAL_TYPE = if ($CustomPrincipalType) { $CustomPrincipalType } else { 'User' }

if ([string]::IsNullOrEmpty($CustomPrincipalId)) {
    Write-Verbose 'No additional principal supplied; granting data access to the CI identity only.'
}
else {
    Write-Verbose "Granting account-scoped data access to additional $CustomPrincipalType '$CustomPrincipalId'."
}

$bicepFile = Join-Path $scriptDir 'main.bicep'
$paramFile = Join-Path $scriptDir 'main.bicepparam'
$deploymentName = "bench-history-$([DateTimeOffset]::UtcNow.ToUnixTimeSeconds())"

Write-Verbose "Deploying '$bicepFile' as '$deploymentName'."
$outputJson = az deployment group create `
    --subscription $SubscriptionId `
    --resource-group $ResourceGroup `
    --name $deploymentName `
    --mode Incremental `
    --template-file $bicepFile `
    --parameters $paramFile `
    --query properties.outputs `
    --output json
$outputs = $outputJson | ConvertFrom-Json

Write-Host ''
Write-Host 'Deployment complete.' -ForegroundColor Green
Write-Host ''
Write-Host 'These identifiers are committed (non-secret) in constants.env, shared by' -ForegroundColor Cyan
Write-Host 'local `just test-azure` runs and the CI `test-azure` job. If you re-created' -ForegroundColor Cyan
Write-Host 'the resources, update constants.env to match:' -ForegroundColor Cyan
Write-Host "  BENCH_HISTORY_TEST_AZURE_ACCOUNT=$($outputs.storageAccountName.value)"
Write-Host "  AZURE_TEST_CLIENT_ID=$($outputs.managedIdentityClientId.value)"
Write-Host "  AZURE_TENANT_ID=$($outputs.tenantId.value)"
Write-Host "  AZURE_SUBSCRIPTION_ID=$($outputs.subscriptionId.value)"
Write-Host ''
Write-Host 'Then run the tests with `az login` followed by `just test-azure`.' -ForegroundColor Cyan
Write-Host ''
Write-Host "Blob endpoint: $($outputs.blobEndpoint.value)"

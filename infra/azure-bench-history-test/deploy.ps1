#requires -Version 7

<#
.SYNOPSIS
    Deploys (or updates) the Azure resources backing the cargo-bench-history
    real-Azure storage tests.

.DESCRIPTION
    Idempotently provisions a resource group, an Entra-only Storage account, a
    user-assigned managed identity with GitHub OIDC federated credentials, and the
    `Storage Blob Data Contributor` role assignments needed by CI and (optionally) a
    local developer principal. Re-running it converges to the same state, so the
    paired `teardown.ps1` + this script let you delete and re-create everything at
    will.

    Requires the Azure CLI (`az`) and an authenticated session (`az login`) for an
    account with rights to create the resources and role assignments.

.PARAMETER SubscriptionId
    Target subscription id.

.PARAMETER ResourceGroup
    Resource group to create/use. Defaults to 'rg-folo-bench-history'.

.PARAMETER Location
    Azure region. Defaults to 'swedencentral'.

.PARAMETER StorageAccountName
    Globally-unique Storage account name (3-24 lowercase alphanumerics).

.PARAMETER LocalPrincipalId
    Object id of a local developer principal (user or group) to grant data access.
    Omit to grant CI access only. Tip: your own user id is
    `az ad signed-in-user show --query id -o tsv`.

.PARAMETER LocalPrincipalType
    'User' (default) or 'Group', matching LocalPrincipalId.

.EXAMPLE
    ./deploy.ps1 -SubscriptionId 00000000-0000-0000-0000-000000000000 `
        -StorageAccountName stfolobenchhist `
        -LocalPrincipalId (az ad signed-in-user show --query id -o tsv)
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

    [string] $LocalPrincipalId = '',

    [ValidateSet('User', 'Group')]
    [string] $LocalPrincipalType = 'User'
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path

Write-Verbose "Selecting subscription $SubscriptionId."
az account set --subscription $SubscriptionId

Write-Verbose "Ensuring resource group '$ResourceGroup' exists in '$Location'."
az group create --name $ResourceGroup --location $Location --output none

Write-Verbose 'Exporting parameters for main.bicepparam (readEnvironmentVariable).'
$env:AZURE_STORAGE_ACCOUNT_NAME = $StorageAccountName
$env:AZURE_LOCATION = $Location
$env:AZURE_MANAGED_IDENTITY_NAME = $ManagedIdentityName
$env:GITHUB_ORG = $GithubOrg
$env:GITHUB_REPO = $GithubRepo
$env:AZURE_LOCAL_PRINCIPAL_ID = $LocalPrincipalId
$env:AZURE_LOCAL_PRINCIPAL_TYPE = $LocalPrincipalType

if ([string]::IsNullOrEmpty($LocalPrincipalId)) {
    Write-Verbose 'No LocalPrincipalId supplied; granting data access to the CI identity only.'
}
else {
    Write-Verbose "Granting data access to local $LocalPrincipalType '$LocalPrincipalId'."
}

$bicepFile = Join-Path $scriptDir 'main.bicep'
$paramFile = Join-Path $scriptDir 'main.bicepparam'
$deploymentName = "bench-history-$([DateTimeOffset]::UtcNow.ToUnixTimeSeconds())"

Write-Verbose "Deploying '$bicepFile' as '$deploymentName'."
$outputJson = az deployment group create `
    --resource-group $ResourceGroup `
    --name $deploymentName `
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
Write-Host "  AZURE_CLIENT_ID=$($outputs.managedIdentityClientId.value)"
Write-Host "  AZURE_TENANT_ID=$($outputs.tenantId.value)"
Write-Host "  AZURE_SUBSCRIPTION_ID=$($outputs.subscriptionId.value)"
Write-Host ''
Write-Host 'Then run the tests with `az login` followed by `just test-azure`.' -ForegroundColor Cyan
Write-Host ''
Write-Host "Blob endpoint: $($outputs.blobEndpoint.value)"

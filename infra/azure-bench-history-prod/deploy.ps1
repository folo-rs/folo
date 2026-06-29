#requires -Version 7

<#
.SYNOPSIS
    Deploys (or updates) the Azure storage account that holds the nightly
    cargo-bench-history benchmark history.

.DESCRIPTION
    Idempotently provisions a resource group and an Entra-only Storage account, then
    grants the existing CI managed identity (reused from infra/azure-bench-history-test)
    and an optional local developer principal the `Storage Blob Data Contributor`
    role on it. Re-running it converges to the same state, so the paired
    `teardown.ps1` + this script let you delete and re-create everything at will.

    This script does NOT create a managed identity or federated credentials: the
    nightly `bench-history` workflow reuses the CI identity that
    infra/azure-bench-history-test/deploy.ps1 created, whose `main`-branch federated
    credential already matches a scheduled run's OIDC subject. This script only
    resolves that identity's principal id and grants it data access here. Deploy the
    test infra first (so the identity exists) if you have not already.

    Requires the Azure CLI (`az`) and an authenticated session (`az login`) for an
    account with rights to create the resources and role assignments.

.PARAMETER SubscriptionId
    Target subscription id.

.PARAMETER ResourceGroup
    Resource group to create/use. Defaults to 'folohistory'.

.PARAMETER Location
    Azure region. Defaults to 'swedencentral'.

.PARAMETER StorageAccountName
    Globally-unique Storage account name (3-24 lowercase alphanumerics). Defaults to
    'folohistory'.

.PARAMETER CiIdentityResourceGroup
    Resource group holding the existing CI managed identity. Defaults to
    'rg-folo-bench-history'.

.PARAMETER CiIdentityName
    Name of the existing CI managed identity. Defaults to 'id-folo-bench-history-ci'.

.PARAMETER LocalPrincipalId
    Object id of a local developer principal (user or group) to grant data access.
    Omit to grant CI access only. Tip: your own user id is
    `az ad signed-in-user show --query id -o tsv`.

.PARAMETER LocalPrincipalType
    'User' (default) or 'Group', matching LocalPrincipalId.

.EXAMPLE
    ./deploy.ps1 -SubscriptionId 00000000-0000-0000-0000-000000000000 `
        -LocalPrincipalId (az ad signed-in-user show --query id -o tsv)
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string] $SubscriptionId,

    [string] $ResourceGroup = 'folohistory',

    [string] $Location = 'swedencentral',

    [ValidatePattern('^[a-z0-9]{3,24}$')]
    [string] $StorageAccountName = 'folohistory',

    [string] $CiIdentityResourceGroup = 'rg-folo-bench-history',

    [string] $CiIdentityName = 'id-folo-bench-history-ci',

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

Write-Verbose "Resolving the principal id of CI identity '$CiIdentityName' in '$CiIdentityResourceGroup'."
$ciPrincipalId = az identity show `
    --resource-group $CiIdentityResourceGroup `
    --name $CiIdentityName `
    --query principalId `
    --output tsv
if ([string]::IsNullOrWhiteSpace($ciPrincipalId)) {
    Write-Error "Could not resolve the CI managed identity '$CiIdentityName' in '$CiIdentityResourceGroup'. Deploy infra/azure-bench-history-test/deploy.ps1 first so the identity exists."
}
Write-Verbose "CI principal id: $ciPrincipalId."

Write-Verbose "Ensuring resource group '$ResourceGroup' exists in '$Location'."
az group create --name $ResourceGroup --location $Location --output none

Write-Verbose 'Exporting parameters for main.bicepparam (readEnvironmentVariable).'
$env:AZURE_STORAGE_ACCOUNT_NAME = $StorageAccountName
$env:AZURE_LOCATION = $Location
$env:AZURE_CI_PRINCIPAL_ID = $ciPrincipalId
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
$deploymentName = "bench-history-data-$([DateTimeOffset]::UtcNow.ToUnixTimeSeconds())"

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
Write-Host 'Record the account name (non-secret) in constants.env; the nightly' -ForegroundColor Cyan
Write-Host 'bench-history workflow reuses the existing AZURE_CLIENT_ID / AZURE_TENANT_ID /' -ForegroundColor Cyan
Write-Host 'AZURE_SUBSCRIPTION_ID for OIDC sign-in:' -ForegroundColor Cyan
Write-Host "  BENCH_HISTORY_PROD_AZURE_ACCOUNT=$($outputs.storageAccountName.value)"
Write-Host ''
Write-Host "Blob endpoint: $($outputs.blobEndpoint.value)"

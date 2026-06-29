#requires -Version 7

<#
.SYNOPSIS
    Deletes the Azure resources backing the cargo-bench-history real-Azure storage
    tests by removing their resource group.

.DESCRIPTION
    Deletes the whole resource group (storage account, managed identity, federated
    credentials, and role assignments) so the environment can be re-created from
    scratch with `deploy.ps1`. Requires the Azure CLI (`az`) and an authenticated
    session.

.PARAMETER SubscriptionId
    Target subscription id.

.PARAMETER ResourceGroup
    Resource group to delete. Defaults to 'rg-folo-bench-history'.

.PARAMETER Wait
    Block until the deletion completes instead of returning while it runs.

.EXAMPLE
    ./teardown.ps1 -SubscriptionId 00000000-0000-0000-0000-000000000000
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string] $SubscriptionId,

    [string] $ResourceGroup = 'rg-folo-bench-history',

    [switch] $Wait
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'

Write-Verbose "Selecting subscription $SubscriptionId."
az account set --subscription $SubscriptionId

$exists = az group exists --name $ResourceGroup
if ($exists -ne 'true') {
    Write-Host "Resource group '$ResourceGroup' does not exist; nothing to delete." -ForegroundColor Yellow
    return
}

Write-Verbose "Deleting resource group '$ResourceGroup'."
$deleteArgs = @('group', 'delete', '--name', $ResourceGroup, '--yes')
if (-not $Wait) {
    $deleteArgs += '--no-wait'
}
az @deleteArgs

if ($Wait) {
    Write-Host "Resource group '$ResourceGroup' deleted." -ForegroundColor Green
}
else {
    Write-Host "Deletion of resource group '$ResourceGroup' started (running in the background)." -ForegroundColor Green
}

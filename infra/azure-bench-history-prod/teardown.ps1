#requires -Version 7

<#
.SYNOPSIS
    Deletes the Azure storage account holding the nightly cargo-bench-history
    benchmark history by removing its resource group.

.DESCRIPTION
    Deletes the whole resource group (the storage account and its role assignments)
    so the environment can be re-created from scratch with `deploy.ps1`. The CI
    managed identity is NOT affected — it lives in the test infra's resource group
    (infra/azure-bench-history-test) and is only referenced here. Requires the Azure CLI
    (`az`) and an authenticated session.

    Note: this permanently deletes the collected benchmark history. The data is
    reconstructible (`cargo bench-history backfill` can re-bench past commits), but
    re-running the nightly collection only repopulates history going forward.

.PARAMETER SubscriptionId
    Target subscription id.

.PARAMETER ResourceGroup
    Resource group to delete. Defaults to 'folohistory'.

.PARAMETER Wait
    Block until the deletion completes instead of returning while it runs.

.EXAMPLE
    ./teardown.ps1 -SubscriptionId 00000000-0000-0000-0000-000000000000
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string] $SubscriptionId,

    [string] $ResourceGroup = 'folohistory',

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

#Requires -Version 7.6

# Native bundle integration fixture called by cbh_setup_azure.rs. Shadows Azure
# CLI inside this process and executes only the exported scripts, validating
# literal parameter handoff and fresh/repeated discovery without Azure access.
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidGlobalVars', '', Justification = 'This fixture owns a disposable child process; one shared state object lets the module-scoped az calls reach the isolated fake without modifying production code.')]
[CmdletBinding()]
param([Parameter(Mandatory)][string] $BundleDirectory)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'

# Module imports have their own script scope. The fixture runs in a disposable
# child process, so one explicitly global state object keeps the az fake shared.
$global:AzureSetupFixture = @{
    DeploymentCount = 0
    Calls = [System.Collections.Generic.List[string]]::new()
    Expected = (Get-Content -LiteralPath (Join-Path $BundleDirectory 'parameters.json') -Raw | ConvertFrom-Json)
    BundleDirectory = $BundleDirectory
}

function global:az {
    $global:LASTEXITCODE = 0
    $operation = if ($args[0] -eq 'version') { 'version' } else { $args[0..1] -join ' ' }
    if ($operation -in @('storage account', 'storage container-rm', 'deployment group')) {
        $operation = $args[0..2] -join ' '
    }
    $global:AzureSetupFixture.Calls.Add($operation)
    if ($args[0] -notin @('version', 'bicep')) {
        if ($args[[array]::IndexOf($args, '--subscription') + 1] -ne $global:AzureSetupFixture.Expected.SubscriptionId) {
            throw 'Selected subscription was not forwarded.'
        }
    }
    switch ($operation) {
        'version' { return '{}' }
        'bicep version' { return 'Installed Bicep' }
        'account show' {
            return ConvertTo-Json @{ id = $global:AzureSetupFixture.Expected.SubscriptionId; state = 'Enabled'; tenantId = 'tenant-canary' }
        }
        'account get-access-token' { return 'expiry-not-token' }
        'group create' {
            if ($args[[array]::IndexOf($args, '--name') + 1] -cne $global:AzureSetupFixture.Expected.ResourceGroup) {
                throw 'Literal resource group was altered.'
            }
            return
        }
        'storage account list' {
            if ($global:AzureSetupFixture.DeploymentCount -eq 0) { return '[]' }
            return ConvertTo-Json -InputObject @(@{ name = $global:AzureSetupFixture.Expected.StorageAccountName })
        }
        'storage container-rm list' {
            return ConvertTo-Json -InputObject @(@{ name = $global:AzureSetupFixture.Expected.HistoryContainerName })
        }
        'deployment group create' {
            $parameters = @{}
            $start = [array]::IndexOf($args, '--parameters') + 1
            $end = [array]::IndexOf($args, '--query')
            foreach ($argument in $args[$start..($end - 1)]) {
                $key, $value = $argument.Split('=', 2)
                $parameters[$key] = $value
            }
            $bootstrap = ($global:AzureSetupFixture.DeploymentCount -eq 0).ToString().ToLowerInvariant()
            if ($parameters.createStorageAccount -ne $bootstrap -or
                $parameters.createHistoryContainer -ne $bootstrap) {
                throw 'Fresh/repeated storage decision was incorrect.'
            }
            if ($parameters.managedIdentityName -ne "id-$($global:AzureSetupFixture.Expected.StorageAccountName)-bench-history" -or
                $parameters.historyBranch -cne $global:AzureSetupFixture.Expected.HistoryBranch) {
                throw 'Identity or branch handoff was incorrect.'
            }
            $template = $args[[array]::IndexOf($args, '--template-file') + 1]
            if ((Split-Path $template -Parent) -cne $global:AzureSetupFixture.BundleDirectory) {
                throw 'Deployment escaped the exported bundle.'
            }
            $global:AzureSetupFixture.DeploymentCount++
            return ConvertTo-Json @{
                storageAccountName = @{ value = $global:AzureSetupFixture.Expected.StorageAccountName }
                historyContainerName = @{ value = $global:AzureSetupFixture.Expected.HistoryContainerName }
                blobEndpoint = @{ value = 'https://examplehistory.blob.core.windows.net/' }
                managedIdentityClientId = @{ value = 'client-canary' }
                managedIdentityPrincipalId = @{ value = 'principal-canary' }
                tenantId = @{ value = 'tenant-canary' }
                subscriptionId = @{ value = $global:AzureSetupFixture.Expected.SubscriptionId }
            }
        }
        default { throw "Unmocked Azure operation: $operation" }
    }
}

& (Join-Path $BundleDirectory 'deploy.ps1')
& (Join-Path $BundleDirectory 'deploy.ps1')
if ($global:AzureSetupFixture.DeploymentCount -ne 2) { throw 'The standalone driver did not deploy twice.' }
Write-Output 'standalone-repeat-canary'

#Requires -Version 7.6

<#
.SYNOPSIS
    Provisions storage and one GitHub-federated identity for benchmark history.
.DESCRIPTION
    Standalone entry point shared by setup-azure, its exported bundle, and Folo's
    maintainer wrapper. JSON parameters are data, never PowerShell expressions.
    Explicit flags override the parameter file. Missing required values fail
    before tool probes, without prompting or choosing an active subscription.
    See README.md for prerequisites and non-secret configuration handoff.
.EXAMPLE
    ./deploy.ps1 -ParametersFile ./parameters.json
#>
[CmdletBinding()]
param(
    [string] $ParametersFile = (Join-Path $PSScriptRoot 'parameters.json'),
    [string] $SubscriptionId,
    [string] $ResourceGroup,
    [string] $Location,
    [string] $StorageAccountName,
    [string] $GithubOrg,
    [string] $GithubRepo,
    [string] $HistoryBranch,
    [string] $HistoryContainerName,
    [string] $ManagedIdentityName,
    [string] $LocalPrincipalId,
    [string] $LocalPrincipalType
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = if ($PSBoundParameters.ContainsKey('Verbose')) { 'Continue' } else { 'SilentlyContinue' }

Import-Module (Join-Path $PSScriptRoot 'ProductionIdentityDeployment.psm1') -Force
$parameters = @{}
$known = @('SubscriptionId', 'ResourceGroup', 'Location', 'StorageAccountName',
    'GithubOrg', 'GithubRepo', 'HistoryBranch', 'HistoryContainerName',
    'ManagedIdentityName', 'LocalPrincipalId', 'LocalPrincipalType')
# Keep singleton arrays intact so they cannot masquerade as a parameter object.
$inputParameters = Get-Content -LiteralPath $ParametersFile -Raw | ConvertFrom-Json -AsHashtable -NoEnumerate
if ($inputParameters -isnot [System.Collections.IDictionary]) {
    throw "Deployment parameter file '$ParametersFile' must contain a JSON object."
}
# Enumerate entries directly: a manually added "Keys" member must not hide keys.
foreach ($entry in $inputParameters.GetEnumerator()) {
    $key = $entry.Key
    if ($key -notin $known) { throw "Unknown deployment parameter '$key'." }
    $value = $entry.Value
    if ($null -ne $value) {
        if ($value -isnot [string]) { throw "Deployment parameter '$key' must be a string or null." }
        $parameters[$key] = $value
    }
}
foreach ($key in $known) {
    if ($PSBoundParameters.ContainsKey($key)) { $parameters[$key] = $PSBoundParameters[$key] }
}
foreach ($key in @('SubscriptionId', 'ResourceGroup', 'Location', 'StorageAccountName',
        'GithubOrg', 'GithubRepo', 'HistoryBranch')) {
    if (-not $parameters.ContainsKey($key) -or [string]::IsNullOrWhiteSpace($parameters[$key])) {
        throw "Supply '$key' explicitly in '$ParametersFile' or as a script parameter before deploying."
    }
}

$outputs = Invoke-ProductionIdentityDeployment @parameters
Write-Output 'Deployment complete. These identifiers are non-secret; no repository or GitHub settings were changed.'
Write-Output '[storage.azure]'
Write-Output "account = `"$($outputs.storageAccountName.value)`""
Write-Output "container = `"$($outputs.historyContainerName.value)`""
Write-Output "Blob endpoint: $($outputs.blobEndpoint.value)"
Write-Output "Azure tenant ID: $($outputs.tenantId.value)"
Write-Output "Azure subscription ID: $($outputs.subscriptionId.value)"
Write-Output "Managed identity client ID: $($outputs.managedIdentityClientId.value)"
Write-Output "Managed identity principal ID: $($outputs.managedIdentityPrincipalId.value)"
Write-Output 'Copy the storage settings into .cargo/bench_history.toml. Use the client, tenant and subscription IDs for workflow OIDC login.'

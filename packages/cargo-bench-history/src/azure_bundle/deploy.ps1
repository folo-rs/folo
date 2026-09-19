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
.PARAMETER ParametersFile
    Optional path to an existing JSON parameter object; defaults to parameters.json
    beside this script. Example: ./team-history.json. Explicit flags override its values.
.PARAMETER SubscriptionId
    Required here or in the parameter file. ID of an existing Azure subscription,
    for example 00000000-0000-0000-0000-000000000001. Never inferred from the CLI default.
.PARAMETER ResourceGroup
    Required here or in the parameter file. Resource group name, for example
    team-history. Created if absent; otherwise reused in the selected subscription.
.PARAMETER Location
    Required here or in the parameter file. Azure region name, for example westeurope.
    Used for new resources and the managed identity; match existing resources' region.
.PARAMETER StorageAccountName
    Required here or in the parameter file. Globally unique account name using
    3-24 lowercase letters/digits, for example teamhistory2026. Created if absent in
    the selected resource group; existing account properties and data are preserved.
.PARAMETER GithubOrg
    Required here or in the parameter file. Existing GitHub owner name, for example
    example-org. Selects the repository's OIDC subjects; does not create a GitHub owner.
.PARAMETER GithubRepo
    Required here or in the parameter file. Existing repository name without owner,
    for example project. Selects OIDC trust; does not create or configure a repository.
.PARAMETER HistoryBranch
    Required here or in the parameter file. Literal branch name, for example main
    or release/next, without refs/heads/. Selects branch OIDC trust alongside PR trust;
    does not create the branch. Changing it reconfigures the existing branch credential.
.PARAMETER HistoryContainerName
    Optional container name; defaults to bench-history. Use 3-63 lowercase letters,
    digits or single internal hyphens, for example team-history. Created if absent;
    an existing container and its data are preserved.
.PARAMETER ManagedIdentityName
    Optional managed identity name, for example id-team-history. Defaults to
    id-<storage-account>-bench-history. Created or reused in the selected resource
    group; grants account-scoped blob contributor access and configures GitHub trust.
.PARAMETER CustomPrincipalId
    Optional object ID of an existing Entra user or group in the subscription's
    tenant, for example 00000000-0000-0000-0000-000000000002. Not an application/client
    ID. Supply CustomPrincipalType too. Adds account-scoped blob contributor access;
    does not create the principal. Cannot be combined with CurrentUser.
.PARAMETER CustomPrincipalType
    Required with CustomPrincipalId. User or Group, matching that Entra object;
    for example Group. Does not change the principal's type.
.PARAMETER CurrentUser
    Optional switch. Resolves the Azure CLI signed-in user in the selected
    subscription's tenant before any Azure changes and grants the same additional
    access. Requires a user login, Microsoft Graph access and the selected subscription
    to be active in Azure CLI. Service-principal logins are not supported.
    Conflicts with either custom principal parameter.
.EXAMPLE
    ./deploy.ps1 -ParametersFile ./parameters.json
.EXAMPLE
    ./deploy.ps1 -ParametersFile ./parameters.json -CurrentUser
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
    [string] $CustomPrincipalId,
    [string] $CustomPrincipalType,
    [switch] $CurrentUser
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = if ($PSBoundParameters.ContainsKey('Verbose')) { 'Continue' } else { 'SilentlyContinue' }

Import-Module (Join-Path $PSScriptRoot 'ProductionIdentityDeployment.psm1') -Force
$parameters = @{}
$known = @('SubscriptionId', 'ResourceGroup', 'Location', 'StorageAccountName',
    'GithubOrg', 'GithubRepo', 'HistoryBranch', 'HistoryContainerName',
    'ManagedIdentityName', 'CustomPrincipalId', 'CustomPrincipalType')
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

$outputs = Invoke-ProductionIdentityDeployment @parameters -CurrentUser:$CurrentUser
Write-Output 'Deployment complete. These identifiers are non-secret; no repository or GitHub settings were changed.'
Write-Output '[storage.azure]'
Write-Output "account = `"$($outputs.storageAccountName.value)`""
Write-Output "container = `"$($outputs.historyContainerName.value)`""
Write-Output "Blob endpoint: $($outputs.blobEndpoint.value)"
Write-Output "Azure tenant ID: $($outputs.tenantId.value)"
Write-Output "Azure subscription ID: $($outputs.subscriptionId.value)"
Write-Output "Managed identity client ID: $($outputs.managedIdentityClientId.value)"
Write-Output "Managed identity principal ID: $($outputs.managedIdentityPrincipalId.value)"
Write-Output 'Copy [storage.azure] into .cargo/bench_history.toml and commit it.'
Write-Output 'Create GitHub Actions repository variables AZURE_CLIENT_ID and AZURE_TENANT_ID with the managed identity client ID and tenant ID.'
Write-Output 'Set job environment variables AZURE_CLIENT_ID and AZURE_TENANT_ID from vars.AZURE_CLIENT_ID and vars.AZURE_TENANT_ID before running the root benchmark action or direct commands.'
Write-Output 'Grant that job id-token: write. The root composite action inherits the job environment, and the benchmark tool obtains its own OIDC token without an azure/login step.'
Write-Output 'The subscription ID selects management operations, or subscription-id in an optional azure/login step; direct benchmark OIDC does not need it.'
Write-Output 'The managed identity principal ID identifies RBAC assignments; do not substitute it for the client ID. The blob endpoint is for connectivity diagnostics, not an extra configuration field.'
Write-Output 'Keep the same-repository PR gate before credentialed work. The PR federated subject does not distinguish fork heads.'

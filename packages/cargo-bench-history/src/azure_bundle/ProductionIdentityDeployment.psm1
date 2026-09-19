#Requires -Version 7.6

# Canonical provisioning boundary for deploy.ps1, both exported and called from
# cargo-bench-history setup-azure. Maintainers need Azure CLI and installed Bicep,
# not Rust. Successful management-plane discovery selects additive bootstrap
# operations; Bicep owns all resource and federation definitions.
# Ref: README.md, "Deployment behavior".
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

# deploy.ps1 calls this policy boundary for both CLI-driven and exported deployments.
# It discovers missing storage before invoking Bicep and returns validated non-secret
# outputs for the driver's configuration handoff; Bicep remains the resource authority.
function Invoke-ProductionIdentityDeployment {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][ValidateNotNullOrWhiteSpace()][string] $SubscriptionId,
        [Parameter(Mandatory)][ValidateNotNullOrWhiteSpace()][string] $ResourceGroup,
        [Parameter(Mandatory)][ValidateNotNullOrWhiteSpace()][string] $Location,
        [Parameter(Mandatory)]
        [ValidatePattern('^[a-z0-9]{3,24}$', Options = 'None')]
        [string] $StorageAccountName,
        [Parameter(Mandatory)]
        [ValidatePattern('^[A-Za-z0-9-]+$', Options = 'None')]
        [string] $GithubOrg,
        [Parameter(Mandatory)]
        [ValidatePattern('^[A-Za-z0-9_.-]+$', Options = 'None')]
        [string] $GithubRepo,
        [Parameter(Mandatory)][ValidateNotNullOrWhiteSpace()][string] $HistoryBranch,
        [string] $ManagedIdentityName = '',
        [ValidatePattern('^(?!.*--)[a-z0-9][a-z0-9-]{1,61}[a-z0-9]$', Options = 'None')]
        [string] $HistoryContainerName = 'bench-history',
        [string] $CustomPrincipalId = '',
        [ValidateSet('', 'User', 'Group')][string] $CustomPrincipalType = '',
        [switch] $CurrentUser
    )

    if ([string]::IsNullOrEmpty($ManagedIdentityName)) {
        $ManagedIdentityName = "id-$StorageAccountName-bench-history"
    }
    if ($CurrentUser -and ($CustomPrincipalId -or $CustomPrincipalType)) {
        throw 'CurrentUser cannot be combined with CustomPrincipalId or CustomPrincipalType.'
    }
    if ([string]::IsNullOrEmpty($CustomPrincipalId) -ne [string]::IsNullOrEmpty($CustomPrincipalType)) {
        throw 'CustomPrincipalId and CustomPrincipalType must be supplied together.'
    }
    # Apply Git's literal branch-name rules without requiring Git in the exported bundle.
    # Component suffixes and HEAD are case-sensitive; @ is valid within refs/heads/@.
    # Ref: https://git-scm.com/docs/git-check-ref-format.
    if ($HistoryBranch -match '[\x00-\x20\x7f:~^?*\[\\]' -or
        $HistoryBranch -cmatch '(^|/)\.|\.lock(/|$)' -or $HistoryBranch -ceq 'HEAD' -or
        $HistoryBranch.Contains('..') -or
        $HistoryBranch.Contains('@{') -or $HistoryBranch.StartsWith('-') -or
        $HistoryBranch.StartsWith('refs/', [StringComparison]::Ordinal) -or
        $HistoryBranch.StartsWith('/') -or $HistoryBranch.EndsWith('/') -or
        $HistoryBranch.EndsWith('.') -or $HistoryBranch.Contains('//')) {
        throw 'HistoryBranch must be a branch name, not a ref or revision expression.'
    }

    $context = Get-AzureDeploymentContext -SubscriptionId $SubscriptionId -CurrentUser:$CurrentUser
    if ($CurrentUser) {
        $CustomPrincipalId = $context.CurrentUserPrincipalId
        $CustomPrincipalType = 'User'
    }

    Write-Verbose "Ensuring resource group '$ResourceGroup' in explicitly selected subscription '$SubscriptionId'; all tooling and authentication probes succeeded."
    Invoke-ProductionIdentityAz -Arguments @(
        'group', 'create', '--subscription', $SubscriptionId,
        '--name', $ResourceGroup, '--location', $Location, '--output', 'none'
    ) | Out-Null

    # Failed listing is never interpreted as absence: doing so could PUT existing
    # storage with fresh defaults. Only successful management-plane lists decide.
    $accounts = @(Invoke-ProductionIdentityAz -Arguments @(
            'storage', 'account', 'list', '--subscription', $SubscriptionId,
            '--resource-group', $ResourceGroup, '--output', 'json'
        ) | ConvertFrom-Json)
    $createStorageAccount = @($accounts | Where-Object name -EQ $StorageAccountName).Count -eq 0
    $createHistoryContainer = $true
    if (-not $createStorageAccount) {
        $containers = @(Invoke-ProductionIdentityAz -Arguments @(
                'storage', 'container-rm', 'list', '--subscription', $SubscriptionId,
                '--resource-group', $ResourceGroup, '--storage-account', $StorageAccountName,
                '--output', 'json'
            ) | ConvertFrom-Json)
        $createHistoryContainer = @($containers | Where-Object name -EQ $HistoryContainerName).Count -eq 0
    }

    Write-Verbose "Account '$StorageAccountName' needs bootstrap: $createStorageAccount; container '$HistoryContainerName' needs bootstrap: $createHistoryContainer. Existing storage is reference-only to preserve properties and history."
    Write-Verbose "Identity '$ManagedIdentityName' receives account-scoped Storage Blob Data Contributor and repository '$GithubOrg/$GithubRepo' branch '$HistoryBranch' plus PR federation. Optional custom principal access uses the same role independently."
    $outputs = Invoke-ProductionIdentityAz -Arguments @(
        'deployment', 'group', 'create', '--subscription', $SubscriptionId,
        '--resource-group', $ResourceGroup,
        '--name', "bench-history-$([guid]::NewGuid().ToString('N'))",
        '--mode', 'Incremental', '--template-file', (Join-Path $PSScriptRoot 'main.bicep'),
        '--parameters',
        "storageAccountName=$StorageAccountName",
        "location=$Location",
        "managedIdentityName=$ManagedIdentityName",
        "historyContainerName=$HistoryContainerName",
        "createStorageAccount=$($createStorageAccount.ToString().ToLowerInvariant())",
        "createHistoryContainer=$($createHistoryContainer.ToString().ToLowerInvariant())",
        "githubOrg=$GithubOrg",
        "githubRepo=$GithubRepo",
        "historyBranch=$HistoryBranch",
        "customPrincipalId=$CustomPrincipalId",
        # An empty ID suppresses the additional role; the placeholder type satisfies Bicep's allowed values.
        "customPrincipalType=$(if ($CustomPrincipalType) { $CustomPrincipalType } else { 'User' })",
        '--query', 'properties.outputs', '--output', 'json'
    ) | ConvertFrom-Json

    foreach ($name in @('storageAccountName', 'historyContainerName', 'blobEndpoint',
            'managedIdentityClientId', 'managedIdentityPrincipalId', 'tenantId', 'subscriptionId')) {
        if ([string]::IsNullOrWhiteSpace($outputs.$name.value)) {
            throw "Deployment did not return required output '$name'. Azure changes are not rolled back."
        }
    }
    return $outputs
}

# Production provisioning and the throwaway test driver call this before any Azure
# mutation. It verifies tooling and the target login, returning a resolved user ID
# only when requested. Sharing preflight does not share storage or federation policy.
function Get-AzureDeploymentContext {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][ValidateNotNullOrWhiteSpace()][string] $SubscriptionId,
        [switch] $CurrentUser
    )

    # `bicep version` fails when Bicep is absent instead of implicitly installing it.
    Invoke-ProductionIdentityAz -Arguments @('version', '--output', 'json') | Out-Null
    Invoke-ProductionIdentityAz -Arguments @('bicep', 'version') | Out-Null
    $account = Invoke-ProductionIdentityAz -Arguments @(
        'account', 'show', '--subscription', $SubscriptionId, '--output', 'json'
    ) | ConvertFrom-Json
    if ($account.id -ne $SubscriptionId -or $account.state -ne 'Enabled' -or
        [string]::IsNullOrWhiteSpace($account.tenantId)) {
        throw "An authenticated, enabled context for subscription '$SubscriptionId' is required."
    }
    # A cached subscription entry alone does not demonstrate a usable credential.
    # Suppress the token completely: none of the command's outputs contain secrets.
    Invoke-ProductionIdentityAz -Arguments @(
        'account', 'get-access-token', '--subscription', $SubscriptionId,
        '--query', 'expires_on', '--output', 'tsv'
    ) | Out-Null

    $principalId = ''
    if ($CurrentUser) {
        if (-not $account.PSObject.Properties['user'] -or $account.user.type -ne 'user') {
            throw "CurrentUser requires Azure CLI to be signed in as a user for subscription '$SubscriptionId'. Sign in with az login --tenant $($account.tenantId), or supply an explicit custom principal."
        }
        # Directory lookup uses the active CLI context; its command has no
        # subscription selector. Check it instead of changing the user's default
        # or handling a raw access token. This also selects the correct guest object.
        # Ref: README.md, "Deploy".
        $activeAccount = Invoke-ProductionIdentityAz -Arguments @(
            'account', 'show', '--output', 'json'
        ) | ConvertFrom-Json
        if ($activeAccount.id -ne $SubscriptionId -or $activeAccount.tenantId -ne $account.tenantId) {
            throw "CurrentUser requires the selected subscription to be active for directory lookup. Run az account set --subscription $SubscriptionId, then retry. No Azure resources were changed."
        }
        Write-Verbose "Resolving the signed-in user in tenant '$($account.tenantId)' through subscription '$SubscriptionId' before any Azure changes."
        $principalId = Invoke-ProductionIdentityAz -Arguments @(
            'ad', 'signed-in-user', 'show', '--query', 'id', '--output', 'tsv'
        )
        $principalGuid = [guid]::Empty
        if (-not [guid]::TryParseExact($principalId, 'D', [ref] $principalGuid) -or
            $principalGuid -eq [guid]::Empty) {
            throw "Could not resolve the signed-in user's Entra object ID in tenant '$($account.tenantId)'. No Azure resources were changed."
        }
    }
    return [pscustomobject]@{ CurrentUserPrincipalId = $principalId }
}

# Shared native-call boundary for provisioning and preflight. Keeping Azure stdout
# separate from stderr lets callers parse JSON/IDs while retaining operation and
# exit diagnostics on failure, without interpreting a failed probe as absence.
function Invoke-ProductionIdentityAz {
    [CmdletBinding()]
    param([Parameter(Mandatory)][string[]] $Arguments)

    # Keep native stderr separate from JSON stdout and retain both operation and
    # exit context. No errors authorize bootstrap or imply rollback.
    try {
        # Explicit exit checking retains stdout even when a failed native command
        # emits diagnostics there; automatic native throwing can discard it.
        $PSNativeCommandUseErrorActionPreference = $false
        $output = az @Arguments
        if ($LASTEXITCODE -ne 0) {
            throw "exit code $LASTEXITCODE. stdout: $($output -join [Environment]::NewLine)"
        }
        return $output
    }
    catch {
        throw "az $($Arguments -join ' ') failed: $_. Any completed Azure changes remain in place."
    }
}

Export-ModuleMember -Function Invoke-ProductionIdentityDeployment, Get-AzureDeploymentContext

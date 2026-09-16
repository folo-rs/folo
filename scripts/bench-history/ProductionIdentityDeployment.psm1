#Requires -Version 7.6

# Azure provisioning boundary for infra/azure-bench-history-prod/deploy.ps1.
# Maintainers need only Azure CLI, installed Bicep and PowerShell, not a prepared
# Rust toolchain. Bicep owns resource definitions; this module observes existing
# storage and writer trust, selects additive deployment parameters, and performs
# the explicitly requested single-credential retirement after deployment succeeds.
# Ref: infra/azure-bench-history-prod/README.md, "Deployment behavior".

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

function Invoke-ProductionIdentityDeployment {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $SubscriptionId,
        [string] $ResourceGroup = 'folohistory',
        [string] $Location = 'swedencentral',
        [ValidatePattern('^[a-z0-9]{3,24}$', Options = 'None')]
        [string] $StorageAccountName = 'folohistory',
        [string] $ManagedIdentityName = 'id-folo-bench-history-prod',
        [string] $ReaderManagedIdentityName = '',
        [ValidatePattern('^(?!.*--)[a-z0-9][a-z0-9-]{1,61}[a-z0-9]$', Options = 'None')]
        [string] $HistoryContainerName = 'bench-history',
        [string] $GithubOrg = 'folo-rs',
        [string] $GithubRepo = 'folo',
        [string] $LocalPrincipalId = '',
        [ValidateSet('User', 'Group')]
        [string] $LocalPrincipalType = 'User',
        [switch] $RetireWriterPullRequestTrust
    )

    if ([string]::IsNullOrEmpty($ReaderManagedIdentityName)) {
        $ReaderManagedIdentityName = "$ManagedIdentityName-reader"
    }
    if ($ReaderManagedIdentityName -eq $ManagedIdentityName) {
        throw 'The reader and writer must use different managed identities.'
    }

    # `version` fails when Bicep is absent instead of installing it implicitly as
    # deployment commands can. Check before making any Azure changes.
    Invoke-ProductionIdentityAz -Arguments @('bicep', 'version') | Out-Null

    Write-Verbose "Ensuring resource group '$ResourceGroup' in subscription '$SubscriptionId' exists before querying its resources."
    Invoke-ProductionIdentityAz -Arguments @(
        'group', 'create', '--subscription', $SubscriptionId,
        '--name', $ResourceGroup, '--location', $Location, '--output', 'none'
    ) | Out-Null

    # Successful list responses distinguish absence from authorization/CLI errors.
    # A failed show must never be interpreted as permission to bootstrap storage.
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

    $identities = @(Invoke-ProductionIdentityAz -Arguments @(
            'identity', 'list', '--subscription', $SubscriptionId,
            '--resource-group', $ResourceGroup, '--output', 'json'
        ) | ConvertFrom-Json)
    $writerExists = @($identities | Where-Object name -EQ $ManagedIdentityName).Count -ne 0
    # New writers need only branch trust; the reader serves PR analysis. Existing
    # PR trust is preserved until explicit retirement, and absence stays absent.
    $trustPullRequests = $false
    if ($writerExists) {
        $trustPullRequests = Test-ProductionWriterPullRequestTrust `
            -SubscriptionId $SubscriptionId -ResourceGroup $ResourceGroup `
            -ManagedIdentityName $ManagedIdentityName
    }
    if ($RetireWriterPullRequestTrust) {
        $trustPullRequests = $false
    }

    Write-Verbose "Storage account '$StorageAccountName' requires creation: $createStorageAccount; container '$HistoryContainerName' requires creation: $createHistoryContainer. Existing storage resources are reference-only to preserve their properties and data."
    Write-Verbose "Writer '$ManagedIdentityName' exists: $writerExists; retirement requested: $RetireWriterPullRequestTrust; provision writer PR trust: $trustPullRequests. Existing credential absence is preserved; branch trust remains provisioned."
    Write-Verbose "Reader '$ReaderManagedIdentityName' receives container-scoped read access. Local principal '$LocalPrincipalId' (type '$LocalPrincipalType') receives account contributor access only when supplied."

    $templateFile = Join-Path $PSScriptRoot '..' '..' 'infra' 'azure-bench-history-prod' 'main.bicep'
    $outputs = Invoke-ProductionIdentityAz -Arguments @(
        'deployment', 'group', 'create', '--subscription', $SubscriptionId,
        '--resource-group', $ResourceGroup,
        '--name', "bench-history-prod-$([guid]::NewGuid().ToString('N'))",
        '--mode', 'Incremental', '--template-file', $templateFile,
        '--parameters',
        "storageAccountName=$StorageAccountName",
        "location=$Location",
        "managedIdentityName=$ManagedIdentityName",
        "readerManagedIdentityName=$ReaderManagedIdentityName",
        "historyContainerName=$HistoryContainerName",
        "createStorageAccount=$($createStorageAccount.ToString().ToLowerInvariant())",
        "createHistoryContainer=$($createHistoryContainer.ToString().ToLowerInvariant())",
        "trustPullRequests=$($trustPullRequests.ToString().ToLowerInvariant())",
        "githubOrg=$GithubOrg",
        "githubRepo=$GithubRepo",
        "localPrincipalId=$LocalPrincipalId",
        "localPrincipalType=$LocalPrincipalType",
        '--query', 'properties.outputs', '--output', 'json'
    ) | ConvertFrom-Json

    if ($RetireWriterPullRequestTrust) {
        # Incremental ARM omission does not delete an existing credential. Re-read
        # after successful reader provisioning so retries are safe and a deployment
        # failure cannot retire legacy access. Never delete an identity or role.
        $stillTrusted = Test-ProductionWriterPullRequestTrust `
            -SubscriptionId $SubscriptionId -ResourceGroup $ResourceGroup `
            -ManagedIdentityName $ManagedIdentityName
        if ($stillTrusted) {
            Write-Verbose "Explicit retirement requested and reader deployment succeeded; deleting only '$ManagedIdentityName/github-pull-request'. Branch credentials and writer roles remain unchanged."
            Invoke-ProductionIdentityAz -Arguments @(
                'identity', 'federated-credential', 'delete', '--subscription', $SubscriptionId,
                '--resource-group', $ResourceGroup, '--identity-name', $ManagedIdentityName,
                '--name', 'github-pull-request', '--yes', '--output', 'none'
            ) | Out-Null
        }
        else {
            Write-Verbose "Writer '$ManagedIdentityName' has no github-pull-request credential; retirement is already satisfied, so no delete is needed."
        }
    }

    return $outputs
}

function Test-ProductionWriterPullRequestTrust {
    [CmdletBinding()]
    [OutputType([bool])]
    param(
        [Parameter(Mandatory)][string] $SubscriptionId,
        [Parameter(Mandatory)][string] $ResourceGroup,
        [Parameter(Mandatory)][string] $ManagedIdentityName
    )

    $credentials = @(Invoke-ProductionIdentityAz -Arguments @(
            'identity', 'federated-credential', 'list', '--subscription', $SubscriptionId,
            '--resource-group', $ResourceGroup, '--identity-name', $ManagedIdentityName,
            '--output', 'json'
        ) | ConvertFrom-Json)
    return @($credentials | Where-Object name -EQ 'github-pull-request').Count -ne 0
}

function Invoke-ProductionIdentityAz {
    [CmdletBinding()]
    param([Parameter(Mandatory)][string[]] $Arguments)

    # Preserve stderr on the console, separate from JSON stdout. The explicit exit
    # check also supports mocked az commands; real native failures already throw.
    $output = az @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "az $($Arguments -join ' ') failed with exit code $LASTEXITCODE."
    }
    return $output
}

Export-ModuleMember -Function Invoke-ProductionIdentityDeployment

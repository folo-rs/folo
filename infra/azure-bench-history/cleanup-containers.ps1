#requires -Version 7

<#
.SYNOPSIS
    Deletes leftover test containers from the cargo-bench-history storage account.

.DESCRIPTION
    Each real-Azure test creates a uniquely-named container (prefix `bh-it-`) and
    deletes it when it finishes. A crashed or timed-out test can leave one behind;
    this script sweeps them up. It is used both locally (manual cleanup) and as the
    CI `test-azure` job's `if: always()` backstop.

    Authenticates with Microsoft Entra ID (`--auth-mode login`), matching the
    Entra-only storage account, so it works with the same `az login` / federated
    session the tests use.

.PARAMETER AccountName
    Storage account name. Defaults to the BENCH_HISTORY_AZURE_ACCOUNT env var.

.PARAMETER Prefix
    Container name prefix to match. Defaults to 'bh-it-'.

.PARAMETER MinAgeMinutes
    Only delete containers last modified at least this many minutes ago. Defaults
    to 0 (delete all matching). Raise it locally to avoid disturbing a container an
    in-progress run is still using.

.EXAMPLE
    ./cleanup-containers.ps1 -AccountName stfolobenchhist
#>
[CmdletBinding()]
param(
    [string] $AccountName = $env:BENCH_HISTORY_AZURE_ACCOUNT,

    [string] $Prefix = 'bh-it-',

    [int] $MinAgeMinutes = 0
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'

if ([string]::IsNullOrEmpty($AccountName)) {
    Write-Error 'No storage account specified. Pass -AccountName or set BENCH_HISTORY_AZURE_ACCOUNT.'
    exit 1
}

Write-Verbose "Listing containers in '$AccountName' with prefix '$Prefix'."
$listJson = az storage container list `
    --account-name $AccountName `
    --auth-mode login `
    --prefix $Prefix `
    --query '[].{name:name, lastModified:properties.lastModified}' `
    --output json
$containers = $listJson | ConvertFrom-Json

if (-not $containers -or $containers.Count -eq 0) {
    Write-Host "No containers with prefix '$Prefix' found in '$AccountName'." -ForegroundColor Green
    return
}

$cutoff = [DateTimeOffset]::UtcNow.AddMinutes(-$MinAgeMinutes)
$deleted = 0
$skipped = 0

foreach ($container in $containers) {
    $lastModified = [DateTimeOffset]::Parse($container.lastModified)
    if ($lastModified -gt $cutoff) {
        Write-Verbose "Skipping '$($container.name)' (modified $lastModified, newer than the $MinAgeMinutes-minute cutoff)."
        $skipped++
        continue
    }

    Write-Verbose "Deleting container '$($container.name)' (last modified $lastModified)."
    az storage container delete `
        --account-name $AccountName `
        --auth-mode login `
        --name $container.name `
        --output none
    $deleted++
}

$deletedNoun = if ($deleted -eq 1) { 'container' } else { 'containers' }
Write-Host "Deleted $deleted $deletedNoun; skipped $skipped newer than the cutoff." -ForegroundColor Green

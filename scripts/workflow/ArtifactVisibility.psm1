#Requires -Version 7.6

# Owns the bounded visibility check required between an artifact upload and a later download from
# the same workflow run. GitHub can finalize an upload before its run-artifact listing exposes the
# result, while the download action itself remains a single explicit operation.
# Ref: .github/workflows/implementation.md#same-run-artifact-visibility.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

Import-Module (Join-Path $PSScriptRoot '..\utility\Retry.psm1') -Force

function Get-GitHubRunArtifact {
    # Reads the paginated artifact inventory for one workflow run. Query failures remain visible to
    # Wait-GitHubRunArtifact, which retries only recognized transient infrastructure failures.
    [CmdletBinding()]
    [OutputType([pscustomobject])]
    param(
        [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string] $Repository,
        [Parameter(Mandatory)][ValidateRange(1, [long]::MaxValue)][long] $RunId
    )

    $previousNativeErrorPreference = $PSNativeCommandUseErrorActionPreference
    try {
        $PSNativeCommandUseErrorActionPreference = $false
        $output = @(gh api --paginate --slurp `
                "repos/$Repository/actions/runs/$RunId/artifacts?per_page=100" 2>&1)
        $exitCode = $LASTEXITCODE
    } finally {
        $PSNativeCommandUseErrorActionPreference = $previousNativeErrorPreference
    }

    if ($exitCode -ne 0) {
        $diagnostics = $output -join "`n"
        throw "GitHub artifact inventory request failed (exit $exitCode): $diagnostics"
    }

    $json = $output -join "`n"
    $pages = @($json | ConvertFrom-Json)
    return @($pages | ForEach-Object { $_.artifacts })
}

function Get-GitHubRunAttemptJob {
    # Reads the job inventory for one attempt so a pattern consumer can distinguish producers
    # rerun in this attempt from successful producers inherited from an earlier attempt.
    [CmdletBinding()]
    [OutputType([pscustomobject])]
    param(
        [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string] $Repository,
        [Parameter(Mandatory)][ValidateRange(1, [long]::MaxValue)][long] $RunId,
        [Parameter(Mandatory)][ValidateRange(1, [int]::MaxValue)][int] $RunAttempt
    )

    $previousNativeErrorPreference = $PSNativeCommandUseErrorActionPreference
    try {
        $PSNativeCommandUseErrorActionPreference = $false
        $output = @(gh api --paginate --slurp `
                "repos/$Repository/actions/runs/$RunId/attempts/$RunAttempt/jobs?per_page=100" 2>&1)
        $exitCode = $LASTEXITCODE
    } finally {
        $PSNativeCommandUseErrorActionPreference = $previousNativeErrorPreference
    }

    if ($exitCode -ne 0) {
        $diagnostics = $output -join "`n"
        throw "GitHub workflow-attempt job request failed (exit $exitCode): $diagnostics"
    }

    $json = $output -join "`n"
    $pages = @($json | ConvertFrom-Json)
    return @($pages | ForEach-Object { $_.jobs })
}

function Wait-GitHubRunArtifact {
    # Waits until a same-run artifact selected by ID, exact name or pattern is visible through the
    # run inventory. The bounded budget only covers eventual upload visibility and transient read
    # failures; deterministic API failures propagate immediately and the final miss fails explicitly.
    [CmdletBinding(DefaultParameterSetName = 'ById')]
    param(
        [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string] $Repository,
        [Parameter(Mandatory)][ValidateRange(1, [long]::MaxValue)][long] $RunId,
        [Parameter(Mandatory, ParameterSetName = 'ById')]
        [ValidateRange(1, [long]::MaxValue)]
        [long] $ArtifactId,
        [Parameter(Mandatory, ParameterSetName = 'ByName')]
        [ValidateNotNullOrEmpty()]
        [string] $Name,
        [Parameter(Mandatory, ParameterSetName = 'ByPattern')]
        [ValidateNotNullOrEmpty()]
        [string] $Pattern,
        [Parameter(ParameterSetName = 'ByPattern')]
        [ValidateRange(1, [int]::MaxValue)]
        [int] $ExpectedCount = 1,
        [Parameter(ParameterSetName = 'ByPattern')]
        [hashtable] $ProducerJob,
        [Parameter(ParameterSetName = 'ByPattern')]
        [ValidateRange(1, [int]::MaxValue)]
        [int] $RunAttempt,
        # The default budget accommodates the observed propagation window while keeping CI bounded.
        [ValidateRange(1, [int]::MaxValue)][int] $Attempt = 12,
        # The fixed delay avoids a long inactive run while artifact-listing consistency catches up.
        [ValidateRange(0, [int]::MaxValue)][int] $DelaySeconds = 5
    )

    $selector = switch ($PSCmdlet.ParameterSetName) {
        'ById' { "artifact ID $ArtifactId" }
        'ByName' { "artifact name '$Name'" }
        'ByPattern' { "artifact pattern '$Pattern' ($ExpectedCount required)" }
        default { throw "Unsupported artifact selector '$($PSCmdlet.ParameterSetName)'." }
    }
    $queryRepository = $Repository
    if ($ProducerJob -and -not $RunAttempt) {
        throw 'RunAttempt is required when ProducerJob freshness is configured.'
    }

    try {
        Invoke-WithRetry -Attempt $Attempt -DelaySeconds $DelaySeconds -RetryOn {
            param($errorRecord)

            return $errorRecord.FullyQualifiedErrorId -ceq 'ArtifactVisibilityPending' -or
                (Test-TransientFailure -Message $errorRecord.Exception.Message)
        } -Action {
            $artifacts = @(Get-GitHubRunArtifact -Repository $queryRepository -RunId $RunId)
            $matchingArtifacts = @(switch ($PSCmdlet.ParameterSetName) {
                'ById' { @($artifacts | Where-Object { $_.id -eq $ArtifactId }) }
                'ByName' { @($artifacts | Where-Object { $_.name -ceq $Name }) }
                'ByPattern' { @($artifacts | Where-Object { $_.name -clike $Pattern }) }
            })

            $matchingCount = if ($PSCmdlet.ParameterSetName -ceq 'ByPattern') {
                @(
                    $matchingArtifacts |
                        ForEach-Object { $_.name } |
                        Sort-Object -CaseSensitive -Unique
                ).Count
            } else {
                $matchingArtifacts.Count
            }

            $staleArtifacts = @()
            if ($ProducerJob -and $matchingCount -ge $ExpectedCount) {
                $attemptJobs = @(Get-GitHubRunAttemptJob -Repository $queryRepository `
                        -RunId $RunId -RunAttempt $RunAttempt)
                foreach ($artifactName in $ProducerJob.Keys) {
                    $currentJobs = @($attemptJobs | Where-Object {
                            $_.run_attempt -eq $RunAttempt -and
                                $_.name -ceq $ProducerJob[$artifactName]
                        })
                    if ($currentJobs.Count -eq 0) {
                        continue
                    }
                    if ($currentJobs.Count -ne 1) {
                        throw "Expected one current-attempt job named '$($ProducerJob[$artifactName])', " +
                            "but found $($currentJobs.Count)."
                    }

                    $latestArtifact = @(
                        $matchingArtifacts |
                            Where-Object { $_.name -ceq $artifactName } |
                            Sort-Object -Property id -Descending
                    ) | Select-Object -First 1
                    if ($null -eq $latestArtifact) {
                        $staleArtifacts += $artifactName
                        continue
                    }

                    $createdAt = [DateTimeOffset]::Parse(
                        [string] $latestArtifact.created_at,
                        [Globalization.CultureInfo]::InvariantCulture
                    )
                    $startedAt = [DateTimeOffset]::Parse(
                        [string] $currentJobs[0].started_at,
                        [Globalization.CultureInfo]::InvariantCulture
                    )
                    if ($createdAt -lt $startedAt) {
                        $staleArtifacts += $artifactName
                    }
                }
            }

            if ($matchingCount -ge $ExpectedCount -and $staleArtifacts.Count -eq 0) {
                Write-Host "Artifact inventory contains $selector."
                return
            }

            $freshnessDetail = if ($staleArtifacts.Count -gt 0) {
                "; stale for current producers: $($staleArtifacts -join ', ')"
            } else {
                ''
            }
            $detail = "$selector is not yet visible in workflow run $RunId " +
                "(observed $matchingCount$freshnessDetail)."
            $errorRecord = [System.Management.Automation.ErrorRecord]::new(
                [System.InvalidOperationException]::new($detail),
                'ArtifactVisibilityPending',
                [System.Management.Automation.ErrorCategory]::ResourceUnavailable,
                $null
            )
            throw $errorRecord
        }
    } catch {
        if ($_.FullyQualifiedErrorId -ceq 'ArtifactVisibilityPending') {
            throw [System.InvalidOperationException]::new(
                "$selector did not become visible after $Attempt attempts in workflow run $RunId."
            )
        }

        throw
    }
}

Export-ModuleMember -Function Wait-GitHubRunArtifact

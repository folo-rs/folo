#Requires -Version 7.6

# Owns the bounded same-run artifact handoff. Metadata selection and archive download are separate
# service observations, so one retry operation covers both instead of leaving the download action
# as an unguarded one-shot request after an upload becomes visible.
# Ref: .github/workflows/implementation.md#same-run-artifact-visibility.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

Import-Module (Join-Path $PSScriptRoot '..\utility\Retry.psm1') -Force

function Get-GitHubArtifactHttpStatusCode {
    # Reads only explicit HTTP status data; transport and parsing failures keep their own type.
    [CmdletBinding()]
    [OutputType([int])]
    param(
        [Parameter(Mandatory)] [System.Management.Automation.ErrorRecord] $ErrorRecord
    )

    $responseProperty = $ErrorRecord.Exception.PSObject.Properties['Response']
    if ($null -eq $responseProperty -or $null -eq $responseProperty.Value) { return $null }
    $statusProperty = $responseProperty.Value.PSObject.Properties['StatusCode']
    if ($null -eq $statusProperty) { return $null }
    return [int] $statusProperty.Value
}

function Get-GitHubArtifactResponseProperty {
    # Preserves empty collection properties instead of pipeline-enumerating [] into $null.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)] $Response,
        [Parameter(Mandatory)] [string] $Name
    )

    $found = $false
    $property = $null
    if ($Response -is [System.Collections.IDictionary]) {
        if ($Response.Contains($Name)) {
            $found = $true
            $property = $Response[$Name]
        }
    }
    else {
        $member = $Response.PSObject.Properties[$Name]
        if ($null -ne $member) {
            $found = $true
            $property = $member.Value
        }
    }

    if (-not $found -or $null -eq $property) {
        throw "GitHub artifact response omitted required property '$Name'."
    }
    return $property
}

function Get-GitHubArtifactPendingError {
    # Missing metadata and archive-not-found responses are deterministic retry evidence, not API
    # defects. The shared error ID keeps the retry classifier narrower than status-text matching.
    [CmdletBinding()]
    [OutputType([System.Management.Automation.ErrorRecord])]
    param(
        [Parameter(Mandatory)] [string] $Message
    )

    return [System.Management.Automation.ErrorRecord]::new(
        [System.InvalidOperationException]::new($Message),
        'ArtifactVisibilityPending',
        [System.Management.Automation.ErrorCategory]::ResourceUnavailable,
        $null
    )
}

function Get-GitHubArtifactTransientError {
    # Marks only explicitly transient HTTP classes so permanent 4xx responses propagate unchanged.
    [CmdletBinding()]
    [OutputType([System.Management.Automation.ErrorRecord])]
    param(
        [Parameter(Mandatory)] [string] $Message,
        [Parameter(Mandatory)] [System.Exception] $InnerException
    )

    return [System.Management.Automation.ErrorRecord]::new(
        [System.InvalidOperationException]::new($Message, $InnerException),
        'ArtifactRequestTransient',
        [System.Management.Automation.ErrorCategory]::ConnectionError,
        $null
    )
}

function Invoke-GitHubArtifactRestMethod {
    # Applies a hard per-request timeout and classifies only bounded retry categories. An
    # unrecognized response remains a permanent failure instead of consuming the retry budget.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)] [uri] $Uri,
        [Parameter(Mandatory)] [hashtable] $Headers,
        [Parameter(Mandatory)] [ValidateRange(1, 300)] [int] $TimeoutSeconds
    )

    try {
        return Invoke-RestMethod -Uri $Uri -Headers $Headers -TimeoutSec $TimeoutSeconds -ErrorAction Stop
    }
    catch {
        $status = Get-GitHubArtifactHttpStatusCode -ErrorRecord $_
        if ($null -ne $status -and ($status -eq 408 -or $status -eq 429 -or $status -ge 500)) {
            throw (Get-GitHubArtifactTransientError `
                    -Message "GitHub artifact request failed with HTTP $status." `
                    -InnerException $_.Exception)
        }
        throw
    }
}

function Get-GitHubRunArtifact {
    # Reads the complete paginated run inventory because pattern selection and download both use
    # this same authenticated REST boundary. Empty pages remain ordinary eventual absence.
    [CmdletBinding()]
    [OutputType([pscustomobject])]
    param(
        [Parameter(Mandatory)] [uri] $ApiUrl,
        [Parameter(Mandatory)] [string] $Repository,
        [Parameter(Mandatory)] [long] $RunId,
        [Parameter(Mandatory)] [hashtable] $Headers,
        [Parameter(Mandatory)] [int] $TimeoutSeconds
    )

    $artifacts = @()
    $expectedCount = $null
    $page = 1
    $base = $ApiUrl.AbsoluteUri.TrimEnd('/')
    while ($true) {
        $uri = "$base/repos/$Repository/actions/runs/$RunId/artifacts?per_page=100&page=$page"
        $response = Invoke-GitHubArtifactRestMethod -Uri $uri -Headers $Headers `
            -TimeoutSeconds $TimeoutSeconds
        if ($null -eq $expectedCount) {
            $expectedCount = [int] (Get-GitHubArtifactResponseProperty `
                    -Response $response -Name 'total_count')
        }
        $batch = @(Get-GitHubArtifactResponseProperty -Response $response -Name 'artifacts')
        $artifacts += $batch
        if ($artifacts.Count -ge $expectedCount) { return @($artifacts) }
        if ($batch.Count -eq 0) {
            throw "GitHub returned an incomplete artifact inventory for workflow run $RunId."
        }
        $page++
    }
}

function Get-GitHubRunAttemptJob {
    # Reads the current attempt's jobs to identify producers that actually need a fresh artifact.
    # Reused successful jobs retain their original start time and therefore accept inherited data.
    [CmdletBinding()]
    [OutputType([pscustomobject])]
    param(
        [Parameter(Mandatory)] [uri] $ApiUrl,
        [Parameter(Mandatory)] [string] $Repository,
        [Parameter(Mandatory)] [long] $RunId,
        [Parameter(Mandatory)] [int] $RunAttempt,
        [Parameter(Mandatory)] [hashtable] $Headers,
        [Parameter(Mandatory)] [int] $TimeoutSeconds
    )

    $jobs = @()
    $expectedCount = $null
    $page = 1
    $base = $ApiUrl.AbsoluteUri.TrimEnd('/')
    while ($true) {
        $uri = "$base/repos/$Repository/actions/runs/$RunId/attempts/$RunAttempt/jobs" +
            "?per_page=100&page=$page"
        $response = Invoke-GitHubArtifactRestMethod -Uri $uri -Headers $Headers `
            -TimeoutSeconds $TimeoutSeconds
        if ($null -eq $expectedCount) {
            $expectedCount = [int] (Get-GitHubArtifactResponseProperty `
                    -Response $response -Name 'total_count')
        }
        $batch = @(Get-GitHubArtifactResponseProperty -Response $response -Name 'jobs')
        $jobs += $batch
        if ($jobs.Count -ge $expectedCount) { return @($jobs) }
        if ($batch.Count -eq 0) {
            throw "GitHub returned an incomplete job inventory for workflow attempt $RunAttempt."
        }
        $page++
    }
}

function Get-GitHubArtifactJobMarker {
    # Reusable workflows can prefix local job names; selection compares the final local component.
    [CmdletBinding()]
    [OutputType([string])]
    param(
        [Parameter(Mandatory)] $Job
    )

    $name = [string] (Get-GitHubArtifactResponseProperty -Response $Job -Name 'name')
    $separator = $name.LastIndexOf(' / ', [StringComparison]::Ordinal)
    if ($separator -lt 0) { return $name }
    return $name.Substring($separator + 3)
}

function Select-GitHubRunArtifact {
    # Resolves latest-per-name selection to match download-artifact behavior while keeping one ID
    # exact. Pattern consumers with producer maps select only expected current matrix identities.
    [CmdletBinding(DefaultParameterSetName = 'ById')]
    [OutputType([pscustomobject])]
    param(
        [Parameter(Mandatory)] [AllowEmptyCollection()] [object[]] $Artifact,
        [Parameter(Mandatory, ParameterSetName = 'ById')] [long] $ArtifactId,
        [Parameter(Mandatory, ParameterSetName = 'ByName')] [string] $Name,
        [Parameter(Mandatory, ParameterSetName = 'ByPattern')] [string] $Pattern,
        [Parameter(ParameterSetName = 'ByPattern')] [hashtable] $ProducerJob
    )

    $matching = @(switch ($PSCmdlet.ParameterSetName) {
            'ById' { $Artifact | Where-Object { [long] $_.id -eq $ArtifactId } }
            'ByName' { $Artifact | Where-Object { [string] $_.name -ceq $Name } }
            'ByPattern' { $Artifact | Where-Object { [string] $_.name -clike $Pattern } }
        })

    if ($PSCmdlet.ParameterSetName -ceq 'ById') { return @($matching) }

    $latestByName = @(
        $matching |
            Group-Object -Property name -CaseSensitive |
            ForEach-Object { @($_.Group | Sort-Object -Property id -Descending)[0] }
    )
    if ($ProducerJob) {
        foreach ($name in $ProducerJob.Keys) {
            if ($name -cnotlike $Pattern) {
                throw "Producer artifact '$name' does not match pattern '$Pattern'."
            }
        }
        return @($latestByName | Where-Object { $ProducerJob.ContainsKey([string] $_.name) })
    }
    return $latestByName
}

function Assert-GitHubRunArtifactSelection {
    # Proves that selected metadata belongs to this run and remains usable before archive download.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)] [object[]] $Artifact,
        [Parameter(Mandatory)] [long] $RunId
    )

    foreach ($item in $Artifact) {
        $name = [string] (Get-GitHubArtifactResponseProperty -Response $item -Name 'name')
        $workflowRun = Get-GitHubArtifactResponseProperty -Response $item -Name 'workflow_run'
        $artifactRun = [long] (Get-GitHubArtifactResponseProperty `
                -Response $workflowRun -Name 'id')
        if ($artifactRun -ne $RunId) {
            throw "Artifact '$name' belongs to workflow run $artifactRun, not $RunId."
        }
        if ([bool] (Get-GitHubArtifactResponseProperty -Response $item -Name 'expired')) {
            throw "Artifact '$name' has expired."
        }
        $null = Get-GitHubArtifactResponseProperty -Response $item -Name 'archive_download_url'
    }
}

function Get-GitHubArtifactFreshness {
    # Identifies expected names whose latest archive predates a producer that actually reran.
    [CmdletBinding()]
    [OutputType([string])]
    param(
        [Parameter(Mandatory)] [AllowEmptyCollection()] [object[]] $Artifact,
        [Parameter(Mandatory)] [AllowEmptyCollection()] [object[]] $Job,
        [Parameter(Mandatory)] [hashtable] $ProducerJob,
        [Parameter(Mandatory)] [int] $RunAttempt
    )

    foreach ($artifactName in $ProducerJob.Keys) {
        $latestArtifact = @($Artifact | Where-Object { [string] $_.name -ceq $artifactName }) |
            Select-Object -First 1
        if ($null -eq $latestArtifact) {
            $artifactName
            continue
        }

        $currentJobs = @($Job | Where-Object {
                [int] $_.run_attempt -eq $RunAttempt -and
                    (Get-GitHubArtifactJobMarker -Job $_) -ceq $ProducerJob[$artifactName]
            })
        if ($currentJobs.Count -eq 0) { continue }
        if ($currentJobs.Count -ne 1) {
            throw "Expected one current-attempt job named '$($ProducerJob[$artifactName])', " +
                "but found $($currentJobs.Count)."
        }

        $createdAt = [DateTimeOffset]::Parse(
            [string] $latestArtifact.created_at,
            [Globalization.CultureInfo]::InvariantCulture,
            [Globalization.DateTimeStyles]::RoundtripKind
        )
        $startedAt = [DateTimeOffset]::Parse(
            [string] $currentJobs[0].started_at,
            [Globalization.CultureInfo]::InvariantCulture,
            [Globalization.DateTimeStyles]::RoundtripKind
        )
        if ($createdAt -lt $startedAt) { $artifactName }
    }
}

function Save-GitHubArtifactArchive {
    # Downloads one archive through its public URL. Only a 404 is artifact visibility evidence;
    # authorization, expiration, service and transport failures retain their distinct outcomes.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)] $Artifact,
        [Parameter(Mandatory)] [string] $Path,
        [Parameter(Mandatory)] [hashtable] $Headers,
        [Parameter(Mandatory)] [int] $TimeoutSeconds
    )

    $uri = [uri] (Get-GitHubArtifactResponseProperty `
            -Response $Artifact -Name 'archive_download_url')
    try {
        Invoke-WebRequest -Uri $uri -Headers $Headers -OutFile $Path `
            -TimeoutSec $TimeoutSeconds -ErrorAction Stop
    }
    catch {
        $status = Get-GitHubArtifactHttpStatusCode -ErrorRecord $_
        if ($status -eq 404) {
            throw (Get-GitHubArtifactPendingError `
                    -Message "Artifact archive is not available yet for download.")
        }
        if ($null -ne $status -and ($status -eq 408 -or $status -eq 429 -or $status -ge 500)) {
            throw (Get-GitHubArtifactTransientError `
                    -Message "GitHub artifact archive request failed with HTTP $status." `
                    -InnerException $_.Exception)
        }
        throw
    }
}

function Get-GitHubArtifactDestination {
    # Mirrors action download directory layout while rejecting names that can escape the root.
    [CmdletBinding()]
    [OutputType([string])]
    param(
        [Parameter(Mandatory)] [string] $Root,
        [Parameter(Mandatory)] $Artifact,
        [Parameter(Mandatory)] [string] $Selector,
        [Parameter(Mandatory)] [int] $SelectedCount,
        [Parameter(Mandatory)] [bool] $MergeMultiple
    )

    if ($MergeMultiple -or $Selector -ceq 'ByName' -or
        ($Selector -ceq 'ById' -and $SelectedCount -eq 1)) {
        return $Root
    }
    $name = [string] (Get-GitHubArtifactResponseProperty -Response $Artifact -Name 'name')
    if ([string]::IsNullOrWhiteSpace($name) -or $name -cin @('.', '..') -or
        $name -cmatch '[\\/]') {
        throw "Artifact name '$name' cannot be used as a download directory."
    }
    return Join-Path $Root $name
}

function Invoke-GitHubRunArtifactDownload {
    # Resolves, downloads and extracts one exact same-run selector through a bounded retry budget.
    # Archives are staged completely before extraction, so a later 404 cannot leave partial files.
    [CmdletBinding(DefaultParameterSetName = 'ById')]
    param(
        [Parameter(Mandatory)] [ValidatePattern('^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$')]
        [string] $Repository,
        [Parameter(Mandatory)] [ValidateRange(1, [long]::MaxValue)] [long] $RunId,
        [Parameter(Mandatory)] [uri] $ApiUrl,
        [Parameter(Mandatory)] [ValidateNotNullOrEmpty()] [string] $GitHubToken,
        [Parameter(Mandatory)] [ValidateNotNullOrEmpty()] [string] $Path,
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
        [Parameter(Mandatory, ParameterSetName = 'ByPattern')]
        [ValidateNotNullOrEmpty()]
        [hashtable] $ProducerJob,
        [Parameter(Mandatory, ParameterSetName = 'ByPattern')]
        [ValidateRange(1, [int]::MaxValue)]
        [int] $RunAttempt,
        [switch] $MergeMultiple,
        # These defaults cover short service propagation without letting a failed handoff hold CI.
        [ValidateRange(1, 100)] [int] $Attempt = 12,
        # A fixed delay keeps the bounded wait short while avoiding a tight REST polling loop.
        [ValidateRange(0, 300)] [int] $DelaySeconds = 5,
        # Each REST and archive request must fail within the overall handoff budget.
        [ValidateRange(1, 300)] [int] $RequestTimeoutSeconds = 30
    )

    if ($PSCmdlet.ParameterSetName -ceq 'ByPattern') {
        if ($ExpectedCount -ne $ProducerJob.Count) {
            throw 'ExpectedCount must equal the producer-job mapping count.'
        }
    }

    $selector = switch ($PSCmdlet.ParameterSetName) {
        'ById' { "artifact ID $ArtifactId" }
        'ByName' { "artifact name '$Name'" }
        'ByPattern' { "artifact pattern '$Pattern'" }
    }
    $headers = @{
        Accept                = 'application/vnd.github+json'
        Authorization         = "Bearer $GitHubToken"
        'X-GitHub-Api-Version' = '2022-11-28'
    }
    $temporaryRoot = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
    $requestApiUrl = $ApiUrl
    $requestRepository = $Repository
    $downloadPath = $Path
    $downloadMergeMultiple = $MergeMultiple.IsPresent
    $requestTimeout = $RequestTimeoutSeconds
    $requestRunAttempt = $RunAttempt

    try {
        Invoke-WithRetry -Attempt $Attempt -DelaySeconds $DelaySeconds -RetryOn {
            param($errorRecord)

            return $errorRecord.FullyQualifiedErrorId -cin @(
                'ArtifactVisibilityPending',
                'ArtifactRequestTransient'
            ) -or (Test-TransientFailure -Message $errorRecord.Exception.Message)
        } -Action {
            $available = @(Get-GitHubRunArtifact -ApiUrl $requestApiUrl `
                    -Repository $requestRepository -RunId $RunId -Headers $headers `
                    -TimeoutSeconds $requestTimeout)
            $selected = @(switch ($PSCmdlet.ParameterSetName) {
                    'ById' {
                        Select-GitHubRunArtifact -Artifact $available -ArtifactId $ArtifactId
                    }
                    'ByName' {
                        Select-GitHubRunArtifact -Artifact $available -Name $Name
                    }
                    'ByPattern' {
                        Select-GitHubRunArtifact -Artifact $available -Pattern $Pattern `
                            -ProducerJob $ProducerJob
                    }
                })

            $selectionComplete = if ($PSCmdlet.ParameterSetName -ceq 'ByPattern') {
                $selected.Count -ge $ExpectedCount
            }
            else {
                $selected.Count -gt 0
            }
            if (-not $selectionComplete) {
                throw (Get-GitHubArtifactPendingError `
                        -Message "$selector is not yet visible in workflow run $RunId.")
            }
            Assert-GitHubRunArtifactSelection -Artifact $selected -RunId $RunId

            if ($PSCmdlet.ParameterSetName -ceq 'ByPattern') {
                $jobs = @(Get-GitHubRunAttemptJob -ApiUrl $requestApiUrl `
                        -Repository $requestRepository `
                        -RunId $RunId -RunAttempt $requestRunAttempt -Headers $headers `
                        -TimeoutSeconds $requestTimeout)
                $stale = @(Get-GitHubArtifactFreshness -Artifact $selected -Job $jobs `
                        -ProducerJob $ProducerJob -RunAttempt $requestRunAttempt)
                if ($stale.Count -gt 0) {
                    throw (Get-GitHubArtifactPendingError `
                            -Message "$selector has stale producer artifacts: $($stale -join ', ').")
                }
            }

            $downloadRoot = Join-Path $temporaryRoot "folo-artifacts-$([guid]::NewGuid().ToString('N'))"
            New-Item -ItemType Directory -Path $downloadRoot | Out-Null
            try {
                $archives = @()
                foreach ($item in $selected) {
                    $id = [long] (Get-GitHubArtifactResponseProperty -Response $item -Name 'id')
                    $archive = Join-Path $downloadRoot "$id.zip"
                    Save-GitHubArtifactArchive -Artifact $item -Path $archive -Headers $headers `
                        -TimeoutSeconds $requestTimeout
                    $archives += @{ artifact = $item; path = $archive }
                }

                New-Item -ItemType Directory -Path $downloadPath -Force | Out-Null
                foreach ($entry in $archives) {
                    $destination = Get-GitHubArtifactDestination -Root $downloadPath `
                        -Artifact $entry.artifact -Selector $PSCmdlet.ParameterSetName `
                        -SelectedCount $selected.Count -MergeMultiple:$downloadMergeMultiple
                    New-Item -ItemType Directory -Path $destination -Force | Out-Null
                    Expand-Archive -LiteralPath $entry.path -DestinationPath $destination -Force
                }
                Write-Host "Downloaded $selector from workflow run $RunId."
            }
            finally {
                Remove-Item -LiteralPath $downloadRoot -Recurse -Force -ErrorAction SilentlyContinue
            }
        }
    }
    catch {
        if ($_.FullyQualifiedErrorId -cin @(
                'ArtifactVisibilityPending',
                'ArtifactRequestTransient'
            )) {
            throw [System.InvalidOperationException]::new(
                "$selector did not become downloadable after $Attempt attempts in workflow run $RunId."
            )
        }
        throw
    }
}

Export-ModuleMember -Function Invoke-GitHubRunArtifactDownload
